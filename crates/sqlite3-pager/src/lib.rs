//! Page cache, rollback journal, and WAL for SQLite3-rs.
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
pub enum CheckpointMode { Passive, Full, Restart, Truncate }

/// Pager error type.
#[derive(Debug, thiserror::Error)]
pub enum PagerError {
    #[error("I/O error: {0}")]        Io(#[from] std::io::Error),
    #[error("database is corrupt")]   Corrupt,
    #[error("out of memory")]         NoMem,
    #[error("database is busy")]      Busy,
    #[error("not in a write transaction")] NotInTransaction,
    #[error("database is read-only")] ReadOnly,
    #[error("page {0} out of range")] PageOutOfRange(u32),
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

// ── Pager ─────────────────────────────────────────────────────────────────────

/// The page-level I/O manager for one database connection.
pub struct Pager {
    file: File,
    path: PathBuf,
    journal: Option<RollbackJournal>,
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
            OpenOptions::new().read(true).write(true).create(true).open(path)?
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
                    let ndb = if ndb == 0 { (file_size / ps as u64) as u32 } else { ndb };
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

    pub fn db_size(&self) -> u32 { self.db_size }
    pub fn page_size(&self) -> u16 { self.page_size }
    pub fn state(&self) -> PagerState { self.state }
    pub fn path(&self) -> &Path { &self.path }
    pub fn is_read_only(&self) -> bool { self.read_only }
    pub fn set_cache_size(&mut self, n: usize) { self.cache_size = n; }

    /// Set a new page size. Only valid on an empty (zero-page) database before
    /// any writes.
    pub fn set_page_size(&mut self, size: u16) -> PagerResult<()> {
        if self.db_size != 0 { return Err(PagerError::Corrupt); }
        if !size.is_power_of_two() || size < MIN_PAGE_SIZE { return Err(PagerError::Corrupt); }
        self.page_size = size;
        Ok(())
    }

    // ── Page access ───────────────────────────────────────────────────────────

    /// Acquire a page for **reading**. Loads from disk if not cached.
    pub fn acquire(&mut self, pgno: PageNumber) -> PagerResult<PageRef> {
        if pgno == 0 { return Err(PagerError::PageOutOfRange(0)); }
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
        if pgno == 0 { return Err(PagerError::PageOutOfRange(0)); }

        if pgno <= self.db_size {
            self.ensure_cached(pgno)?;
        } else {
            self.cache.entry(pgno).or_insert_with(|| CachedPage {
                data: vec![0u8; self.page_size as usize],
                dirty: false,
            });
        }

        if !self.cache[&pgno].dirty {
            self.journal_page(pgno)?;
            let p = self.cache.get_mut(&pgno).unwrap();
            p.dirty = true;
            self.dirty_order.push(pgno);
            if pgno > self.db_size { self.db_size = pgno; }
        }

        Ok(&mut self.cache.get_mut(&pgno).unwrap().data)
    }

    // ── Transactions ──────────────────────────────────────────────────────────

    /// Begin a write transaction: creates the rollback journal file.
    pub fn begin_write(&mut self) -> PagerResult<()> {
        if self.read_only { return Err(PagerError::ReadOnly); }
        if matches!(self.state, PagerState::Writer | PagerState::WriterLocked) {
            return Ok(());
        }

        let jpath = journal_path(&self.path);
        let mut jfile = OpenOptions::new()
            .read(true).write(true).create(true).truncate(true)
            .open(&jpath)?;

        let nonce = random_u32();
        let db_size = self.db_size;

        let mut hdr = [0u8; 28];
        hdr[0..8].copy_from_slice(&JOURNAL_MAGIC);
        hdr[8..12].copy_from_slice(&u32::MAX.to_be_bytes());          // n_records = unknown
        hdr[12..16].copy_from_slice(&nonce.to_be_bytes());
        hdr[16..20].copy_from_slice(&db_size.to_be_bytes());
        hdr[20..24].copy_from_slice(&512u32.to_be_bytes());           // sector size
        hdr[24..28].copy_from_slice(&(self.page_size as u32).to_be_bytes());
        jfile.write_all(&hdr)?;
        jfile.flush()?;

        self.journal = Some(RollbackJournal {
            file: jfile, path: jpath, nonce,
            n_records: 0, db_size_at_start: db_size,
        });
        self.state = PagerState::Writer;
        Ok(())
    }

    /// Phase-1 commit: flush journal record count, write dirty pages, sync.
    pub fn commit_phase_one(&mut self, _super_journal: Option<&Path>) -> PagerResult<()> {
        if !matches!(self.state, PagerState::Writer | PagerState::WriterLocked) {
            return Err(PagerError::NotInTransaction);
        }

        // Seal the journal with the real record count.
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
        self.state = PagerState::WriterLocked;
        Ok(())
    }

    /// Phase-2 commit: delete the journal (makes the transaction permanent).
    pub fn commit_phase_two(&mut self) -> PagerResult<()> {
        if let Some(j) = self.journal.take() {
            drop(j.file);
            let _ = std::fs::remove_file(&j.path);
        }
        for p in self.cache.values_mut() { p.dirty = false; }
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

    /// Roll back by replaying the rollback journal.
    pub fn rollback(&mut self) -> PagerResult<()> {
        if !matches!(self.state, PagerState::Writer | PagerState::WriterLocked) {
            return Err(PagerError::NotInTransaction);
        }
        self.apply_rollback(JOURNAL_HEADER_SIZE)
    }

    // ── Savepoints ────────────────────────────────────────────────────────────

    /// Snapshot current dirty-page contents as savepoint `n`.
    pub fn open_savepoint(&mut self, _n: usize) -> PagerResult<()> {
        let dirty_pages: HashMap<PageNumber, Vec<u8>> = self.dirty_order.iter()
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
        if n < self.savepoints.len() { self.savepoints.truncate(n); }
        Ok(())
    }

    // ── WAL (stub — Phase 2 continuation) ────────────────────────────────────

    pub fn checkpoint(&mut self, _mode: CheckpointMode) -> PagerResult<(u32, u32)> {
        Ok((0, 0))
    }

    pub fn move_page(&mut self, _page: PageRef, _new_pgno: PageNumber) -> PagerResult<()> {
        Err(PagerError::NotInTransaction) // auto-vacuum — Phase 2 cont.
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn ensure_cached(&mut self, pgno: PageNumber) -> PagerResult<()> {
        if self.cache.contains_key(&pgno) { return Ok(()); }
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
        if self.cache.len() > self.cache_size { self.evict_clean(pgno); }
        Ok(())
    }

    fn journal_page(&mut self, pgno: PageNumber) -> PagerResult<()> {
        let data = self.cache.get(&pgno)
            .map(|p| p.data.clone())
            .unwrap_or_else(|| vec![0u8; self.page_size as usize]);

        let Some(j) = &mut self.journal else { return Ok(()); };
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

        let saved_db_size = self.journal.as_ref().map(|j| j.db_size_at_start).unwrap_or(0);

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
        if self.db_size < 1 { return Ok(()); }
        self.ensure_cached(1)?;
        if let Some(p) = self.cache.get_mut(&1) {
            if p.data.len() >= 100 && &p.data[0..16] == DB_MAGIC {
                // Change counter (bytes 24-27): increment.
                let cc = u32::from_be_bytes(p.data[24..28].try_into().unwrap());
                p.data[24..28].copy_from_slice(&cc.wrapping_add(1).to_be_bytes());
                // Database size (bytes 28-31).
                p.data[28..32].copy_from_slice(&self.db_size.to_be_bytes());
                let data = p.data.clone();
                pwrite(&mut self.file, 0, &data)?;
            }
        }
        Ok(())
    }

    fn evict_clean(&mut self, keep: PageNumber) {
        if let Some(v) = self.cache.iter()
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
    while i > 0 { c = c.wrapping_add(data[i as usize] as u32); i -= 200; }
    c
}

fn journal_path(db: &Path) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push("-journal");
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
}

