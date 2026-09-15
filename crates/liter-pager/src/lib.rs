//! Page cache, rollback journal, and WAL for Liter-rs.
//!
//! Mirrors `pager.c` / `wal.c`. This module owns every byte of the database
//! file: nothing above the pager may perform raw I/O.
//!
//! ## Transaction state machine
//! ```text
//! Unlocked ──begin_write──► Writer ──commit_phase_one──► WriterLocked
//!   ▲                         │                                │
//!   └─────────────────────────┘                                │
//!              rollback                       commit_phase_two / rollback
//!                                                              ▼
//!                                                           Reader
//! ```
//!
//! ## Rollback journal format (byte-compatible with C SQLite)
//! ```text
//! Header (28 bytes):
//!   [0.. 7]  magic: D9 D5 05 F9 20 A1 63 D7
//!   [8..11]  n_records: u32 BE  (0xFFFFFFFF during write = unknown)
//!   [12..15] nonce: u32 BE  (random; seed for per-page checksum)
//!   [16..19] db_size_at_start: u32 BE
//!   [20..23] sector_size: u32 BE
//!   [24..27] page_size: u32 BE
//!
//! Each record (page_size + 8 bytes):
//!   [0..3]               pgno: u32 BE
//!   [4 .. 4+page_size]   original page data
//!   [-4..]               checksum: u32 BE
//! ```
//!
//! ## WAL file format (byte-compatible with C SQLite)
//! ```text
//! WAL header (32 bytes):
//!   [0..3]   magic: 0x377F0682 (big-endian) or 0x377F0683 (native)
//!   [4..7]   file format version: 3007000
//!   [8..11]  page size
//!   [12..15] checkpoint sequence number
//!   [16..19] salt-1 (random)
//!   [20..23] salt-2 (random)
//!   [24..27] checksum-1
//!   [28..31] checksum-2
//!
//! WAL frame header (24 bytes), followed by raw page data:
//!   [0..3]   page number
//!   [4..7]   database size after this frame (0 = not a commit frame)
//!   [8..11]  salt-1
//!   [12..15] salt-2
//!   [16..19] checksum-1
//!   [20..23] checksum-2
//! ```

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ── Public type aliases & enums ───────────────────────────────────────────────

/// A page number (1-based; 0 is invalid).
pub type PageNumber = u32;

/// Minimum/default page sizes.
pub const MIN_PAGE_SIZE: u16 = 512;
pub const DEFAULT_PAGE_SIZE: u16 = 4096;
/// SQLite maximum page size (65536). The on-disk header encodes this as 1.
pub const MAX_PAGE_SIZE: u32 = 65536;

/// Default maximum pages held in the page cache.
pub const DEFAULT_CACHE_SIZE: usize = 2000;

/// WAL checkpoint modes (`SQLITE_CHECKPOINT_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointMode {
    Passive,
    Full,
    Restart,
    Truncate,
}

/// Pager error type.
#[derive(Debug, thiserror::Error)]
pub enum PagerError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database is corrupt")]
    Corrupt,
    #[error("out of memory")]
    NoMem,
    #[error("database is busy")]
    Busy,
    #[error("not in a write transaction")]
    NotInTransaction,
    #[error("database is read-only")]
    ReadOnly,
    #[error("page {0} out of range")]
    PageOutOfRange(u32),
}

pub type PagerResult<T> = Result<T, PagerError>;

/// An immutable, reference-counted snapshot of one page's bytes.
pub type PageRef = Arc<Vec<u8>>;

/// Pager state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagerState {
    Unlocked,
    Reader,
    /// Write transaction open; journal being written.
    Writer,
    /// Journal flushed; dirty pages written to the database file.
    WriterLocked,
    Error,
}

// ── Journal magic ─────────────────────────────────────────────────────────────

const JOURNAL_MAGIC: [u8; 8] = [0xD9, 0xD5, 0x05, 0xF9, 0x20, 0xA1, 0x63, 0xD7];
const DB_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const JOURNAL_HEADER_SIZE: u64 = 28;

// ── WAL constants ─────────────────────────────────────────────────────────────

/// WAL file magic (big-endian checksum variant, matching C SQLite default).
const WAL_MAGIC: u32 = 0x377F0682;
const WAL_FORMAT_VERSION: u32 = 3_007_000;
/// Size of the WAL file header in bytes.
const WAL_HDR_SIZE: u64 = 32;
/// Size of a single WAL frame header in bytes.
const WAL_FRAME_HDR_SIZE: u64 = 24;

// ── Internal types ────────────────────────────────────────────────────────────

struct CachedPage {
    data: Vec<u8>,
    dirty: bool,
}

struct RollbackJournal {
    file: File,
    path: PathBuf,
    nonce: u32,
    n_records: u32,
    db_size_at_start: u32,
}

/// Snapshot of dirty-page contents taken when a savepoint is opened.
/// On savepoint rollback we restore pages directly from this snapshot
/// without touching the journal, so the enclosing transaction stays live.
pub struct Savepoint {
    /// Data of every dirty page at savepoint-open time.
    dirty_pages: HashMap<PageNumber, Vec<u8>>,
    /// Length of `dirty_order` at savepoint-open time.
    n_dirty_at_open: usize,
    /// `db_size` at savepoint-open time.
    db_size_at_open: u32,
}

// ── WAL index (in-process, single-connection) ─────────────────────────────────

/// Lightweight metadata about one WAL frame (stored in-process).
#[derive(Clone)]
struct WalFrameMeta {
    pgno: PageNumber,
    /// Database size in pages at the point this commit was written
    /// (non-zero only for commit frames).
    db_size: u32,
}

/// In-process WAL index.  Mirrors the critical fields of the shm-file-backed
/// `WalCkptInfo` / `WalIndexHdr` in C SQLite, but without the shared-memory
/// complexity (single connection model for Phase 2).
struct WalIndex {
    /// Frames appended so far (mxFrame in C SQLite).
    mx_frame: u32,
    /// Random salts used for frame checksums.
    salt: [u32; 2],
    /// Per-frame metadata (1-indexed; index 0 unused).
    frames: Vec<WalFrameMeta>,
    /// Page size, stored here so the WAL can compute frame offsets.
    page_size: u32,
    /// Checksum accumulators carried across the WAL header.
    cksum: [u32; 2],
}

impl WalIndex {
    fn new(page_size: u32, salt: [u32; 2], cksum: [u32; 2]) -> Self {
        Self {
            mx_frame: 0,
            salt,
            frames: vec![WalFrameMeta {
                pgno: 0,
                db_size: 0,
            }], // index 0 unused
            page_size,
            cksum,
        }
    }

    /// Find the most recent WAL frame for `pgno`, if any.
    fn find_latest_frame(&self, pgno: PageNumber) -> Option<u32> {
        // Scan backward to find the most recent commit frame for pgno.
        // Only frames that are part of a committed transaction are visible
        // (i.e., frames whose db_size != 0, or frames before the last commit).
        // `frames` never has gaps below `mx_frame` (and `last_commit_frame`
        // never returns more than that), so indexing directly is safe.
        let last_commit = self.last_commit_frame();
        (1..=last_commit)
            .rev()
            .find(|&frame_no| self.frames[frame_no as usize].pgno == pgno)
    }

    /// The index of the last commit frame visible to readers.
    fn last_commit_frame(&self) -> u32 {
        // Walk backward from mx_frame to find the last frame with db_size != 0.
        // `frames` never has gaps below `mx_frame`, so indexing is safe.
        for i in (1..=self.mx_frame).rev() {
            if self.frames[i as usize].db_size != 0 {
                return i;
            }
        }
        0
    }

    /// Byte offset in the WAL file of the data portion of frame `frame_no`.
    fn frame_data_offset(&self, frame_no: u32) -> u64 {
        WAL_HDR_SIZE
            + (frame_no as u64 - 1) * (WAL_FRAME_HDR_SIZE + self.page_size as u64)
            + WAL_FRAME_HDR_SIZE
    }

    /// Byte offset in the WAL file of the header of frame `frame_no`.
    fn frame_hdr_offset(&self, frame_no: u32) -> u64 {
        WAL_HDR_SIZE + (frame_no as u64 - 1) * (WAL_FRAME_HDR_SIZE + self.page_size as u64)
    }
}

/// Open WAL state for a database connection.
struct Wal {
    file: File,
    #[allow(dead_code)]
    path: PathBuf,
    index: WalIndex,
}

impl Wal {
    /// Create or open the WAL file for `db_path`, reading its header if present.
    fn open(db_path: &Path, page_size: u16) -> PagerResult<Self> {
        let path = wal_path(db_path);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;

        let file_len = file.metadata()?.len();
        let ps = page_size as u32;

        if file_len >= WAL_HDR_SIZE {
            // Try to read an existing WAL header.
            let mut hdr = [0u8; 32];
            pread(&file, 0, &mut hdr)?;
            let magic = u32::from_be_bytes(hdr[0..4].try_into().unwrap());
            if magic == WAL_MAGIC || magic == WAL_MAGIC | 1 {
                let file_ps = u32::from_be_bytes(hdr[8..12].try_into().unwrap());
                let salt1 = u32::from_be_bytes(hdr[16..20].try_into().unwrap());
                let salt2 = u32::from_be_bytes(hdr[20..24].try_into().unwrap());
                let cksum1 = u32::from_be_bytes(hdr[24..28].try_into().unwrap());
                let cksum2 = u32::from_be_bytes(hdr[28..32].try_into().unwrap());

                let effective_ps = if file_ps == 0 { ps } else { file_ps };
                let frame_size = WAL_FRAME_HDR_SIZE + effective_ps as u64;
                // `file_len >= WAL_HDR_SIZE` was already established by the
                // enclosing `if` above, so this can't underflow.
                let n_frames = ((file_len - WAL_HDR_SIZE) / frame_size) as u32;

                let mut index = WalIndex::new(effective_ps, [salt1, salt2], [cksum1, cksum2]);

                // Replay frame headers to build the in-process index.
                //
                // `n_frames` is computed above by flooring `(file_len -
                // WAL_HDR_SIZE) / frame_size`, which guarantees every
                // offset this loop reads from is within the file as it
                // stood when `file_len` was measured. So in practice this
                // can only fail via a genuine race (another process
                // truncating the WAL file concurrently) rather than any
                // reachable combination of header/content values; it's kept
                // as a hard stop rather than an unwrap purely as defense
                // against that race.
                for frame_no in 1..=n_frames {
                    let off = index.frame_hdr_offset(frame_no);
                    let mut fhdr = [0u8; 24];
                    if pread(&file, off, &mut fhdr).is_err() {
                        break;
                    }
                    // Validate salt (only include frames with matching salt).
                    let fs1 = u32::from_be_bytes(fhdr[8..12].try_into().unwrap());
                    let fs2 = u32::from_be_bytes(fhdr[12..16].try_into().unwrap());
                    if fs1 != salt1 || fs2 != salt2 {
                        break;
                    }
                    let pgno = u32::from_be_bytes(fhdr[0..4].try_into().unwrap());
                    let db_size = u32::from_be_bytes(fhdr[4..8].try_into().unwrap());
                    index.frames.push(WalFrameMeta { pgno, db_size });
                    index.mx_frame = frame_no;
                }

                return Ok(Self { file, path, index });
            }
        }

        // New WAL: write the header.
        let salt1 = random_u32();
        let salt2 = random_u32();
        let mut hdr = [0u8; 32];
        hdr[0..4].copy_from_slice(&WAL_MAGIC.to_be_bytes());
        hdr[4..8].copy_from_slice(&WAL_FORMAT_VERSION.to_be_bytes());
        hdr[8..12].copy_from_slice(&ps.to_be_bytes());
        hdr[12..16].copy_from_slice(&0u32.to_be_bytes()); // ckpt seq
        hdr[16..20].copy_from_slice(&salt1.to_be_bytes());
        hdr[20..24].copy_from_slice(&salt2.to_be_bytes());
        // Compute checksum over first 24 bytes of header.
        let [c1, c2] = wal_checksum(&hdr[0..24], [0, 0]);
        hdr[24..28].copy_from_slice(&c1.to_be_bytes());
        hdr[28..32].copy_from_slice(&c2.to_be_bytes());
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&hdr)?;
        file.sync_all()?;

        Ok(Self {
            file,
            path,
            index: WalIndex::new(ps, [salt1, salt2], [c1, c2]),
        })
    }

    /// Read the page data for the given frame number.
    fn read_frame_data(&mut self, frame_no: u32) -> PagerResult<Vec<u8>> {
        let ps = self.index.page_size as usize;
        let off = self.index.frame_data_offset(frame_no);
        let mut data = vec![0u8; ps];
        pread(&self.file, off, &mut data)?;
        Ok(data)
    }

    /// Append a batch of (pgno, data) frames as a single commit.
    ///
    /// `db_size` is the total database size in pages after this commit.
    fn append_frames(&mut self, frames: &[(PageNumber, Vec<u8>)], db_size: u32) -> PagerResult<()> {
        let ps = self.index.page_size as usize;
        let [salt1, salt2] = self.index.salt;
        let mut cksum = self.index.cksum;

        let n = frames.len();
        for (i, (pgno, data)) in frames.iter().enumerate() {
            let is_commit = i + 1 == n;
            let frame_db_size = if is_commit { db_size } else { 0 };

            // Build frame header.
            let mut fhdr = [0u8; 24];
            fhdr[0..4].copy_from_slice(&pgno.to_be_bytes());
            fhdr[4..8].copy_from_slice(&frame_db_size.to_be_bytes());
            fhdr[8..12].copy_from_slice(&salt1.to_be_bytes());
            fhdr[12..16].copy_from_slice(&salt2.to_be_bytes());
            // Checksum covers frame header bytes [0..8] then the page data.
            cksum = wal_checksum(&fhdr[0..8], cksum);
            cksum = wal_checksum(data, cksum);
            fhdr[16..20].copy_from_slice(&cksum[0].to_be_bytes());
            fhdr[20..24].copy_from_slice(&cksum[1].to_be_bytes());

            // Write frame header then data.
            let frame_no = self.index.mx_frame + 1;
            let off = self.index.frame_hdr_offset(frame_no);
            self.file.seek(SeekFrom::Start(off))?;
            self.file.write_all(&fhdr)?;
            if data.len() < ps {
                let mut padded = data.clone();
                padded.resize(ps, 0);
                self.file.write_all(&padded)?;
            } else {
                self.file.write_all(&data[..ps])?;
            }

            self.index.frames.push(WalFrameMeta {
                pgno: *pgno,
                db_size: frame_db_size,
            });
            self.index.mx_frame = frame_no;
            self.index.cksum = cksum;
        }
        self.file.sync_all()?;
        Ok(())
    }

    /// Copy WAL frames back to the main database file (checkpoint).
    ///
    /// Returns `(frames_in_wal, frames_checkpointed)`.
    fn checkpoint(
        &mut self,
        db_file: &mut File,
        page_size: u32,
        mode: CheckpointMode,
    ) -> PagerResult<(u32, u32)> {
        let last_commit = self.index.last_commit_frame();
        if last_commit == 0 {
            return Ok((0, 0));
        }

        let ps = page_size as usize;
        let mut copied = 0u32;

        for frame_no in 1..=last_commit {
            // `last_commit` came from `last_commit_frame()`, which only
            // returns an index it already found present in `frames`, and
            // `frames` never has gaps below its highest index — so every
            // frame_no in this range is guaranteed to exist.
            let meta = self.index.frames[frame_no as usize].clone();
            if meta.pgno == 0 {
                continue;
            }

            let data = self.read_frame_data(frame_no)?;
            let off = (meta.pgno as u64 - 1) * ps as u64;
            db_file.seek(SeekFrom::Start(off))?;
            db_file.write_all(&data)?;
            copied += 1;
        }
        db_file.sync_all()?;

        if matches!(mode, CheckpointMode::Truncate | CheckpointMode::Restart) {
            // Reset the WAL: truncate to just the header.
            db_file.sync_all()?;
            self.file.set_len(WAL_HDR_SIZE)?;
            self.file.sync_all()?;
            self.index.mx_frame = 0;
            self.index.frames.truncate(1); // keep index-0 sentinel
        }

        Ok((last_commit, copied))
    }
}

// ── WAL checksum ──────────────────────────────────────────────────────────────

/// Compute the WAL checksum for `data` starting from `init`.
///
/// SQLite uses a big-endian checksum that treats the data as u32 pairs.
/// Unpaired trailing bytes are ignored (data length need not be a multiple of 8).
fn wal_checksum(data: &[u8], init: [u32; 2]) -> [u32; 2] {
    let [mut s1, mut s2] = init;
    let mut i = 0;
    while i + 8 <= data.len() {
        s1 = s1
            .wrapping_add(u32::from_be_bytes(data[i..i + 4].try_into().unwrap()))
            .wrapping_add(s2);
        s2 = s2
            .wrapping_add(u32::from_be_bytes(data[i + 4..i + 8].try_into().unwrap()))
            .wrapping_add(s1);
        i += 8;
    }
    [s1, s2]
}

// ── Pager ─────────────────────────────────────────────────────────────────────

/// The page-level I/O manager for one database connection.
pub struct Pager {
    file: File,
    path: PathBuf,
    journal: Option<RollbackJournal>,
    wal: Option<Wal>,
    page_size: u16,
    cache: HashMap<PageNumber, CachedPage>,
    /// Page numbers in first-dirty order (deterministic write order on commit).
    dirty_order: Vec<PageNumber>,
    state: PagerState,
    read_only: bool,
    /// Pages in the database as of the last commit (or file open).
    db_size: u32,
    savepoints: Vec<Savepoint>,
    cache_size: usize,
}

impl Pager {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Open or create a database at `path`.
    ///
    /// * If the file exists, the page size is read from the database header.
    /// * If the file is empty, `default_page_size` is used.
    pub fn open(path: &Path, default_page_size: u16, read_only: bool) -> PagerResult<Self> {
        if !default_page_size.is_power_of_two() || default_page_size < MIN_PAGE_SIZE {
            return Err(PagerError::Corrupt);
        }

        let file = if read_only {
            File::open(path)?
        } else {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)?
        };

        let file_size = file.metadata()?.len();

        let (page_size, db_size) = if file_size == 0 {
            (default_page_size, 0)
        } else {
            // Try to read the SQLite header. If the magic is present we can
            // extract the page size; otherwise fall back to default_page_size
            // (the B-tree layer is responsible for validating the magic).
            let (ps, ndb) = if file_size >= 100 {
                let mut hdr = [0u8; 100];
                pread(&file, 0, &mut hdr)?;
                if &hdr[0..16] == DB_MAGIC {
                    let ps_raw = u16::from_be_bytes([hdr[16], hdr[17]]);
                    let ps: u16 = if ps_raw == 1 { 32768 } else { ps_raw };
                    let ndb = u32::from_be_bytes(hdr[28..32].try_into().unwrap());
                    let ndb = if ndb == 0 {
                        (file_size / ps as u64) as u32
                    } else {
                        ndb
                    };
                    (ps, ndb)
                } else {
                    let ndb = (file_size / default_page_size as u64) as u32;
                    (default_page_size, ndb.max(1))
                }
            } else {
                // Partial header – treat as one page.
                (default_page_size, 1)
            };
            (ps, ndb)
        };

        Ok(Self {
            file,
            path: path.to_owned(),
            journal: None,
            wal: None,
            page_size,
            cache: HashMap::new(),
            dirty_order: Vec::new(),
            state: PagerState::Unlocked,
            read_only,
            db_size,
            savepoints: Vec::new(),
            cache_size: DEFAULT_CACHE_SIZE,
        })
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    pub fn db_size(&self) -> u32 {
        self.db_size
    }
    pub fn page_size(&self) -> u16 {
        self.page_size
    }
    pub fn state(&self) -> PagerState {
        self.state
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }
    pub fn set_cache_size(&mut self, n: usize) {
        self.cache_size = n;
    }
    pub fn is_wal_mode(&self) -> bool {
        self.wal.is_some()
    }

    /// Set a new page size. Only valid on an empty (zero-page) database before
    /// any writes.
    pub fn set_page_size(&mut self, size: u16) -> PagerResult<()> {
        if self.db_size != 0 {
            return Err(PagerError::Corrupt);
        }
        if !size.is_power_of_two() || size < MIN_PAGE_SIZE {
            return Err(PagerError::Corrupt);
        }
        self.page_size = size;
        Ok(())
    }

    /// Enable WAL mode for this connection.
    ///
    /// Opens (or creates) the `<db>-wal` file and switches to WAL-mode reads.
    /// Must be called before any write transaction when WAL mode is desired.
    pub fn enable_wal(&mut self) -> PagerResult<()> {
        if self.read_only {
            return Err(PagerError::ReadOnly);
        }
        if self.wal.is_some() {
            return Ok(());
        }
        let wal = Wal::open(&self.path, self.page_size)?;
        // If the WAL has committed frames, update db_size from the last
        // commit. `last_commit_frame` only ever returns an index that (a)
        // genuinely exists in `frames` (that's how it found it) and (b) has
        // `db_size != 0` (that's its own search predicate), so neither is
        // re-checked here.
        let lc = wal.index.last_commit_frame();
        if lc > 0 {
            self.db_size = wal.index.frames[lc as usize].db_size;
        }
        self.wal = Some(wal);
        Ok(())
    }

    // ── Page access ───────────────────────────────────────────────────────────

    /// Acquire a page for **reading**. Loads from disk if not cached.
    pub fn acquire(&mut self, pgno: PageNumber) -> PagerResult<PageRef> {
        if pgno == 0 {
            return Err(PagerError::PageOutOfRange(0));
        }
        self.ensure_cached(pgno)?;
        Ok(Arc::new(self.cache[&pgno].data.clone()))
    }

    /// Look up a page already in the cache without any I/O.
    pub fn lookup(&self, pgno: PageNumber) -> Option<PageRef> {
        self.cache.get(&pgno).map(|p| Arc::new(p.data.clone()))
    }

    /// Obtain a **mutable** reference to a page's data buffer.
    ///
    /// Before the first write to a page the original content is appended to
    /// the rollback journal. Requires an active write transaction.
    pub fn write_access(&mut self, pgno: PageNumber) -> PagerResult<&mut Vec<u8>> {
        if !matches!(self.state, PagerState::Writer | PagerState::WriterLocked) {
            return Err(PagerError::NotInTransaction);
        }
        if pgno == 0 {
            return Err(PagerError::PageOutOfRange(0));
        }

        if pgno <= self.db_size {
            self.ensure_cached(pgno)?;
        } else {
            self.cache.entry(pgno).or_insert_with(|| CachedPage {
                data: vec![0u8; self.page_size as usize],
                dirty: false,
            });
        }

        if !self.cache[&pgno].dirty {
            // In rollback-journal mode, record the original page content.
            if self.wal.is_none() {
                self.journal_page(pgno)?;
            }
            let p = self.cache.get_mut(&pgno).unwrap();
            p.dirty = true;
            self.dirty_order.push(pgno);
            if pgno > self.db_size {
                self.db_size = pgno;
            }
        }

        Ok(&mut self.cache.get_mut(&pgno).unwrap().data)
    }

    // ── Transactions ──────────────────────────────────────────────────────────

    /// Begin a write transaction: creates the rollback journal file (or
    /// prepares WAL frames in WAL mode).
    pub fn begin_write(&mut self) -> PagerResult<()> {
        if self.read_only {
            return Err(PagerError::ReadOnly);
        }
        if matches!(self.state, PagerState::Writer | PagerState::WriterLocked) {
            return Ok(());
        }

        if self.wal.is_none() {
            // Rollback-journal mode.
            let jpath = journal_path(&self.path);
            let mut jfile = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&jpath)?;

            let nonce = random_u32();
            let db_size = self.db_size;

            let mut hdr = [0u8; 28];
            hdr[0..8].copy_from_slice(&JOURNAL_MAGIC);
            hdr[8..12].copy_from_slice(&u32::MAX.to_be_bytes()); // n_records = unknown
            hdr[12..16].copy_from_slice(&nonce.to_be_bytes());
            hdr[16..20].copy_from_slice(&db_size.to_be_bytes());
            hdr[20..24].copy_from_slice(&512u32.to_be_bytes()); // sector size
            hdr[24..28].copy_from_slice(&(self.page_size as u32).to_be_bytes());
            jfile.write_all(&hdr)?;
            jfile.flush()?;

            self.journal = Some(RollbackJournal {
                file: jfile,
                path: jpath,
                nonce,
                n_records: 0,
                db_size_at_start: db_size,
            });
        }
        // In WAL mode we don't need a rollback journal.

        self.state = PagerState::Writer;
        Ok(())
    }

    /// Phase-1 commit: flush journal record count, write dirty pages, sync.
    pub fn commit_phase_one(&mut self, _super_journal: Option<&Path>) -> PagerResult<()> {
        if !matches!(self.state, PagerState::Writer | PagerState::WriterLocked) {
            return Err(PagerError::NotInTransaction);
        }

        if let Some(wal) = &mut self.wal {
            // WAL mode: collect dirty pages as frames and append to WAL.
            let dirty: Vec<PageNumber> = self.dirty_order.clone();
            let mut frames: Vec<(PageNumber, Vec<u8>)> = Vec::new();
            for pgno in dirty {
                if let Some(page) = self.cache.get(&pgno) {
                    if page.dirty {
                        frames.push((pgno, page.data.clone()));
                    }
                }
            }
            let db_size = self.db_size;
            wal.append_frames(&frames, db_size)?;
        } else {
            // Rollback-journal mode: seal the journal and write dirty pages.
            if let Some(j) = &mut self.journal {
                j.file.seek(SeekFrom::Start(8))?;
                j.file.write_all(&j.n_records.to_be_bytes())?;
                j.file.sync_all()?;
            }

            // Write all dirty pages to the database file.
            let ps = self.page_size as u64;
            let dirty: Vec<PageNumber> = self.dirty_order.clone();
            for pgno in dirty {
                if let Some(page) = self.cache.get(&pgno) {
                    if page.dirty {
                        let data = page.data.clone();
                        pwrite(&mut self.file, (pgno as u64 - 1) * ps, &data)?;
                    }
                }
            }

            // Patch the database header on page 1.
            self.update_db_header()?;
            self.file.sync_all()?;
        }

        self.state = PagerState::WriterLocked;
        Ok(())
    }

    /// Phase-2 commit: delete the journal (makes the transaction permanent).
    pub fn commit_phase_two(&mut self) -> PagerResult<()> {
        if self.wal.is_none() {
            // Rollback-journal mode: delete the journal file.
            if let Some(j) = self.journal.take() {
                drop(j.file);
                let _ = std::fs::remove_file(&j.path);
            }
        }
        for p in self.cache.values_mut() {
            p.dirty = false;
        }
        self.dirty_order.clear();
        self.savepoints.clear();
        self.state = PagerState::Reader;
        Ok(())
    }

    /// Convenience: commit in a single step.
    pub fn commit(&mut self) -> PagerResult<()> {
        self.commit_phase_one(None)?;
        self.commit_phase_two()
    }

    /// Roll back by replaying the rollback journal (or discarding WAL frames).
    pub fn rollback(&mut self) -> PagerResult<()> {
        if !matches!(self.state, PagerState::Writer | PagerState::WriterLocked) {
            return Err(PagerError::NotInTransaction);
        }

        if self.wal.is_some() {
            // WAL mode: discard dirty-page cache entries.
            // The WAL frames for the aborted transaction are simply not
            // committed (mx_frame is not advanced past the last commit).
            // We need to evict dirty pages from the cache so they are re-read
            // from the WAL on next access.
            let dirty: Vec<PageNumber> = std::mem::take(&mut self.dirty_order);
            for pgno in dirty {
                self.cache.remove(&pgno);
            }
            self.savepoints.clear();
            self.state = PagerState::Reader;
            Ok(())
        } else {
            self.apply_rollback(JOURNAL_HEADER_SIZE)
        }
    }

    // ── Savepoints ────────────────────────────────────────────────────────────

    /// Snapshot current dirty-page contents as savepoint `n`.
    pub fn open_savepoint(&mut self, _n: usize) -> PagerResult<()> {
        let dirty_pages: HashMap<PageNumber, Vec<u8>> = self
            .dirty_order
            .iter()
            .filter_map(|&pgno| self.cache.get(&pgno).map(|p| (pgno, p.data.clone())))
            .collect();
        self.savepoints.push(Savepoint {
            dirty_pages,
            n_dirty_at_open: self.dirty_order.len(),
            db_size_at_open: self.db_size,
        });
        Ok(())
    }

    /// Roll back to savepoint `n` (0-based), keeping the transaction open.
    pub fn savepoint_rollback(&mut self, n: usize) -> PagerResult<()> {
        if n >= self.savepoints.len() {
            return Err(PagerError::PageOutOfRange(n as u32));
        }
        let sp_n_dirty = self.savepoints[n].n_dirty_at_open;
        let sp_db_size = self.savepoints[n].db_size_at_open;
        let sp_snapshot = self.savepoints[n].dirty_pages.clone();

        // Drop pages that were added after the savepoint.
        let pages_added: Vec<PageNumber> = self.dirty_order[sp_n_dirty..].to_vec();
        for pgno in pages_added {
            self.cache.remove(&pgno);
        }
        self.dirty_order.truncate(sp_n_dirty);

        // Restore pages that were dirty at savepoint to their snapshot values.
        for (pgno, snap) in &sp_snapshot {
            if let Some(p) = self.cache.get_mut(pgno) {
                p.data.copy_from_slice(snap);
                p.dirty = true; // still in the enclosing transaction
            }
        }

        self.db_size = sp_db_size;
        self.savepoints.truncate(n + 1);
        self.state = PagerState::Writer;
        Ok(())
    }

    /// Merge savepoint `n` into the outer transaction.
    pub fn savepoint_release(&mut self, n: usize) -> PagerResult<()> {
        if n < self.savepoints.len() {
            self.savepoints.truncate(n);
        }
        Ok(())
    }

    // ── WAL ───────────────────────────────────────────────────────────────────

    /// Checkpoint the WAL, copying committed frames back to the main database.
    ///
    /// Returns `(frames_in_wal, frames_checkpointed)`.
    pub fn checkpoint(&mut self, mode: CheckpointMode) -> PagerResult<(u32, u32)> {
        let ps = self.page_size as u32;
        if let Some(wal) = &mut self.wal {
            let result = wal.checkpoint(&mut self.file, ps, mode)?;
            // Invalidate the page cache: pages may have been updated on disk.
            self.cache.clear();
            Ok(result)
        } else {
            // Not in WAL mode — nothing to checkpoint.
            Ok((0, 0))
        }
    }

    /// Move a page's contents to a new page number (used by auto-vacuum).
    ///
    /// The old page is zeroed and the new page receives the old content.
    /// Requires an active write transaction.
    pub fn move_page(&mut self, page: PageRef, new_pgno: PageNumber) -> PagerResult<()> {
        if !matches!(self.state, PagerState::Writer | PagerState::WriterLocked) {
            return Err(PagerError::NotInTransaction);
        }
        if new_pgno == 0 {
            return Err(PagerError::PageOutOfRange(0));
        }

        let data = (*page).clone();

        // Write the data to the new page number.
        let new_data = self.write_access(new_pgno)?;
        *new_data = data;

        Ok(())
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn ensure_cached(&mut self, pgno: PageNumber) -> PagerResult<()> {
        if self.cache.contains_key(&pgno) {
            return Ok(());
        }

        // In WAL mode, check the WAL first.
        if let Some(wal) = &mut self.wal {
            if let Some(frame_no) = wal.index.find_latest_frame(pgno) {
                let data = wal.read_frame_data(frame_no)?;
                self.cache.insert(pgno, CachedPage { data, dirty: false });
                if self.cache.len() > self.cache_size {
                    self.evict_clean(pgno);
                }
                return Ok(());
            }
        }

        if pgno > self.db_size && self.db_size > 0 {
            return Err(PagerError::PageOutOfRange(pgno));
        }

        let ps = self.page_size as usize;
        let mut data = vec![0u8; ps];
        match pread(&self.file, (pgno as u64 - 1) * ps as u64, &mut data) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof && self.db_size == 0 => {}
            Err(e) => return Err(PagerError::Io(e)),
        }

        self.cache.insert(pgno, CachedPage { data, dirty: false });
        if self.cache.len() > self.cache_size {
            self.evict_clean(pgno);
        }
        Ok(())
    }

    fn journal_page(&mut self, pgno: PageNumber) -> PagerResult<()> {
        let data = self
            .cache
            .get(&pgno)
            .map(|p| p.data.clone())
            .unwrap_or_else(|| vec![0u8; self.page_size as usize]);

        let Some(j) = &mut self.journal else {
            return Ok(());
        };
        let nonce = j.nonce;
        let cksum = journal_checksum(nonce, &data);

        let mut rec = Vec::with_capacity(4 + data.len() + 4);
        rec.extend_from_slice(&pgno.to_be_bytes());
        rec.extend_from_slice(&data);
        rec.extend_from_slice(&cksum.to_be_bytes());
        j.file.write_all(&rec)?;
        j.n_records += 1;
        Ok(())
    }

    fn apply_rollback(&mut self, from_offset: u64) -> PagerResult<()> {
        let ps = self.page_size as usize;
        let records = if let Some(j) = &mut self.journal {
            j.file.seek(SeekFrom::Start(from_offset))?;
            read_journal_records(&mut j.file, ps)?
        } else {
            vec![]
        };

        let saved_db_size = self
            .journal
            .as_ref()
            .map(|j| j.db_size_at_start)
            .unwrap_or(0);

        let fps = self.page_size as u64;
        for (pgno, orig) in &records {
            pwrite(&mut self.file, (*pgno as u64 - 1) * fps, orig)?;
            if let Some(p) = self.cache.get_mut(pgno) {
                p.data.copy_from_slice(orig);
                p.dirty = false;
            }
        }
        self.file.sync_all()?;
        self.db_size = saved_db_size;

        if let Some(j) = self.journal.take() {
            drop(j.file);
            let _ = std::fs::remove_file(&j.path);
        }
        self.dirty_order.clear();
        self.savepoints.clear();
        self.state = PagerState::Reader;
        Ok(())
    }

    fn update_db_header(&mut self) -> PagerResult<()> {
        if self.db_size < 1 {
            return Ok(());
        }
        self.ensure_cached(1)?;
        // `ensure_cached` always inserts the page before returning `Ok`, so
        // it's guaranteed to be present here.
        let p = self.cache.get_mut(&1).unwrap();
        if p.data.len() >= 100 && &p.data[0..16] == DB_MAGIC {
            // Change counter (bytes 24-27): increment.
            let cc = u32::from_be_bytes(p.data[24..28].try_into().unwrap());
            p.data[24..28].copy_from_slice(&cc.wrapping_add(1).to_be_bytes());
            // Database size (bytes 28-31).
            p.data[28..32].copy_from_slice(&self.db_size.to_be_bytes());
            let data = p.data.clone();
            pwrite(&mut self.file, 0, &data)?;
        }
        Ok(())
    }

    fn evict_clean(&mut self, keep: PageNumber) {
        if let Some(v) = self
            .cache
            .iter()
            .find(|(&pgno, p)| pgno != keep && !p.dirty)
            .map(|(&pgno, _)| pgno)
        {
            self.cache.remove(&v);
        }
    }
}

// ── Pure helpers ──────────────────────────────────────────────────────────────

fn journal_checksum(nonce: u32, data: &[u8]) -> u32 {
    let mut c = nonce;
    let mut i = data.len() as isize - 200;
    while i > 0 {
        c = c.wrapping_add(data[i as usize] as u32);
        i -= 200;
    }
    c
}

fn journal_path(db: &Path) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push("-journal");
    db.with_file_name(name)
}

fn wal_path(db: &Path) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push("-wal");
    db.with_file_name(name)
}

#[cfg(unix)]
fn pread(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset)
}

#[cfg(not(unix))]
fn pread(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    // Fallback for non-Unix (Windows): clone handle then seek+read.
    // This is not atomic but sufficient for single-process use.
    let mut f = file.try_clone()?;
    f.seek(SeekFrom::Start(offset))?;
    f.read_exact(buf)
}

fn pwrite(file: &mut File, offset: u64, buf: &[u8]) -> io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(buf)
}

fn read_journal_records(file: &mut File, ps: usize) -> io::Result<Vec<(PageNumber, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut pgno_buf = [0u8; 4];
    let mut data = vec![0u8; ps];
    let mut _ckbuf = [0u8; 4];
    loop {
        match file.read_exact(&mut pgno_buf) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        }
        file.read_exact(&mut data)?;
        file.read_exact(&mut _ckbuf)?;
        out.push((u32::from_be_bytes(pgno_buf), data.clone()));
    }
    Ok(out)
}

fn random_u32() -> u32 {
    let mut b = [0u8; 4];
    getrandom::getrandom(&mut b).expect("getrandom");
    u32::from_le_bytes(b)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn new_pager() -> (Pager, NamedTempFile) {
        let f = NamedTempFile::new().unwrap();
        let p = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        (p, f)
    }

    fn new_wal_pager() -> (Pager, NamedTempFile) {
        let f = NamedTempFile::new().unwrap();
        let mut p = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        p.enable_wal().unwrap();
        (p, f)
    }

    // ── Rollback-journal tests ─────────────────────────────────────────────

    #[test]
    fn open_empty() {
        let (_p, _f) = new_pager();
    }

    #[test]
    fn write_read_page() {
        let (mut pager, f) = new_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..8].copy_from_slice(b"hello123");
        pager.commit().unwrap();

        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        let pr = p2.acquire(1).unwrap();
        assert_eq!(&pr[0..8], b"hello123");
    }

    #[test]
    fn rollback_restores_original() {
        let (mut pager, f) = new_pager();

        // First commit: write "orig".
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..4].copy_from_slice(b"orig");
        pager.commit().unwrap();

        // Second: overwrite then rollback.
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..4].copy_from_slice(b"XXXX");
        pager.rollback().unwrap();

        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        let pr = p2.acquire(1).unwrap();
        assert_eq!(&pr[0..4], b"orig");
    }

    #[test]
    fn multiple_pages() {
        let (mut pager, f) = new_pager();
        pager.begin_write().unwrap();
        for pgno in 1u32..=5 {
            pager.write_access(pgno).unwrap()[0] = pgno as u8;
        }
        pager.commit().unwrap();

        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        assert_eq!(p2.db_size(), 5);
        for pgno in 1u32..=5 {
            assert_eq!(p2.acquire(pgno).unwrap()[0], pgno as u8);
        }
    }

    #[test]
    fn savepoint_partial_rollback() {
        let (mut pager, f) = new_pager();

        // Write 0xAA to page 1.
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 0xAA;

        // Open savepoint, then overwrite with 0xBB.
        pager.open_savepoint(0).unwrap();
        pager.write_access(1).unwrap()[0] = 0xBB;

        // Roll back to savepoint: should see 0xAA again.
        pager.savepoint_rollback(0).unwrap();
        assert_eq!(pager.acquire(1).unwrap()[0], 0xAA);

        pager.begin_write().unwrap();
        pager.commit().unwrap();

        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        assert_eq!(p2.acquire(1).unwrap()[0], 0xAA);
    }

    // ── WAL tests ─────────────────────────────────────────────────────────

    #[test]
    fn wal_basic_write_read() {
        let (mut pager, _f) = new_wal_pager();
        assert!(pager.is_wal_mode());

        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..4].copy_from_slice(b"WALX");
        pager.commit().unwrap();

        // Evict cache and re-read through WAL.
        pager.cache.clear();
        let page = pager.acquire(1).unwrap();
        assert_eq!(&page[0..4], b"WALX");
    }

    #[test]
    fn wal_checkpoint_passive() {
        let (mut pager, f) = new_wal_pager();

        // Write 5 pages in WAL mode.
        pager.begin_write().unwrap();
        for pgno in 1u32..=5 {
            pager.write_access(pgno).unwrap()[0] = (pgno * 10) as u8;
        }
        pager.commit().unwrap();

        // Checkpoint: copies WAL frames to the main db file.
        let (total, copied) = pager.checkpoint(CheckpointMode::Passive).unwrap();
        assert_eq!(total, 5);
        assert_eq!(copied, 5);

        // Re-open WITHOUT WAL: should read the checkpointed data from the main db.
        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        for pgno in 1u32..=5 {
            assert_eq!(
                p2.acquire(pgno).unwrap()[0],
                (pgno * 10) as u8,
                "page {pgno} mismatch after checkpoint"
            );
        }
    }

    #[test]
    fn wal_rollback_discards_frames() {
        let (mut pager, _f) = new_wal_pager();

        // Commit an initial value.
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..4].copy_from_slice(b"INIT");
        pager.commit().unwrap();

        // Start a second write, then roll it back.
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..4].copy_from_slice(b"ABRT");
        pager.rollback().unwrap();

        // Cache was evicted; re-reading should give the committed value.
        let page = pager.acquire(1).unwrap();
        assert_eq!(&page[0..4], b"INIT");
    }

    #[test]
    fn wal_multiple_commits() {
        let (mut pager, _f) = new_wal_pager();

        // Three separate commits to the same page.
        for i in 0u8..3 {
            pager.begin_write().unwrap();
            pager.write_access(1).unwrap()[0] = i;
            pager.commit().unwrap();
            pager.cache.clear(); // force WAL re-read
            assert_eq!(pager.acquire(1).unwrap()[0], i);
        }
    }

    #[test]
    fn wal_checkpoint_truncate() {
        let (mut pager, f) = new_wal_pager();

        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 0xFF;
        pager.commit().unwrap();

        // Truncate checkpoint should reset the WAL file to header-only size.
        pager.checkpoint(CheckpointMode::Truncate).unwrap();

        // The WAL file should now be header-only (32 bytes).
        let wal_path_buf = wal_path(f.path());
        let wal_len = std::fs::metadata(&wal_path_buf).unwrap().len();
        assert_eq!(
            wal_len, WAL_HDR_SIZE,
            "WAL should be truncated to header only"
        );

        // Data should still be readable from the main db file.
        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        assert_eq!(p2.acquire(1).unwrap()[0], 0xFF);
    }

    #[test]
    fn move_page_basic() {
        let (mut pager, _f) = new_pager();

        // Write page 1.
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..4].copy_from_slice(b"data");
        pager.commit().unwrap();

        // Move page 1 content to page 2.
        pager.begin_write().unwrap();
        let pg1 = pager.acquire(1).unwrap();
        pager.move_page(pg1, 2).unwrap();
        pager.commit().unwrap();

        let page2 = pager.acquire(2).unwrap();
        assert_eq!(&page2[0..4], b"data");
    }

    #[test]
    fn wal_checksum_deterministic() {
        let data = b"SQLite WAL checksum test data!!";
        let r1 = wal_checksum(data, [0, 0]);
        let r2 = wal_checksum(data, [0, 0]);
        assert_eq!(r1, r2);
        assert_ne!(r1, [0, 0]);
    }

    #[test]
    fn crash_journal_recovery() {
        // Simulate crash by leaving a complete journal behind, then reopening.
        let (mut pager, f) = new_pager();

        // Write initial content.
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..4].copy_from_slice(b"SAFE");
        pager.commit().unwrap();

        // Begin a write but do NOT commit — simulating a crash.
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..4].copy_from_slice(b"CRSH");
        // Flush the journal header with proper record count so it's recoverable.
        if let Some(j) = &mut pager.journal {
            j.file.seek(SeekFrom::Start(8)).unwrap();
            j.file.write_all(&j.n_records.to_be_bytes()).unwrap();
            j.file.sync_all().unwrap();
        }
        // Write the dirty page to db file (simulating partial commit phase 1).
        let dirty_data = pager.cache.get(&1).unwrap().data.clone();
        pager.file.seek(SeekFrom::Start(0)).unwrap();
        pager.file.write_all(&dirty_data).unwrap();
        pager.file.sync_all().unwrap();
        // Do NOT call commit_phase_two — journal stays on disk.
        drop(pager);

        // Re-open: the pager should detect and replay the journal.
        // For now, verify the journal file exists and the db is openable.
        let jpath = journal_path(f.path());
        assert!(
            jpath.exists(),
            "journal should still exist after simulated crash"
        );

        // A real recovery would replay the journal on open; this test validates
        // that apply_rollback works by manually triggering it.
        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        // Manually replay journal (in production, Pager::open would do this).
        if jpath.exists() {
            p2.journal = Some(RollbackJournal {
                file: OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&jpath)
                    .unwrap(),
                path: jpath.clone(),
                nonce: 0,
                n_records: 0,
                db_size_at_start: 1,
            });
            p2.state = PagerState::Writer;
            p2.apply_rollback(JOURNAL_HEADER_SIZE).unwrap();
        }
        let page = p2.acquire(1).unwrap();
        assert_eq!(
            &page[0..4],
            b"SAFE",
            "journal recovery should restore pre-crash data"
        );
    }

    // ── WAL index internals ──────────────────────────────────────────────

    #[test]
    fn wal_find_latest_frame_scans_multiple_frames() {
        let (mut pager, _f) = new_wal_pager();
        pager.begin_write().unwrap();
        for pgno in 1u32..=3 {
            pager.write_access(pgno).unwrap()[0] = pgno as u8;
        }
        pager.commit().unwrap();
        pager.cache.clear();

        // Acquiring page 1 forces `find_latest_frame` to scan past the
        // non-matching frames for pages 3 and 2 before it finds page 1.
        assert_eq!(pager.acquire(1).unwrap()[0], 1);
        assert_eq!(pager.acquire(2).unwrap()[0], 2);
        assert_eq!(pager.acquire(3).unwrap()[0], 3);
    }

    #[test]
    fn wal_lookup_miss_falls_back_to_db_file() {
        let (mut pager, f) = new_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..4].copy_from_slice(b"ONE1");
        pager.write_access(2).unwrap()[0..4].copy_from_slice(b"TWO2");
        pager.commit().unwrap();
        drop(pager);

        let mut pager = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        pager.enable_wal().unwrap();
        // Rewrite page 1 via WAL; page 2 is left only in the main db file.
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..4].copy_from_slice(b"ONEW");
        pager.commit().unwrap();
        pager.cache.clear();

        // Page 2 has no WAL frame, so `find_latest_frame` returns `None` and
        // the pager must fall back to reading it from the main db file.
        assert_eq!(&pager.acquire(2).unwrap()[0..4], b"TWO2");
        assert_eq!(&pager.acquire(1).unwrap()[0..4], b"ONEW");
    }

    #[test]
    fn wal_reopen_replays_committed_frames() {
        let f = NamedTempFile::new().unwrap();
        {
            let mut pager = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
            pager.enable_wal().unwrap();
            pager.begin_write().unwrap();
            pager.write_access(1).unwrap()[0..4].copy_from_slice(b"PER1");
            pager.commit().unwrap();
            pager.begin_write().unwrap();
            pager.write_access(2).unwrap()[0..4].copy_from_slice(b"PER2");
            pager.commit().unwrap();
        } // Original pager (and its WAL file handle) dropped here.

        let mut pager2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        pager2.enable_wal().unwrap();
        assert_eq!(
            pager2.db_size(),
            2,
            "enable_wal should sync db_size from the replayed WAL"
        );
        assert_eq!(&pager2.acquire(1).unwrap()[0..4], b"PER1");
        assert_eq!(&pager2.acquire(2).unwrap()[0..4], b"PER2");
    }

    #[test]
    fn wal_reopen_stops_replay_at_salt_mismatch() {
        let f = NamedTempFile::new().unwrap();
        let wal_file_path;
        let corrupt_offset;
        {
            let mut pager = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
            pager.enable_wal().unwrap();
            pager.begin_write().unwrap();
            pager.write_access(1).unwrap()[0..4].copy_from_slice(b"GOOD");
            pager.commit().unwrap();
            pager.begin_write().unwrap();
            pager.write_access(2).unwrap()[0..4].copy_from_slice(b"BAD!");
            pager.commit().unwrap();

            let wal = pager.wal.as_ref().unwrap();
            wal_file_path = wal.path.clone();
            // Offset of frame 2's salt-1 field (bytes 8..12 of its header).
            corrupt_offset = wal.index.frame_hdr_offset(2) + 8;
        }

        // Corrupt frame 2's salt so it no longer matches the WAL header's salt.
        let mut wal_file = OpenOptions::new().write(true).open(&wal_file_path).unwrap();
        wal_file.seek(SeekFrom::Start(corrupt_offset)).unwrap();
        wal_file.write_all(&[0xFF, 0xFF, 0xFF, 0xFF]).unwrap();
        wal_file.sync_all().unwrap();
        drop(wal_file);

        let mut pager2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        pager2.enable_wal().unwrap();
        // Replay must have stopped after frame 1; frame 2 is invisible.
        assert_eq!(pager2.wal.as_ref().unwrap().index.mx_frame, 1);
        assert_eq!(&pager2.acquire(1).unwrap()[0..4], b"GOOD");
    }

    #[test]
    fn wal_append_frames_pads_short_data() {
        let (mut pager, _f) = new_wal_pager();
        let ps = pager.page_size() as usize;
        let wal = pager.wal.as_mut().unwrap();
        let short = vec![0xAB, 0xCD, 0xEF];
        wal.append_frames(&[(1, short.clone())], 1).unwrap();
        let data = wal.read_frame_data(1).unwrap();
        assert_eq!(data.len(), ps);
        assert_eq!(&data[0..3], &short[..]);
        assert!(
            data[3..].iter().all(|&b| b == 0),
            "tail must be zero-padded"
        );
    }

    #[test]
    fn checkpoint_with_no_commits_is_noop() {
        let (mut pager, _f) = new_wal_pager();
        assert_eq!(pager.checkpoint(CheckpointMode::Passive).unwrap(), (0, 0));
    }

    #[test]
    fn checkpoint_skips_uncommitted_trailing_frames() {
        let (mut pager, f) = new_wal_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 0x22;
        pager.commit().unwrap();

        // Simulate a WAL that has extra, never-committed frames appended
        // after the last real commit (e.g. a writer that crashed mid-write).
        // `last_commit_frame` must walk backward past them.
        {
            let wal = pager.wal.as_mut().unwrap();
            wal.index.frames.push(WalFrameMeta {
                pgno: 2,
                db_size: 0,
            });
            wal.index.frames.push(WalFrameMeta {
                pgno: 2,
                db_size: 0,
            });
            wal.index.mx_frame = 3;
        }

        let (total, copied) = pager.checkpoint(CheckpointMode::Passive).unwrap();
        assert_eq!(
            total, 1,
            "only the real commit frame should be checkpointed"
        );
        assert_eq!(copied, 1);

        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        assert_eq!(p2.acquire(1).unwrap()[0], 0x22);
    }

    #[test]
    fn wal_open_reinitializes_when_existing_file_has_wrong_magic() {
        let db = NamedTempFile::new().unwrap();
        let existing_wal = wal_path(db.path());
        // Long enough to pass the header-size check, but not a valid WAL magic.
        std::fs::write(&existing_wal, vec![0u8; 40]).unwrap();

        let mut pager = Pager::open(db.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        pager.enable_wal().unwrap();
        assert!(pager.is_wal_mode());

        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..4].copy_from_slice(b"FRSH");
        pager.commit().unwrap();
        pager.cache.clear();
        assert_eq!(&pager.acquire(1).unwrap()[0..4], b"FRSH");
    }

    #[test]
    fn checkpoint_skips_frames_with_pgno_zero() {
        let (mut pager, f) = new_wal_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 0x11;
        pager.commit().unwrap();

        // Inject a corrupt "commit" frame with pgno == 0 as the new latest frame.
        {
            let wal = pager.wal.as_mut().unwrap();
            wal.index.frames.push(WalFrameMeta {
                pgno: 0,
                db_size: 1,
            });
            wal.index.mx_frame = 2;
        }

        let (total, copied) = pager.checkpoint(CheckpointMode::Passive).unwrap();
        assert_eq!(total, 2);
        assert_eq!(copied, 1, "the pgno==0 frame must be skipped");

        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        assert_eq!(p2.acquire(1).unwrap()[0], 0x11);
    }

    // ── Pager::open header parsing ───────────────────────────────────────

    #[test]
    fn open_rejects_invalid_default_page_size() {
        let f = NamedTempFile::new().unwrap();
        assert!(matches!(
            Pager::open(f.path(), 1000, false),
            Err(PagerError::Corrupt)
        ));
        assert!(matches!(
            Pager::open(f.path(), 256, false),
            Err(PagerError::Corrupt)
        ));
    }

    #[test]
    fn open_readonly_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.db");
        assert!(Pager::open(&path, DEFAULT_PAGE_SIZE, true).is_err());
    }

    #[test]
    fn open_reads_page_size_and_autocalculates_db_size() {
        let f = NamedTempFile::new().unwrap();
        let mut buf = vec![0u8; 512 * 3];
        buf[0..16].copy_from_slice(DB_MAGIC);
        buf[16..18].copy_from_slice(&512u16.to_be_bytes());
        // ndb left as 0 -> the pager must derive it from file_size / page_size.
        std::fs::write(f.path(), &buf).unwrap();

        let pager = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        assert_eq!(pager.page_size(), 512);
        assert_eq!(pager.db_size(), 3);
    }

    #[test]
    fn open_maps_page_size_raw_one_to_32768() {
        let f = NamedTempFile::new().unwrap();
        let mut buf = vec![0u8; 100];
        buf[0..16].copy_from_slice(DB_MAGIC);
        buf[16..18].copy_from_slice(&1u16.to_be_bytes());
        buf[28..32].copy_from_slice(&1u32.to_be_bytes());
        std::fs::write(f.path(), &buf).unwrap();

        let pager = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        assert_eq!(pager.page_size(), 32768);
        assert_eq!(pager.db_size(), 1);
    }

    #[test]
    fn open_without_magic_falls_back_to_default_page_size() {
        let f = NamedTempFile::new().unwrap();
        let buf = vec![0u8; 200]; // no SQLite magic present
        std::fs::write(f.path(), &buf).unwrap();

        let pager = Pager::open(f.path(), 512, false).unwrap();
        assert_eq!(pager.page_size(), 512);
        assert_eq!(pager.db_size(), 1); // (200 / 512) = 0, clamped to 1
    }

    #[test]
    fn open_with_partial_header_treats_as_one_page() {
        let f = NamedTempFile::new().unwrap();
        std::fs::write(f.path(), vec![0u8; 40]).unwrap();

        let pager = Pager::open(f.path(), 512, false).unwrap();
        assert_eq!(pager.page_size(), 512);
        assert_eq!(pager.db_size(), 1);
    }

    // ── Accessors & simple validation ────────────────────────────────────

    #[test]
    fn accessors_report_expected_state() {
        let (mut pager, f) = new_pager();
        assert_eq!(pager.page_size(), DEFAULT_PAGE_SIZE);
        assert_eq!(pager.state(), PagerState::Unlocked);
        assert_eq!(pager.path(), f.path());
        assert!(!pager.is_read_only());
        assert!(!pager.is_wal_mode());
        pager.set_cache_size(7);
        assert_eq!(pager.cache_size, 7);
        pager.begin_write().unwrap();
        assert_eq!(pager.state(), PagerState::Writer);
    }

    #[test]
    fn set_page_size_validates_and_updates() {
        let (mut pager, _f) = new_pager();
        pager.set_page_size(8192).unwrap();
        assert_eq!(pager.page_size(), 8192);

        // Not a power of two / below the minimum.
        assert!(pager.set_page_size(3000).is_err());

        // Once the database has pages, the page size is locked.
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap();
        pager.commit().unwrap();
        assert!(pager.set_page_size(4096).is_err());
    }

    #[test]
    fn enable_wal_errors_on_readonly_and_is_idempotent() {
        let f = NamedTempFile::new().unwrap();
        {
            Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        }
        let mut ro = Pager::open(f.path(), DEFAULT_PAGE_SIZE, true).unwrap();
        assert!(matches!(ro.enable_wal(), Err(PagerError::ReadOnly)));

        let (mut pager, _f2) = new_wal_pager();
        assert!(pager.is_wal_mode());
        pager.enable_wal().unwrap(); // second call is a no-op
        assert!(pager.is_wal_mode());
    }

    #[test]
    fn acquire_rejects_page_zero() {
        let (mut pager, _f) = new_pager();
        assert!(matches!(
            pager.acquire(0),
            Err(PagerError::PageOutOfRange(0))
        ));
    }

    #[test]
    fn lookup_returns_cached_page_or_none() {
        let (mut pager, _f) = new_pager();
        assert!(pager.lookup(1).is_none());
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 42;
        assert_eq!(pager.lookup(1).unwrap()[0], 42);
    }

    #[test]
    fn write_access_errors_outside_transaction_and_on_page_zero() {
        let (mut pager, _f) = new_pager();
        assert!(matches!(
            pager.write_access(1),
            Err(PagerError::NotInTransaction)
        ));
        pager.begin_write().unwrap();
        assert!(matches!(
            pager.write_access(0),
            Err(PagerError::PageOutOfRange(0))
        ));
    }

    #[test]
    fn begin_write_errors_on_readonly_pager() {
        let f = NamedTempFile::new().unwrap();
        {
            Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        }
        let mut ro = Pager::open(f.path(), DEFAULT_PAGE_SIZE, true).unwrap();
        assert!(matches!(ro.begin_write(), Err(PagerError::ReadOnly)));
    }

    #[test]
    fn commit_phase_one_errors_outside_transaction() {
        let (mut pager, _f) = new_pager();
        assert!(matches!(
            pager.commit_phase_one(None),
            Err(PagerError::NotInTransaction)
        ));
    }

    #[test]
    fn rollback_errors_outside_transaction() {
        let (mut pager, _f) = new_pager();
        assert!(matches!(
            pager.rollback(),
            Err(PagerError::NotInTransaction)
        ));
    }

    // ── Savepoints ────────────────────────────────────────────────────────

    #[test]
    fn savepoint_rollback_rejects_invalid_index() {
        let (mut pager, _f) = new_pager();
        pager.begin_write().unwrap();
        assert!(matches!(
            pager.savepoint_rollback(0),
            Err(PagerError::PageOutOfRange(0))
        ));
    }

    #[test]
    fn savepoint_rollback_discards_pages_added_after_savepoint() {
        let (mut pager, _f) = new_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 1;
        pager.open_savepoint(0).unwrap();
        // Page 2 didn't exist at savepoint-open time.
        pager.write_access(2).unwrap()[0] = 2;
        assert_eq!(pager.db_size(), 2);

        pager.savepoint_rollback(0).unwrap();

        assert_eq!(
            pager.db_size(),
            1,
            "db_size should revert to its savepoint-open value"
        );
        assert!(
            pager.lookup(2).is_none(),
            "page 2 should be evicted from the cache"
        );
    }

    #[test]
    fn savepoint_release_merges_into_enclosing_transaction() {
        let (mut pager, _f) = new_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 1;
        pager.open_savepoint(0).unwrap();
        pager.write_access(1).unwrap()[0] = 2;
        pager.open_savepoint(1).unwrap();
        pager.write_access(1).unwrap()[0] = 3;

        // Releasing savepoint 0 drops both savepoints without touching data.
        pager.savepoint_release(0).unwrap();
        assert!(pager.savepoints.is_empty());
        assert_eq!(pager.acquire(1).unwrap()[0], 3);
    }

    // ── Checkpoint / move_page edge cases ────────────────────────────────

    #[test]
    fn checkpoint_without_wal_is_noop() {
        let (mut pager, _f) = new_pager();
        assert_eq!(pager.checkpoint(CheckpointMode::Full).unwrap(), (0, 0));
    }

    #[test]
    fn move_page_errors_outside_transaction_and_on_target_zero() {
        let (mut pager, _f) = new_pager();
        let page: PageRef = Arc::new(vec![0u8; DEFAULT_PAGE_SIZE as usize]);
        assert!(matches!(
            pager.move_page(page.clone(), 1),
            Err(PagerError::NotInTransaction)
        ));
        pager.begin_write().unwrap();
        assert!(matches!(
            pager.move_page(page, 0),
            Err(PagerError::PageOutOfRange(0))
        ));
    }

    // ── Cache eviction ────────────────────────────────────────────────────

    #[test]
    fn cache_evicts_clean_pages_beyond_cache_size_disk_mode() {
        let (mut pager, _f) = new_pager();
        pager.begin_write().unwrap();
        for pgno in 1u32..=5 {
            pager.write_access(pgno).unwrap()[0] = pgno as u8;
        }
        pager.commit().unwrap();
        pager.cache.clear();
        pager.set_cache_size(2);

        for pgno in 1u32..=5 {
            pager.acquire(pgno).unwrap();
        }
        assert_eq!(
            pager.cache.len(),
            2,
            "cache should settle back at cache_size after eviction"
        );
    }

    #[test]
    fn cache_evicts_clean_pages_beyond_cache_size_wal_mode() {
        let (mut pager, _f) = new_wal_pager();
        pager.begin_write().unwrap();
        for pgno in 1u32..=5 {
            pager.write_access(pgno).unwrap()[0] = pgno as u8;
        }
        pager.commit().unwrap();
        pager.cache.clear();
        pager.set_cache_size(2);

        for pgno in 1u32..=5 {
            pager.acquire(pgno).unwrap();
        }
        assert_eq!(
            pager.cache.len(),
            2,
            "cache should settle back at cache_size after eviction"
        );
    }

    #[test]
    fn evict_clean_leaves_cache_oversized_when_all_other_pages_are_dirty() {
        let (mut pager, _f) = new_wal_pager();

        // Commit two pages so page 2 can later be re-read from the WAL.
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 1;
        pager.write_access(2).unwrap()[0] = 2;
        pager.commit().unwrap();
        pager.cache.clear();

        pager.set_cache_size(1);
        pager.begin_write().unwrap();
        // Dirty two pages so neither is a candidate for eviction.
        pager.write_access(1).unwrap()[0] = 11;
        pager.write_access(3).unwrap()[0] = 33; // new page, also dirty
        assert_eq!(pager.cache.len(), 2);

        // Reading page 2 (clean, from the WAL) inserts a 3rd cache entry;
        // eviction can't find any clean *other* page, so the cache grows
        // past cache_size instead of panicking or corrupting state.
        pager.acquire(2).unwrap();
        assert_eq!(pager.cache.len(), 3, "no clean page was available to evict");
    }

    // ── ensure_cached edge cases ──────────────────────────────────────────

    #[test]
    fn acquire_errors_when_page_exceeds_db_size() {
        let (mut pager, _f) = new_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap();
        pager.commit().unwrap();

        assert!(matches!(
            pager.acquire(5),
            Err(PagerError::PageOutOfRange(5))
        ));
    }

    #[test]
    fn acquire_on_empty_db_returns_zeroed_page() {
        let (mut pager, _f) = new_pager();
        let page = pager.acquire(1).unwrap();
        assert!(page.iter().all(|&b| b == 0));
        assert_eq!(page.len(), DEFAULT_PAGE_SIZE as usize);
    }

    #[test]
    fn acquire_propagates_io_error_when_file_shorter_than_expected() {
        let (mut pager, f) = new_pager();
        pager.begin_write().unwrap();
        for pgno in 1u32..=3 {
            pager.write_access(pgno).unwrap()[0] = pgno as u8;
        }
        pager.commit().unwrap();
        pager.cache.clear();

        // Truncate the underlying file so page 3's bytes no longer exist,
        // while the pager still believes db_size == 3.
        let file = OpenOptions::new().write(true).open(f.path()).unwrap();
        file.set_len(DEFAULT_PAGE_SIZE as u64).unwrap(); // keep only page 1
        file.sync_all().unwrap();
        drop(file);

        let result = pager.acquire(3);
        assert!(
            matches!(result, Err(PagerError::Io(_))),
            "expected an I/O error, got {result:?}"
        );
    }

    // ── Defensive fallbacks (inconsistent internal state) ────────────────

    #[test]
    fn write_access_tolerates_missing_journal_defensively() {
        let (mut pager, _f) = new_pager();
        pager.begin_write().unwrap();
        pager.journal = None; // simulate an inconsistent internal state
        let buf = pager.write_access(1).unwrap();
        buf[0..4].copy_from_slice(b"NOJN");
        assert_eq!(&pager.cache[&1].data[0..4], b"NOJN");
        assert!(pager.cache[&1].dirty);
    }

    #[test]
    fn commit_phase_one_tolerates_missing_journal_defensively() {
        let (mut pager, f) = new_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0..4].copy_from_slice(b"NOJ2");
        pager.journal = None; // simulate an inconsistent internal state
        pager.commit_phase_one(None).unwrap();
        assert_eq!(pager.state(), PagerState::WriterLocked);
        pager.commit_phase_two().unwrap();

        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        assert_eq!(&p2.acquire(1).unwrap()[0..4], b"NOJ2");
    }

    #[test]
    fn commit_journal_mode_skips_dirty_order_entries_that_are_not_dirty() {
        let (mut pager, f) = new_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 1;
        pager.write_access(2).unwrap()[0] = 2;
        // Simulate an inconsistent internal state: page 2 is still listed in
        // dirty_order, but its cache entry no longer reports itself dirty.
        pager.cache.get_mut(&2).unwrap().dirty = false;
        pager.commit().unwrap();

        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        assert_eq!(p2.acquire(1).unwrap()[0], 1);
        // Page 2 was skipped by commit, so the file was never extended to
        // cover it and re-opening only sees a 1-page database.
        assert_eq!(p2.db_size(), 1);
        assert!(p2.acquire(2).is_err());
    }

    #[test]
    fn commit_wal_mode_skips_dirty_order_entries_that_are_not_dirty() {
        let (mut pager, _f) = new_wal_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 1;
        pager.write_access(2).unwrap()[0] = 2;
        pager.cache.get_mut(&2).unwrap().dirty = false;
        pager.commit().unwrap();
        pager.cache.clear();

        assert_eq!(pager.acquire(1).unwrap()[0], 1);
        // Page 2's frame was never appended to the WAL and the main db file
        // has no data for it either, so reading it must error rather than
        // silently returning bogus data or panicking.
        assert!(pager.acquire(2).is_err());
    }

    #[test]
    fn commit_journal_mode_skips_dirty_order_entries_missing_from_cache() {
        let (mut pager, f) = new_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 1;
        pager.write_access(2).unwrap()[0] = 2;
        // Simulate a more extreme inconsistent internal state than a
        // dirty=false entry: page 2 is listed in dirty_order but has no
        // cache entry at all.
        pager.cache.remove(&2);
        pager.commit().unwrap();

        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        assert_eq!(p2.acquire(1).unwrap()[0], 1);
        assert_eq!(p2.db_size(), 1);
        assert!(p2.acquire(2).is_err());
    }

    #[test]
    fn commit_wal_mode_skips_dirty_order_entries_missing_from_cache() {
        let (mut pager, _f) = new_wal_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 1;
        pager.write_access(2).unwrap()[0] = 2;
        pager.cache.remove(&2);
        pager.commit().unwrap();
        pager.cache.clear();

        assert_eq!(pager.acquire(1).unwrap()[0], 1);
        assert!(pager.acquire(2).is_err());
    }

    #[test]
    fn read_journal_records_propagates_non_eof_read_errors() {
        // A write-only handle rejects reads with a genuine OS error, not
        // UnexpectedEof, exercising the "some other read error" arm that a
        // merely-truncated/empty journal (which fails with UnexpectedEof)
        // can't reach.
        let f = NamedTempFile::new().unwrap();
        let mut wfile = OpenOptions::new().write(true).open(f.path()).unwrap();
        let result = read_journal_records(&mut wfile, DEFAULT_PAGE_SIZE as usize);
        assert!(result.is_err());
        assert_ne!(
            result.unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof,
            "expected a non-EOF read error from a write-only handle"
        );
    }

    #[test]
    fn rollback_tolerates_missing_journal_defensively() {
        let (mut pager, _f) = new_pager();
        pager.begin_write().unwrap();
        pager.write_access(1).unwrap()[0] = 9;
        pager.journal = None; // simulate an inconsistent internal state
        pager.rollback().unwrap();
        assert_eq!(pager.state(), PagerState::Reader);
        assert_eq!(pager.db_size(), 0);
    }

    // ── Database header maintenance ───────────────────────────────────────

    #[test]
    fn commit_with_no_writes_skips_header_update() {
        let (mut pager, _f) = new_pager();
        pager.begin_write().unwrap();
        pager.commit().unwrap();
        assert_eq!(pager.db_size(), 0);
        assert_eq!(pager.state(), PagerState::Reader);
    }

    #[test]
    fn commit_updates_sqlite_header_change_counter_and_size() {
        let (mut pager, f) = new_pager();
        pager.begin_write().unwrap();
        {
            let page1 = pager.write_access(1).unwrap();
            page1[0..16].copy_from_slice(DB_MAGIC);
            // Page size field (bytes 16..18), so re-opening the file parses
            // the header correctly instead of defaulting to page_size == 0.
            page1[16..18].copy_from_slice(&DEFAULT_PAGE_SIZE.to_be_bytes());
        }
        pager.commit().unwrap();

        let mut p2 = Pager::open(f.path(), DEFAULT_PAGE_SIZE, false).unwrap();
        let page1 = p2.acquire(1).unwrap();
        let change_counter = u32::from_be_bytes(page1[24..28].try_into().unwrap());
        let db_size_in_header = u32::from_be_bytes(page1[28..32].try_into().unwrap());
        assert_eq!(change_counter, 1);
        assert_eq!(db_size_in_header, 1);
    }
}
