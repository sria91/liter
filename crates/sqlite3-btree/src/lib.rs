//! B-tree read/write/cursor operations for SQLite3-rs.
//!
//! Mirrors `btree.c` / `btreeInt.h`. Implements both table B-trees (row-id
//! keyed) and index B-trees (arbitrary key).
//!
//! ## Status
//! Phase 2 — stub skeleton with type/trait definitions only.

use sqlite3_pager::PageNumber;

/// Maximum depth of the B-tree cursor page stack.
/// SQLite uses 20 (BTCURSOR_MAX_DEPTH in btreeInt.h).
const MAX_DEPTH: usize = 20;

/// Seek bias when moving a cursor to a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekBias {
    /// Position on the exact key, or the first key greater-than if not found.
    Ge,
    /// Position on the first key strictly greater than the given key.
    Gt,
}

/// Result of a cursor `move_to` operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekResult {
    /// Exact key found.
    Equal,
    /// Cursor is positioned before the key (key is greater than cursor entry).
    Less,
    /// Cursor is positioned after the key (key is less than cursor entry).
    Greater,
    /// Table/index is empty.
    Empty,
}

/// State of a B-tree cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorState {
    Invalid,
    Valid,
    Fault,
}

/// B-tree error type.
#[derive(Debug, thiserror::Error)]
pub enum BTreeError {
    #[error("pager error: {0}")]
    Pager(#[from] sqlite3_pager::PagerError),
    #[error("database is corrupt")]
    Corrupt,
    #[error("cursor is not valid")]
    InvalidCursor,
    #[error("key not found")]
    NotFound,
    #[error("duplicate key")]
    DuplicateKey,
}

pub type BTreeResult<T> = Result<T, BTreeError>;

/// A single B-tree (table or index) backed by the pager.
pub struct BTree {
    // pager: Arc<Mutex<Pager>>,  — Phase 2
    meta: [u32; 16],
}

impl BTree {
    /// Create a new in-memory B-tree for testing.
    pub fn new_in_memory() -> Self {
        Self { meta: [0u32; 16] }
    }

    /// Open a cursor on the given root page.
    pub fn cursor(&self, root_page: PageNumber, wrflag: bool) -> BTreeResult<BTreeCursor<'_>> {
        let _ = wrflag;
        Ok(BTreeCursor {
            btree: self,
            root_page,
            state: CursorState::Invalid,
        })
    }
}

/// A cursor for iterating over a B-tree.
pub struct BTreeCursor<'bt> {
    #[allow(dead_code)]
    btree: &'bt BTree,
    root_page: PageNumber,
    state: CursorState,
}

impl BTreeCursor<'_> {
    pub fn move_to_first(&mut self) -> BTreeResult<bool> {
        todo!("Phase 2: implement move_to_first")
    }

    pub fn move_to_last(&mut self) -> BTreeResult<bool> {
        todo!("Phase 2: implement move_to_last")
    }

    pub fn move_to(&mut self, _key: &[u8], _bias: SeekBias) -> BTreeResult<SeekResult> {
        todo!("Phase 2: implement move_to")
    }

    pub fn next(&mut self) -> BTreeResult<bool> {
        todo!("Phase 2: implement next")
    }

    pub fn previous(&mut self) -> BTreeResult<bool> {
        todo!("Phase 2: implement previous")
    }

    pub fn key(&self) -> BTreeResult<&[u8]> {
        todo!("Phase 2: implement key")
    }

    pub fn data(&self) -> BTreeResult<&[u8]> {
        todo!("Phase 2: implement data")
    }

    pub fn insert(&mut self, _key: &[u8], _data: &[u8], _append: bool) -> BTreeResult<()> {
        todo!("Phase 2: implement insert")
    }

    pub fn delete(&mut self) -> BTreeResult<()> {
        todo!("Phase 2: implement delete")
    }

    pub fn is_valid(&self) -> bool {
        self.state == CursorState::Valid
    }

    pub fn root_page(&self) -> PageNumber {
        self.root_page
    }
}
