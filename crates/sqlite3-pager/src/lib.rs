//! Page cache, WAL (Write-Ahead Log), and rollback journal.
//!
//! Mirrors `pager.c` and `wal.c` from the C SQLite source. This is the most
//! complex module in the storage engine (~6,000 LOC in C).
//!
//! ## Status
//! Phase 2 — stub skeleton with type definitions only.

use std::sync::Arc;

/// A page number within a database file (1-based; page 0 is invalid).
pub type PageNumber = u32;

/// The minimum and maximum allowed page sizes (powers of two).
pub const MIN_PAGE_SIZE: u16 = 512;
pub const MAX_PAGE_SIZE: u32 = 65536;
pub const DEFAULT_PAGE_SIZE: u16 = 4096;

/// WAL checkpoint modes (SQLITE_CHECKPOINT_*).
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
    #[error("cannot rollback — no active write transaction")]
    NotInTransaction,
    #[error("page number out of range: {0}")]
    PageOutOfRange(u32),
}

pub type PagerResult<T> = Result<T, PagerError>;

/// A reference-counted handle to a cached page.
///
/// The inner `Vec<u8>` is page_size bytes long.
pub type PageRef = Arc<Vec<u8>>;

/// Pager state machine states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PagerState {
    Unlocked,
    Reader,
    Writer,
    WriterLocked,
    WriterCacheStress,
    WriterDbLocked,
    WriterFinished,
    Error,
}

/// The pager — manages the page cache, locking, and durability for one
/// database connection.
///
/// All fields are stubs; implementation is Phase 2.
pub struct Pager {
    page_size: u16,
    state: PagerState,
    read_only: bool,
    // page_cache, wal, journal, ... — to be filled in Phase 2.
}

impl Pager {
    /// Open a new pager for the given VFS file.
    pub fn open(page_size: u16, read_only: bool) -> PagerResult<Self> {
        if !page_size.is_power_of_two()
            || page_size < MIN_PAGE_SIZE
            || page_size > MAX_PAGE_SIZE as u16
        {
            return Err(PagerError::Corrupt);
        }
        Ok(Self {
            page_size,
            state: PagerState::Unlocked,
            read_only,
        })
    }

    pub fn page_size(&self) -> u16 {
        self.page_size
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Acquire a page by number, loading it from disk if not cached.
    pub fn acquire(&mut self, _pgno: PageNumber) -> PagerResult<PageRef> {
        todo!("Phase 2: implement page acquisition")
    }

    /// Look up a cached page without triggering a disk read.
    pub fn lookup(&self, _pgno: PageNumber) -> Option<PageRef> {
        todo!("Phase 2: implement page lookup")
    }

    /// Begin a write transaction.
    pub fn begin_write(&mut self) -> PagerResult<()> {
        todo!("Phase 2: implement begin_write")
    }

    /// Phase-1 commit: write the super-journal path.
    pub fn commit_phase_one(&mut self, _super_journal: Option<&std::path::Path>) -> PagerResult<()> {
        todo!("Phase 2: implement commit_phase_one")
    }

    /// Phase-2 commit: make the transaction durable.
    pub fn commit_phase_two(&mut self) -> PagerResult<()> {
        todo!("Phase 2: implement commit_phase_two")
    }

    /// Roll back the current write transaction.
    pub fn rollback(&mut self) -> PagerResult<()> {
        todo!("Phase 2: implement rollback")
    }

    /// WAL checkpoint.
    pub fn checkpoint(&mut self, _mode: CheckpointMode) -> PagerResult<(u32, u32)> {
        todo!("Phase 2: implement checkpoint")
    }
}
