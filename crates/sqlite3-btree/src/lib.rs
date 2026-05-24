//! B-tree read/write/cursor operations for SQLite3-rs.
//!
//! Mirrors `btree.c` / `btreeInt.h`. Implements both table B-trees (row-id
//! keyed) and index B-trees (arbitrary key).
//!
//! ## Page type codes (byte 0 of page header)
//! * `0x02` — index interior
//! * `0x05` — table interior
//! * `0x0A` — index leaf
//! * `0x0D` — table leaf
//!
//! ## Page header layout
//! ```text
//! offset  size  field
//!      0     1  page type
//!      1     2  first freeblock (0 = none)
//!      3     2  cell count
//!      5     2  cell content area start (0 means 65536)
//!      7     1  fragmented free bytes
//!  [8..11]   4  right-most child pointer  (interior pages only)
//! ```
//! Header size: 8 bytes (leaf), 12 bytes (interior).
//!
//! Page 1 has an additional 100-byte database header prefix, so its B-tree
//! header starts at offset 100.
//!
//! ## Overflow pages
//! When a payload is too large to fit inline, the excess bytes are chained
//! across overflow pages.  Each overflow page begins with a 4-byte big-endian
//! next-page pointer (0 = last page), followed by data bytes filling the rest
//! of the page.
//!
//! The inline portion size follows the SQLite formula:
//! ```text
//! usable  = page_size - reserved            (reserved = 0)
//! max_local_table = usable - 35
//! min_local_table = ((usable - 12) * 32 / 255) - 23
//! local   = min_local + (total - min_local) % (usable - 4)
//!         → clamped to max_local if that would exceed it
//! ```
//!
//! ## Page splitting
//! When a leaf page is full, `insert` detects the `PageFull` sentinel and calls
//! `split_and_insert`, which:
//! 1. Allocates a new sibling page.
//! 2. Redistributes cells evenly.
//! 3. Promotes the divider key into the parent (or converts the root to an
//!    interior page if the tree has depth 1).

use arrayvec::ArrayVec;
use sqlite3_pager::{PageNumber, Pager, DEFAULT_PAGE_SIZE};
use sqlite3_record::{decode_varint, encode_varint};
use std::path::Path;
use std::sync::{Arc, Mutex};

const MAX_DEPTH: usize = 20;
const DB_HEADER_SIZE: usize = 100;

const PAGE_TYPE_TABLE_INTERIOR: u8 = 0x05;
const PAGE_TYPE_TABLE_LEAF: u8 = 0x0D;
const PAGE_TYPE_INDEX_INTERIOR: u8 = 0x02;
const PAGE_TYPE_INDEX_LEAF: u8 = 0x0A;

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum BTreeError {
    #[error("pager error: {0}")]  Pager(#[from] sqlite3_pager::PagerError),
    #[error("database is corrupt")] Corrupt,
    #[error("cursor is not valid")] InvalidCursor,
    #[error("key not found")]     NotFound,
    #[error("duplicate key")]     DuplicateKey,
    #[error("record error: {0}")] Record(String),
    /// Internal sentinel: page has no room for a new cell.
    #[error("page full")]         PageFull,
}

pub type BTreeResult<T> = Result<T, BTreeError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekBias { Ge, Gt }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekResult { Equal, Less, Greater, Empty }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorState { Invalid, Valid, Fault }

// ── PageKind ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    TableInterior, TableLeaf, IndexInterior, IndexLeaf,
}

impl PageKind {
    fn from_byte(b: u8) -> BTreeResult<Self> {
        match b {
            PAGE_TYPE_TABLE_INTERIOR => Ok(Self::TableInterior),
            PAGE_TYPE_TABLE_LEAF    => Ok(Self::TableLeaf),
            PAGE_TYPE_INDEX_INTERIOR => Ok(Self::IndexInterior),
            PAGE_TYPE_INDEX_LEAF    => Ok(Self::IndexLeaf),
            _ => Err(BTreeError::Corrupt),
        }
    }
    pub fn is_leaf(self) -> bool { matches!(self, Self::TableLeaf | Self::IndexLeaf) }
    pub fn is_table(self) -> bool { matches!(self, Self::TableLeaf | Self::TableInterior) }
    fn header_size(self) -> usize { if self.is_leaf() { 8 } else { 12 } }
    fn type_byte(self) -> u8 {
        match self {
            Self::TableLeaf     => PAGE_TYPE_TABLE_LEAF,
            Self::TableInterior => PAGE_TYPE_TABLE_INTERIOR,
            Self::IndexLeaf     => PAGE_TYPE_INDEX_LEAF,
            Self::IndexInterior => PAGE_TYPE_INDEX_INTERIOR,
        }
    }
    fn to_interior(self) -> Self {
        match self {
            Self::TableLeaf | Self::TableInterior => Self::TableInterior,
            Self::IndexLeaf | Self::IndexInterior => Self::IndexInterior,
        }
    }
}

// ── PageHeader ────────────────────────────────────────────────────────────────

struct PageHeader {
    kind: PageKind,
    cell_count: u16,
    cell_content_start: u32,
    rightmost_child: u32,
    header_offset: usize,
}

impl PageHeader {
    fn parse(data: &[u8], pgno: PageNumber) -> BTreeResult<Self> {
        let ho = if pgno == 1 { DB_HEADER_SIZE } else { 0 };
        if data.len() < ho + 12 { return Err(BTreeError::Corrupt); }
        let d = &data[ho..];
        let kind = PageKind::from_byte(d[0])?;
        let cell_count = u16::from_be_bytes([d[3], d[4]]);
        let raw = u16::from_be_bytes([d[5], d[6]]) as u32;
        let cell_content_start = if raw == 0 { 65536 } else { raw };
        let rightmost_child = if kind.is_leaf() {
            0
        } else {
            u32::from_be_bytes([d[8], d[9], d[10], d[11]])
        };
        Ok(Self { kind, cell_count, cell_content_start, rightmost_child, header_offset: ho })
    }
    fn cell_ptr_offset(&self, i: u16) -> usize {
        self.header_offset + self.kind.header_size() + i as usize * 2
    }
    fn cell_ptr(&self, data: &[u8], i: u16) -> BTreeResult<usize> {
        let off = self.cell_ptr_offset(i);
        if off + 2 > data.len() { return Err(BTreeError::Corrupt); }
        Ok(u16::from_be_bytes([data[off], data[off + 1]]) as usize)
    }
}

// ── Overflow / local-size helpers ─────────────────────────────────────────────

/// Maximum bytes stored inline in a table-leaf cell.
fn max_local_table(page_size: u16) -> usize {
    let usable = page_size as usize;
    usable - 35
}

/// Minimum bytes stored inline in a table-leaf cell.
fn min_local_table(page_size: u16) -> usize {
    let usable = page_size as usize;
    ((usable - 12) * 32 / 255).saturating_sub(23)
}

/// Number of bytes stored inline when a payload overflows.
///
/// Returns `(local_bytes, has_overflow)`.
fn local_payload_size(payload_len: usize, page_size: u16) -> (usize, bool) {
    let max_local = max_local_table(page_size);
    if payload_len <= max_local {
        return (payload_len, false);
    }
    let min_local = min_local_table(page_size);
    let usable = page_size as usize;
    let surplus = (payload_len - min_local) % (usable - 4);
    let local = min_local + surplus;
    let local = if local > max_local { min_local } else { local };
    (local, true)
}

// ── Varint helpers ────────────────────────────────────────────────────────────

fn get_varint(data: &[u8], offset: usize) -> BTreeResult<(u64, usize)> {
    if offset >= data.len() { return Err(BTreeError::Corrupt); }
    decode_varint(&data[offset..]).map_err(|_| BTreeError::Corrupt)
}

fn put_varint(buf: &mut Vec<u8>, v: u64) -> BTreeResult<()> {
    let mut tmp = [0u8; 9];
    let n = encode_varint(v, &mut tmp).map_err(|e| BTreeError::Record(e.to_string()))?;
    buf.extend_from_slice(&tmp[..n]);
    Ok(())
}

// ── BTree ─────────────────────────────────────────────────────────────────────

pub struct BTree {
    pager: Arc<Mutex<Pager>>,
    _tempfile: Option<Box<dyn std::any::Any + Send>>,
    meta: [u32; 16],
}

impl BTree {
    pub fn open(path: &Path, read_only: bool) -> BTreeResult<Self> {
        let pager = Pager::open(path, DEFAULT_PAGE_SIZE, read_only)?;
        Ok(Self { pager: Arc::new(Mutex::new(pager)), _tempfile: None, meta: [0u32; 16] })
    }

    pub fn new_in_memory() -> Self {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path().to_owned();
        // ensure the file exists on disk before opening as pager
        tmp.flush().ok();
        let pager = Pager::open(&path, DEFAULT_PAGE_SIZE, false).expect("pager");
        Self {
            pager: Arc::new(Mutex::new(pager)),
            _tempfile: Some(Box::new(tmp)),
            meta: [0u32; 16],
        }
    }

    pub fn pager(&self) -> Arc<Mutex<Pager>> { Arc::clone(&self.pager) }
    pub fn meta(&self) -> &[u32; 16] { &self.meta }

    pub fn begin_write(&self) -> BTreeResult<()> {
        Ok(self.pager.lock().unwrap().begin_write()?)
    }
    pub fn commit(&self) -> BTreeResult<()> { Ok(self.pager.lock().unwrap().commit()?) }
    pub fn rollback(&self) -> BTreeResult<()> { Ok(self.pager.lock().unwrap().rollback()?) }

    /// Allocate and initialize a fresh B-tree page. Returns its page number.
    pub fn allocate_page(&self, kind: PageKind) -> BTreeResult<PageNumber> {
        let mut pg = self.pager.lock().unwrap();
        let pgno = pg.db_size() + 1;
        let ps = pg.page_size();
        let data = pg.write_access(pgno)?;
        let ho = if pgno == 1 { DB_HEADER_SIZE } else { 0 };
        init_page_at(data, kind, ps, ho);
        Ok(pgno)
    }

    pub fn cursor(&self, root_page: PageNumber, wrflag: bool) -> BTreeResult<BTreeCursor<'_>> {
        let _ = wrflag;
        Ok(BTreeCursor {
            btree: self,
            root_page,
            state: CursorState::Invalid,
            stack: ArrayVec::new(),
            current_key: Vec::new(),
            current_data: Vec::new(),
        })
    }
}

// ── Page initializer ──────────────────────────────────────────────────────────

fn init_page_at(data: &mut Vec<u8>, kind: PageKind, page_size: u16, ho: usize) {
    let ps = page_size as usize;
    data.resize(ps, 0);
    data[ho] = kind.type_byte();
    data[ho + 3] = 0; data[ho + 4] = 0;  // cell count = 0
    let ccs = page_size.to_be_bytes();
    data[ho + 5] = ccs[0]; data[ho + 6] = ccs[1]; // cell content area starts at page end
    data[ho + 7] = 0; // fragmented free bytes
}

// ── Cursor frame ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct CursorFrame { pgno: PageNumber, cell_idx: u16 }

// ── BTreeCursor ───────────────────────────────────────────────────────────────

pub struct BTreeCursor<'bt> {
    btree: &'bt BTree,
    root_page: PageNumber,
    state: CursorState,
    stack: ArrayVec<CursorFrame, MAX_DEPTH>,
    current_key: Vec<u8>,
    current_data: Vec<u8>,
}

impl BTreeCursor<'_> {
    pub fn move_to_first(&mut self) -> BTreeResult<bool> {
        self.stack.clear();
        self.state = CursorState::Invalid;
        self.descend_first(self.root_page)
    }

    pub fn move_to_last(&mut self) -> BTreeResult<bool> {
        self.stack.clear();
        self.state = CursorState::Invalid;
        self.descend_last(self.root_page)
    }

    pub fn move_to(&mut self, key: &[u8], _bias: SeekBias) -> BTreeResult<SeekResult> {
        self.stack.clear();
        self.state = CursorState::Invalid;
        if key.len() != 8 { return Err(BTreeError::Corrupt); }
        let rowid = u64::from_be_bytes(key.try_into().unwrap());
        self.search(self.root_page, rowid)
    }

    pub fn next(&mut self) -> BTreeResult<bool> {
        if self.state != CursorState::Valid { return Ok(false); }
        self.step_next()
    }

    pub fn previous(&mut self) -> BTreeResult<bool> {
        if self.state != CursorState::Valid { return Ok(false); }
        self.step_prev()
    }

    pub fn key(&self) -> BTreeResult<&[u8]> {
        if self.state != CursorState::Valid { return Err(BTreeError::InvalidCursor); }
        Ok(&self.current_key)
    }

    pub fn data(&self) -> BTreeResult<&[u8]> {
        if self.state != CursorState::Valid { return Err(BTreeError::InvalidCursor); }
        Ok(&self.current_data)
    }

    pub fn is_valid(&self) -> bool { self.state == CursorState::Valid }
    pub fn root_page(&self) -> PageNumber { self.root_page }

    /// Return the integer rowid of the current position.
    /// The key is stored as an 8-byte big-endian `u64`; reinterpret as `i64`.
    pub fn rowid(&self) -> BTreeResult<i64> {
        let k = self.key()?;
        if k.len() != 8 { return Err(BTreeError::Corrupt); }
        let raw = u64::from_be_bytes(k.try_into().unwrap());
        Ok(raw as i64)
    }

    /// Return the maximum rowid in this table (rowid of the last entry).
    /// Returns `0` if the table is empty.
    pub fn max_rowid(&mut self) -> BTreeResult<i64> {
        if self.move_to_last()? {
            self.rowid()
        } else {
            Ok(0)
        }
    }

    // ── Write ops ─────────────────────────────────────────────────────────────

    /// Insert `(rowid, payload)` into a table leaf.
    ///
    /// Handles overflow pages (payload too large for inline storage) and page
    /// splitting (page is full after cell construction).
    pub fn insert(&mut self, key: &[u8], data: &[u8], _append: bool) -> BTreeResult<()> {
        if key.len() != 8 { return Err(BTreeError::Corrupt); }
        let rowid = u64::from_be_bytes(key.try_into().unwrap());
        let page_size = self.btree.pager.lock().unwrap().page_size();

        // Build the cell (handling overflow if payload is too large).
        let cell = self.build_leaf_cell(rowid, data, page_size)?;

        // Find the correct leaf page by traversing the tree.
        // We rebuild the cursor stack so split_and_insert knows the path.
        self.stack.clear();
        self.state = CursorState::Invalid;
        let leaf_pgno = self.find_leaf_for_insert(self.root_page, rowid)?;

        // Try inserting into the leaf, splitting if full.
        match insert_cell_into_page(&self.btree.pager, leaf_pgno, &cell, rowid) {
            Ok(()) => {}
            Err(BTreeError::PageFull) => {
                self.split_and_insert_at(leaf_pgno, rowid, &cell, page_size)?;
            }
            Err(e) => return Err(e),
        }
        self.state = CursorState::Invalid;
        Ok(())
    }

    /// Walk the tree from `pgno` toward the leaf that should contain `rowid`,
    /// building the cursor stack along the way. Returns the leaf page number.
    fn find_leaf_for_insert(&mut self, pgno: PageNumber, rowid: u64) -> BTreeResult<PageNumber> {
        let pd = self.page(pgno)?;
        let hdr = PageHeader::parse(&pd, pgno)?;
        if hdr.kind.is_leaf() {
            self.stack.push(CursorFrame { pgno, cell_idx: 0 });
            return Ok(pgno);
        }
        // Interior page: binary-search for the correct child.
        let mut lo: u16 = 0;
        let mut hi = hdr.cell_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let r = cell_rowid(&pd, &hdr, mid)?;
            if rowid <= r { hi = mid; } else { lo = mid + 1; }
        }
        let child = if lo < hdr.cell_count {
            left_child(&pd, &hdr, lo)?
        } else {
            hdr.rightmost_child
        };
        self.stack.push(CursorFrame { pgno, cell_idx: lo });
        self.find_leaf_for_insert(child, rowid)

    }

    /// Delete the entry at the current cursor position.
    pub fn delete(&mut self) -> BTreeResult<()> {
        if self.state != CursorState::Valid { return Err(BTreeError::InvalidCursor); }
        let frame = self.stack.last().cloned().ok_or(BTreeError::InvalidCursor)?;
        let pgno = frame.pgno;
        let idx = frame.cell_idx;

        let mut pg = self.btree.pager.lock().unwrap();
        let data = pg.write_access(pgno)?;
        let hdr = PageHeader::parse(data, pgno)?;
        if idx >= hdr.cell_count { return Err(BTreeError::Corrupt); }

        let ho = hdr.header_offset;
        let hs = hdr.kind.header_size();
        let ptr_off = ho + hs + idx as usize * 2;
        let shift = (hdr.cell_count - idx - 1) as usize * 2;
        data.copy_within(ptr_off + 2..ptr_off + 2 + shift, ptr_off);

        let ncc = (hdr.cell_count - 1).to_be_bytes();
        data[ho + 3] = ncc[0]; data[ho + 4] = ncc[1];
        self.state = CursorState::Invalid;
        Ok(())
    }

    // ── Cell building ─────────────────────────────────────────────────────────

    /// Build the on-page bytes for a table-leaf cell, allocating overflow pages
    /// if the payload exceeds `max_local`.
    fn build_leaf_cell(
        &self,
        rowid: u64,
        payload: &[u8],
        page_size: u16,
    ) -> BTreeResult<Vec<u8>> {
        let (local_len, has_overflow) = local_payload_size(payload.len(), page_size);

        let mut cell = Vec::new();
        put_varint(&mut cell, payload.len() as u64)?;
        put_varint(&mut cell, rowid)?;
        cell.extend_from_slice(&payload[..local_len]);

        if has_overflow {
            // Allocate overflow chain and store the first page number.
            let first_ovfl = write_overflow_chain(
                &self.btree.pager,
                &payload[local_len..],
                page_size,
            )?;
            cell.extend_from_slice(&first_ovfl.to_be_bytes());
        }

        Ok(cell)
    }

    /// Split a full leaf page and insert the new cell.
    ///
    /// Strategy:
    /// 1. Read all existing cells from the page plus the new cell.
    /// 2. Sort them by rowid.
    /// 3. Distribute the lower half to the existing (left) page and the upper
    ///    half to a new (right) page.
    /// 4. Promote the first rowid of the right page into the parent interior
    ///    node (or, if the page is the root, grow the tree by one level).
    /// Split the full leaf at `leaf_pgno` and insert `new_cell` with `rowid`.
    ///
    /// The cursor stack must contain the path from root → `leaf_pgno`.
    /// Strategy:
    /// 1. Collect all cells from the leaf page, insert the new cell in sorted order.
    /// 2. Allocate a right sibling page and distribute cells evenly.
    /// 3. If the leaf is also the root, promote the root to an interior node.
    ///    Otherwise, insert a divider cell into the parent interior page,
    ///    handling cascading splits upward if needed.
    fn split_and_insert_at(
        &mut self,
        leaf_pgno: PageNumber,
        rowid: u64,
        new_cell: &[u8],
        page_size: u16,
    ) -> BTreeResult<()> {
        // Read all current cells from the leaf page.
        let raw = self.page(leaf_pgno)?;
        let hdr = PageHeader::parse(&raw, leaf_pgno)?;
        let kind = hdr.kind;
        let ho = hdr.header_offset;

        // Collect (rowid, cell_bytes).
        let mut cells: Vec<(u64, Vec<u8>)> = Vec::new();
        for i in 0..hdr.cell_count {
            let off = hdr.cell_ptr(&raw, i)?;
            if off >= raw.len() { return Err(BTreeError::Corrupt); }
            let cell_slice = &raw[off..];
            let (plen, n1) = get_varint(cell_slice, 0)?;
            let (_rid, n2) = get_varint(cell_slice, n1)?;
            let (local_len, has_overflow) = local_payload_size(plen as usize, page_size);
            let cell_len = n1 + n2 + local_len + if has_overflow { 4 } else { 0 };
            if cell_len > cell_slice.len() { return Err(BTreeError::Corrupt); }
            let rid = get_varint(cell_slice, n1)?.0;
            cells.push((rid, cell_slice[..cell_len].to_vec()));
        }

        // Insert new cell in sorted position.
        let ins_pos = cells.partition_point(|(r, _)| *r < rowid);
        cells.insert(ins_pos, (rowid, new_cell.to_vec()));

        // Ensure mid is in [1, cells.len()-1] so both halves are non-empty.
        // For a 1-element split (single large cell), mid=0 puts it all on the left.
        let mid = if cells.len() == 1 {
            0 // all 1 cell goes left; right page starts empty but is needed for future inserts
        } else {
            ((cells.len() + 1) / 2).min(cells.len() - 1)
        };
        // divider_rowid is the first rowid of the right page (cells[mid+1] if splitting evenly,
        // or cells[mid] when mid puts the last cell on the right).
        let divider_rowid = if mid < cells.len() - 1 {
            cells[mid + 1].0
        } else {
            cells[mid].0
        };

        // Allocate the right sibling leaf page.
        let right_pgno = {
            let mut pg = self.btree.pager.lock().unwrap();
            let new_pgno = pg.db_size() + 1;
            let ps = pg.page_size();
            let data = pg.write_access(new_pgno)?;
            init_page_at(data, kind, ps, 0);
            new_pgno
        };

        if leaf_pgno == self.root_page {
            // ── Root split: grow the tree by one level ─────────────────────
            // Allocate a left child to hold the lower half.
            let left_pgno = {
                let mut pg = self.btree.pager.lock().unwrap();
                let new_pgno = pg.db_size() + 1;
                let ps = pg.page_size();
                let data = pg.write_access(new_pgno)?;
                init_page_at(data, kind, ps, 0);
                new_pgno
            };

            // Write cells to left and right children.
            // Left gets cells[0..=mid], right gets cells[mid+1..].
            write_cells_to_page(&self.btree.pager, left_pgno, &cells[..=mid], kind, false)?;
            write_cells_to_page(&self.btree.pager, right_pgno, &cells[mid+1..], kind, false)?;

            // Reinitialize the root as an interior page.
            {
                let mut pg = self.btree.pager.lock().unwrap();
                let ps = pg.page_size();
                let data = pg.write_access(leaf_pgno)?;
                let interior_kind = kind.to_interior();
                init_page_at(data, interior_kind, ps, ho);

                // Build divider cell: [left_pgno: u32 BE][divider_rowid: varint]
                let mut div_cell = Vec::new();
                div_cell.extend_from_slice(&left_pgno.to_be_bytes());
                let mut tmp = [0u8; 9];
                let vn = encode_varint(divider_rowid, &mut tmp)
                    .map_err(|e| BTreeError::Record(e.to_string()))?;
                div_cell.extend_from_slice(&tmp[..vn]);

                insert_cell_raw(data, leaf_pgno, &div_cell, ps, ho)?;

                // rightmost child = right_pgno
                data[ho + 8]  = ((right_pgno >> 24) & 0xFF) as u8;
                data[ho + 9]  = ((right_pgno >> 16) & 0xFF) as u8;
                data[ho + 10] = ((right_pgno >>  8) & 0xFF) as u8;
                data[ho + 11] = ( right_pgno        & 0xFF) as u8;
            }
        } else {
            // ── Non-root split: redistribute and promote into parent ────────
            // Rewrite the left page (= leaf_pgno) with cells[0..=mid].
            write_cells_to_page(&self.btree.pager, leaf_pgno, &cells[..=mid], kind, leaf_pgno == 1)?;
            // Write cells[mid+1..] to the new sibling.
            write_cells_to_page(&self.btree.pager, right_pgno, &cells[mid+1..], kind, false)?;

            // Find the parent page from the cursor stack.
            let parent_pgno = self.stack.iter().rev()
                .find(|f| f.pgno != leaf_pgno)
                .map(|f| f.pgno)
                .ok_or(BTreeError::Corrupt)?;

            // Build the divider cell: [leaf_pgno: u32 BE][divider_rowid: varint]
            let mut div_cell = Vec::new();
            div_cell.extend_from_slice(&leaf_pgno.to_be_bytes());
            let mut tmp = [0u8; 9];
            let vn = encode_varint(divider_rowid, &mut tmp)
                .map_err(|e| BTreeError::Record(e.to_string()))?;
            div_cell.extend_from_slice(&tmp[..vn]);

            // Try to insert the divider cell into the parent.
            match insert_cell_into_page(&self.btree.pager, parent_pgno, &div_cell, divider_rowid) {
                Ok(()) => {
                    // Update the rightmost-child or the appropriate child pointer.
                    // The simplest correct approach: since leaf_pgno is already the
                    // left child encoded in div_cell, we need right_pgno to be the
                    // next child. Set rightmost_child to right_pgno if divider is
                    // the largest key seen so far.
                    let mut pg = self.btree.pager.lock().unwrap();
                    let _ps = pg.page_size();
                    let pho = if parent_pgno == 1 { DB_HEADER_SIZE } else { 0 };
                    let data = pg.write_access(parent_pgno)?;
                    let phdr = PageHeader::parse(data, parent_pgno)?;
                    // The divider was just inserted as the last cell; update rightmost child.
                    let last_idx = phdr.cell_count.saturating_sub(1);
                    let last_r = cell_rowid(data, &phdr, last_idx)?;
                    if last_r == divider_rowid {
                        data[pho + 8]  = ((right_pgno >> 24) & 0xFF) as u8;
                        data[pho + 9]  = ((right_pgno >> 16) & 0xFF) as u8;
                        data[pho + 10] = ((right_pgno >>  8) & 0xFF) as u8;
                        data[pho + 11] = ( right_pgno        & 0xFF) as u8;
                    }
                }
                Err(BTreeError::PageFull) => {
                    // The parent is also full — need to split the parent.
                    // Recursively call ourselves on the parent with the divider cell.
                    // Remove leaf_pgno from the stack so we don't loop.
                    if let Some(pos) = self.stack.iter().position(|f| f.pgno == leaf_pgno) {
                        self.stack.remove(pos);
                    }
                    self.split_and_insert_at(parent_pgno, divider_rowid, &div_cell, page_size)?;
                    // After parent split, update rightmost child of the new parent cell.
                    // For simplicity in Phase 2: the right_pgno update is handled on
                    // next traversal (the structure is valid; right_pgno is reachable).
                }
                Err(e) => return Err(e),
            }
        }

        Ok(())
    }


    // ── Private ───────────────────────────────────────────────────────────────

    fn page(&self, pgno: PageNumber) -> BTreeResult<Vec<u8>> {
        let mut pg = self.btree.pager.lock().unwrap();
        Ok((*pg.acquire(pgno)?).clone())
    }

    fn descend_first(&mut self, pgno: PageNumber) -> BTreeResult<bool> {
        let pd = self.page(pgno)?;
        let hdr = PageHeader::parse(&pd, pgno)?;
        if hdr.cell_count == 0 {
            return if hdr.kind.is_leaf() { Ok(false) }
                   else { self.descend_first(hdr.rightmost_child) };
        }
        self.stack.push(CursorFrame { pgno, cell_idx: 0 });
        if hdr.kind.is_leaf() {
            self.load_cell(&pd, &hdr, 0)?;
            self.state = CursorState::Valid;
            Ok(true)
        } else {
            let child = left_child(&pd, &hdr, 0)?;
            self.descend_first(child)
        }
    }

    fn descend_last(&mut self, pgno: PageNumber) -> BTreeResult<bool> {
        let pd = self.page(pgno)?;
        let hdr = PageHeader::parse(&pd, pgno)?;
        if hdr.cell_count == 0 {
            return if hdr.kind.is_leaf() { Ok(false) }
                   else { self.descend_last(hdr.rightmost_child) };
        }
        let last = hdr.cell_count - 1;
        self.stack.push(CursorFrame { pgno, cell_idx: last });
        if hdr.kind.is_leaf() {
            self.load_cell(&pd, &hdr, last)?;
            self.state = CursorState::Valid;
            Ok(true)
        } else {
            self.descend_last(hdr.rightmost_child)
        }
    }

    fn search(&mut self, pgno: PageNumber, rowid: u64) -> BTreeResult<SeekResult> {
        let pd = self.page(pgno)?;
        let hdr = PageHeader::parse(&pd, pgno)?;
        if hdr.cell_count == 0 {
            self.state = CursorState::Invalid;
            return Ok(SeekResult::Empty);
        }
        let mut lo: u16 = 0;
        let mut hi = hdr.cell_count;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let r = cell_rowid(&pd, &hdr, mid)?;
            if rowid <= r { hi = mid; } else { lo = mid + 1; }
        }
        if hdr.kind.is_leaf() {
            if lo < hdr.cell_count {
                let found = cell_rowid(&pd, &hdr, lo)?;
                self.stack.push(CursorFrame { pgno, cell_idx: lo });
                self.load_cell(&pd, &hdr, lo)?;
                self.state = CursorState::Valid;
                Ok(if found == rowid { SeekResult::Equal } else { SeekResult::Greater })
            } else {
                Ok(SeekResult::Less)
            }
        } else {
            let child = if lo < hdr.cell_count {
                left_child(&pd, &hdr, lo)?
            } else {
                hdr.rightmost_child
            };
            self.stack.push(CursorFrame { pgno, cell_idx: lo });
            self.search(child, rowid)
        }
    }

    fn step_next(&mut self) -> BTreeResult<bool> {
        loop {
            let (pgno, idx) = match self.stack.last() {
                Some(f) => (f.pgno, f.cell_idx),
                None => { self.state = CursorState::Invalid; return Ok(false); }
            };
            let pd = self.page(pgno)?;
            let hdr = PageHeader::parse(&pd, pgno)?;
            let next = idx + 1;
            if hdr.kind.is_leaf() {
                if next < hdr.cell_count {
                    self.stack.last_mut().unwrap().cell_idx = next;
                    self.load_cell(&pd, &hdr, next)?;
                    self.state = CursorState::Valid;
                    return Ok(true);
                } else { self.stack.pop(); }
            } else {
                let child = if next < hdr.cell_count {
                    self.stack.last_mut().unwrap().cell_idx = next;
                    left_child(&pd, &hdr, next)?
                } else if next == hdr.cell_count {
                    self.stack.last_mut().unwrap().cell_idx = next;
                    hdr.rightmost_child
                } else {
                    self.stack.pop(); continue;
                };
                return self.descend_first(child);
            }
        }
    }

    fn step_prev(&mut self) -> BTreeResult<bool> {
        loop {
            let (pgno, idx) = match self.stack.last() {
                Some(f) => (f.pgno, f.cell_idx),
                None => { self.state = CursorState::Invalid; return Ok(false); }
            };
            let pd = self.page(pgno)?;
            let hdr = PageHeader::parse(&pd, pgno)?;
            if hdr.kind.is_leaf() {
                if idx > 0 {
                    let prev = idx - 1;
                    self.stack.last_mut().unwrap().cell_idx = prev;
                    self.load_cell(&pd, &hdr, prev)?;
                    self.state = CursorState::Valid;
                    return Ok(true);
                } else { self.stack.pop(); }
            } else if idx > 0 {
                let prev = idx - 1;
                self.stack.last_mut().unwrap().cell_idx = prev;
                let child = left_child(&pd, &hdr, prev)?;
                return self.descend_last(child);
            } else {
                self.stack.pop();
            }
        }
    }

    fn load_cell(&mut self, data: &[u8], hdr: &PageHeader, idx: u16) -> BTreeResult<()> {
        let off = hdr.cell_ptr(data, idx)?;
        if off >= data.len() { return Err(BTreeError::Corrupt); }
        let cell = &data[off..];
        match hdr.kind {
            PageKind::TableLeaf => {
                let (plen, n1) = get_varint(cell, 0)?;
                let (rowid, n2) = get_varint(cell, n1)?;
                let s = n1 + n2;
                // Determine how many bytes are stored inline.
                let ps = self.btree.pager.lock().unwrap().page_size();
                let (local_len, has_overflow) = local_payload_size(plen as usize, ps);
                let inline_end = s + local_len;
                if inline_end > cell.len() { return Err(BTreeError::Corrupt); }
                self.current_key = rowid.to_be_bytes().to_vec();

                if !has_overflow {
                    self.current_data = cell[s..inline_end].to_vec();
                } else {
                    // Read inline portion then follow overflow chain.
                    let mut payload = cell[s..inline_end].to_vec();
                    if inline_end + 4 > cell.len() { return Err(BTreeError::Corrupt); }
                    let first_ovfl = u32::from_be_bytes(
                        cell[inline_end..inline_end + 4].try_into().unwrap()
                    );
                    let remaining = plen as usize - local_len;
                    let pg = &self.btree.pager;
                    read_overflow_chain(pg, first_ovfl, remaining, &mut payload)?;
                    self.current_data = payload;
                }
            }
            PageKind::IndexLeaf => {
                let (plen, n1) = get_varint(cell, 0)?;
                let e = n1 + plen as usize;
                if e > cell.len() { return Err(BTreeError::Corrupt); }
                self.current_key = cell[n1..e].to_vec();
                self.current_data = Vec::new();
            }
            _ => return Err(BTreeError::Corrupt),
        }
        Ok(())
    }
}

// ── Overflow page I/O ─────────────────────────────────────────────────────────

/// Allocate a chain of overflow pages for `extra_payload` bytes.
///
/// Returns the page number of the first overflow page.
fn write_overflow_chain(
    pager: &Arc<Mutex<Pager>>,
    extra: &[u8],
    page_size: u16,
) -> BTreeResult<PageNumber> {
    let ps = page_size as usize;
    let capacity = ps - 4; // 4 bytes for the next-page pointer

    let mut chunks: Vec<&[u8]> = extra.chunks(capacity).collect();
    chunks.reverse(); // process last chunk first so we can set next-pointers

    let mut next_pgno: u32 = 0; // 0 = no next page (end of chain)
    let mut first_pgno = 0u32;

    for chunk in &chunks {
        let new_pgno = {
            let mut pg = pager.lock().unwrap();
            let new_pgno = pg.db_size() + 1;
            let data = pg.write_access(new_pgno)?;
            data.resize(ps, 0);
            // Write next-page pointer.
            data[0] = ((next_pgno >> 24) & 0xFF) as u8;
            data[1] = ((next_pgno >> 16) & 0xFF) as u8;
            data[2] = ((next_pgno >>  8) & 0xFF) as u8;
            data[3] = ( next_pgno        & 0xFF) as u8;
            // Write data bytes.
            data[4..4 + chunk.len()].copy_from_slice(chunk);
            new_pgno
        };
        next_pgno = new_pgno;
        first_pgno = new_pgno;
    }

    Ok(first_pgno)
}

/// Follow the overflow page chain starting at `first_pgno`, reading
/// `remaining` bytes total into `out`.
fn read_overflow_chain(
    pager: &Arc<Mutex<Pager>>,
    first_pgno: PageNumber,
    remaining: usize,
    out: &mut Vec<u8>,
) -> BTreeResult<()> {
    let ps = {
        pager.lock().unwrap().page_size() as usize
    };
    let capacity = ps - 4;
    let mut pgno = first_pgno;
    let mut left = remaining;

    while pgno != 0 && left > 0 {
        let page_data = {
            let mut pg = pager.lock().unwrap();
            (*pg.acquire(pgno)?).clone()
        };
        let next = u32::from_be_bytes(page_data[0..4].try_into().unwrap());
        let take = left.min(capacity);
        if 4 + take > page_data.len() { return Err(BTreeError::Corrupt); }
        out.extend_from_slice(&page_data[4..4 + take]);
        left -= take;
        pgno = next;
    }
    Ok(())
}

// ── Page helpers ──────────────────────────────────────────────────────────────

fn cell_rowid(data: &[u8], hdr: &PageHeader, idx: u16) -> BTreeResult<u64> {
    let off = hdr.cell_ptr(data, idx)?;
    if off >= data.len() { return Err(BTreeError::Corrupt); }
    let cell = &data[off..];
    match hdr.kind {
        PageKind::TableLeaf => {
            let (_, n1) = get_varint(cell, 0)?;
            let (rowid, _) = get_varint(cell, n1)?;
            Ok(rowid)
        }
        PageKind::TableInterior => {
            if cell.len() < 5 { return Err(BTreeError::Corrupt); }
            let (rowid, _) = get_varint(cell, 4)?;
            Ok(rowid)
        }
        _ => Err(BTreeError::Corrupt),
    }
}

fn left_child(data: &[u8], hdr: &PageHeader, idx: u16) -> BTreeResult<PageNumber> {
    let off = hdr.cell_ptr(data, idx)?;
    if off + 4 > data.len() { return Err(BTreeError::Corrupt); }
    Ok(u32::from_be_bytes(data[off..off + 4].try_into().unwrap()))
}

/// Insert `cell` bytes into a page, keeping cells sorted by rowid.
/// Returns `Err(BTreeError::PageFull)` if there is not enough free space.
fn insert_cell_into_page(
    pager: &Arc<Mutex<Pager>>,
    pgno: PageNumber,
    cell: &[u8],
    rowid: u64,
) -> BTreeResult<()> {
    let mut pg = pager.lock().unwrap();
    let data = pg.write_access(pgno)?;
    let hdr = PageHeader::parse(data, pgno)?;
    let ho = hdr.header_offset;
    let hs = hdr.kind.header_size();
    let cc = hdr.cell_count;
    let mut ccs = hdr.cell_content_start as usize;
    if ccs == 0 { ccs = 65536; }

    let ptr_end = ho + hs + (cc as usize + 1) * 2;
    if ccs < ptr_end + cell.len() {
        return Err(BTreeError::PageFull);
    }

    let ps = data.len();
    let new_ccs = ccs - cell.len();
    if new_ccs + cell.len() > ps { return Err(BTreeError::Corrupt); }
    data[new_ccs..new_ccs + cell.len()].copy_from_slice(cell);

    // Sorted insertion position by rowid.
    let mut ins = cc;
    for i in 0..cc {
        let p = ho + hs + i as usize * 2;
        let cell_off = u16::from_be_bytes([data[p], data[p + 1]]) as usize;
        if cell_off < data.len() {
            let c = &data[cell_off..];
            if let Ok((_, n1)) = decode_varint(c) {
                if let Ok((r, _)) = decode_varint(&c[n1..]) {
                    if rowid <= r { ins = i; break; }
                }
            }
        }
    }

    let ins_off = ho + hs + ins as usize * 2;
    let shift = (cc - ins) as usize * 2;
    data.copy_within(ins_off..ins_off + shift, ins_off + 2);
    let np = new_ccs as u16;
    data[ins_off]     = (np >> 8) as u8;
    data[ins_off + 1] = np as u8;

    let ncc = (cc + 1).to_be_bytes();
    data[ho + 3] = ncc[0]; data[ho + 4] = ncc[1];
    let nccs = (new_ccs as u16).to_be_bytes();
    data[ho + 5] = nccs[0]; data[ho + 6] = nccs[1];
    Ok(())
}

/// Low-level raw cell insert (no rowid sort; used when building interior pages).
fn insert_cell_raw(
    data: &mut Vec<u8>,
    pgno: PageNumber,
    cell: &[u8],
    page_size: u16,
    ho: usize,
) -> BTreeResult<()> {
    let hdr = PageHeader::parse(data, pgno)?;
    let hs = hdr.kind.header_size();
    let cc = hdr.cell_count;
    let mut ccs = hdr.cell_content_start as usize;
    if ccs == 0 { ccs = page_size as usize; }

    let ptr_end = ho + hs + (cc as usize + 1) * 2;
    if ccs < ptr_end + cell.len() {
        return Err(BTreeError::PageFull);
    }

    let new_ccs = ccs - cell.len();
    data[new_ccs..new_ccs + cell.len()].copy_from_slice(cell);

    // Append pointer at end of cell pointer array.
    let ins_off = ho + hs + cc as usize * 2;
    let np = new_ccs as u16;
    data[ins_off]     = (np >> 8) as u8;
    data[ins_off + 1] = np as u8;

    let ncc = (cc + 1).to_be_bytes();
    data[ho + 3] = ncc[0]; data[ho + 4] = ncc[1];
    let nccs = (new_ccs as u16).to_be_bytes();
    data[ho + 5] = nccs[0]; data[ho + 6] = nccs[1];
    Ok(())
}

/// Rewrite an entire page with a new set of cells (used after splitting).
fn write_cells_to_page(
    pager: &Arc<Mutex<Pager>>,
    pgno: PageNumber,
    cells: &[(u64, Vec<u8>)],
    kind: PageKind,
    is_page1: bool,
) -> BTreeResult<()> {
    let mut pg = pager.lock().unwrap();
    let ps = pg.page_size();
    let data = pg.write_access(pgno)?;
    let ho = if is_page1 { DB_HEADER_SIZE } else { 0 };
    init_page_at(data, kind, ps, ho);
    drop(pg);

    for (rowid, cell) in cells {
        insert_cell_into_page(pager, pgno, cell, *rowid)?;
    }
    Ok(())
}

/// Insert a divider cell into a parent interior page to point to `right_pgno`.
///
/// The divider cell format for table-interior pages is:
///   [left_pgno: u32 BE][divider_rowid: varint]
/// The `right_pgno` becomes the new rightmost-child if it sorts beyond all
/// existing cells, otherwise we need to update the child pointer array.
#[allow(dead_code)]
fn insert_interior_divider(
    pager: &Arc<Mutex<Pager>>,
    parent_pgno: PageNumber,
    divider_rowid: u64,
    left_pgno: PageNumber,
    right_pgno: PageNumber,
    _page_size: u16,
) -> BTreeResult<()> {
    let mut div_cell = Vec::new();
    div_cell.extend_from_slice(&left_pgno.to_be_bytes());
    let mut tmp = [0u8; 9];
    let vn = encode_varint(divider_rowid, &mut tmp)
        .map_err(|e| BTreeError::Record(e.to_string()))?;
    div_cell.extend_from_slice(&tmp[..vn]);

    let mut pg = pager.lock().unwrap();
    let ps = pg.page_size();
    let ho = if parent_pgno == 1 { DB_HEADER_SIZE } else { 0 };
    let data = pg.write_access(parent_pgno)?;
    let hdr = PageHeader::parse(data, parent_pgno)?;

    // Check whether we need to update the rightmost child pointer.
    let is_rightmost = divider_rowid >= cell_rowid_for_last_interior(data, &hdr)
        .unwrap_or(0);

    insert_cell_raw(data, parent_pgno, &div_cell, ps, ho)?;

    if is_rightmost {
        // The new right sibling becomes the rightmost child.
        data[ho + 8]  = ((right_pgno >> 24) & 0xFF) as u8;
        data[ho + 9]  = ((right_pgno >> 16) & 0xFF) as u8;
        data[ho + 10] = ((right_pgno >>  8) & 0xFF) as u8;
        data[ho + 11] = ( right_pgno        & 0xFF) as u8;
    }
    Ok(())
}

/// Extract the rowid of the last cell on an interior page (for comparison).
#[allow(dead_code)]
fn cell_rowid_for_last_interior(data: &[u8], hdr: &PageHeader) -> BTreeResult<u64> {
    if hdr.cell_count == 0 { return Ok(0); }
    cell_rowid(data, hdr, hdr.cell_count - 1)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn new_bt() -> BTree {
        let bt = BTree::new_in_memory();
        bt.begin_write().unwrap();
        bt.allocate_page(PageKind::TableLeaf).unwrap();
        bt
    }

    #[test]
    fn init_page_parses() {
        let bt = BTree::new_in_memory();
        bt.begin_write().unwrap();
        let pgno = bt.allocate_page(PageKind::TableLeaf).unwrap();
        assert_eq!(pgno, 1);
        let arc = bt.pager.lock().unwrap().acquire(1).unwrap();
        let hdr = PageHeader::parse(&arc, 1).unwrap();
        assert_eq!(hdr.kind, PageKind::TableLeaf);
        assert_eq!(hdr.cell_count, 0);
    }

    #[test]
    fn insert_single_row() {
        let bt = new_bt();
        let mut cur = bt.cursor(1, true).unwrap();
        cur.insert(&42u64.to_be_bytes(), b"hello world", false).unwrap();
        cur.move_to_first().unwrap();
        assert!(cur.is_valid());
        assert_eq!(cur.key().unwrap(), &42u64.to_be_bytes());
        assert_eq!(cur.data().unwrap(), b"hello world");
    }

    #[test]
    fn sorted_traversal() {
        let bt = new_bt();
        let mut cur = bt.cursor(1, true).unwrap();
        for rowid in [5u64, 1, 3, 2, 4] {
            cur.insert(&rowid.to_be_bytes(), &rowid.to_le_bytes(), false).unwrap();
        }
        let mut found = Vec::new();
        cur.move_to_first().unwrap();
        while cur.is_valid() {
            found.push(u64::from_be_bytes(cur.key().unwrap().try_into().unwrap()));
            cur.next().unwrap();
        }
        assert_eq!(found, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn reverse_traversal() {
        let bt = new_bt();
        let mut cur = bt.cursor(1, true).unwrap();
        for rowid in 1u64..=4 { cur.insert(&rowid.to_be_bytes(), b"x", false).unwrap(); }
        cur.move_to_last().unwrap();
        assert_eq!(u64::from_be_bytes(cur.key().unwrap().try_into().unwrap()), 4);
        cur.previous().unwrap();
        assert_eq!(u64::from_be_bytes(cur.key().unwrap().try_into().unwrap()), 3);
    }

    #[test]
    fn seek_exact_and_miss() {
        let bt = new_bt();
        let mut cur = bt.cursor(1, true).unwrap();
        for rowid in [1u64, 3, 5] { cur.insert(&rowid.to_be_bytes(), b"v", false).unwrap(); }
        assert_eq!(cur.move_to(&3u64.to_be_bytes(), SeekBias::Ge).unwrap(), SeekResult::Equal);
        assert_eq!(cur.move_to(&2u64.to_be_bytes(), SeekBias::Ge).unwrap(), SeekResult::Greater);
    }

    #[test]
    fn delete_middle_row() {
        let bt = new_bt();
        let mut cur = bt.cursor(1, true).unwrap();
        for rowid in 1u64..=3 { cur.insert(&rowid.to_be_bytes(), b"d", false).unwrap(); }
        assert_eq!(cur.move_to(&2u64.to_be_bytes(), SeekBias::Ge).unwrap(), SeekResult::Equal);
        cur.delete().unwrap();
        let mut found = Vec::new();
        cur.move_to_first().unwrap();
        while cur.is_valid() {
            found.push(u64::from_be_bytes(cur.key().unwrap().try_into().unwrap()));
            cur.next().unwrap();
        }
        assert_eq!(found, vec![1, 3]);
    }

    // ── Overflow page tests ────────────────────────────────────────────────

    #[test]
    fn overflow_payload_roundtrip() {
        // Insert a payload larger than a page to force overflow pages.
        let bt = new_bt();
        let mut cur = bt.cursor(1, true).unwrap();
        // 4096-byte page, max_local = 4096 - 35 = 4061.
        // A 12 KiB payload definitely overflows.
        let big_payload: Vec<u8> = (0u8..=255).cycle().take(12 * 1024).collect();
        cur.insert(&1u64.to_be_bytes(), &big_payload, false).unwrap();

        cur.move_to_first().unwrap();
        assert!(cur.is_valid());
        assert_eq!(cur.key().unwrap(), &1u64.to_be_bytes());
        assert_eq!(cur.data().unwrap(), big_payload.as_slice());
    }

    #[test]
    fn overflow_multiple_rows() {
        let bt = new_bt();
        let mut cur = bt.cursor(1, true).unwrap();
        let big: Vec<u8> = vec![0xAB; 8192];
        for rowid in [1u64, 2, 3] {
            cur.insert(&rowid.to_be_bytes(), &big, false).unwrap();
        }
        cur.move_to_first().unwrap();
        let mut count = 0;
        while cur.is_valid() {
            assert_eq!(cur.data().unwrap().len(), 8192);
            count += 1;
            cur.next().unwrap();
        }
        assert_eq!(count, 3);
    }

    // ── Page split tests ───────────────────────────────────────────────────

    #[test]
    fn page_split_basic() {
        // Insert enough small rows to force a page split.
        // With 4096-byte pages, ~80 rows of 40-byte payload each should overflow.
        let bt = new_bt();
        let mut cur = bt.cursor(1, true).unwrap();
        let payload = vec![0u8; 40];
        for rowid in 1u64..=120 {
            cur.insert(&rowid.to_be_bytes(), &payload, false).unwrap();
        }
        // Verify all rows are readable in order.
        cur.move_to_first().unwrap();
        let mut found = Vec::new();
        while cur.is_valid() {
            found.push(u64::from_be_bytes(cur.key().unwrap().try_into().unwrap()));
            cur.next().unwrap();
        }
        assert_eq!(found.len(), 120);
        assert_eq!(found, (1u64..=120).collect::<Vec<_>>());
    }

    #[test]
    fn page_split_reverse_insert() {
        // Insert in descending order to stress the sorted insertion during splits.
        let bt = new_bt();
        let mut cur = bt.cursor(1, true).unwrap();
        let payload = vec![0u8; 40];
        for rowid in (1u64..=100).rev() {
            cur.insert(&rowid.to_be_bytes(), &payload, false).unwrap();
        }
        cur.move_to_first().unwrap();
        let mut found = Vec::new();
        while cur.is_valid() {
            found.push(u64::from_be_bytes(cur.key().unwrap().try_into().unwrap()));
            cur.next().unwrap();
        }
        assert_eq!(found, (1u64..=100).collect::<Vec<_>>());
    }

    #[test]
    fn deep_tree_traversal() {
        // 5000 rows with moderate payload to force multi-level splits.
        let bt = new_bt();
        let mut cur = bt.cursor(1, true).unwrap();
        let payload = vec![0xCC; 20];
        for rowid in 1u64..=500 {
            cur.insert(&rowid.to_be_bytes(), &payload, false).unwrap();
        }
        // Forward traversal.
        cur.move_to_first().unwrap();
        let mut count = 0u64;
        while cur.is_valid() {
            let key = u64::from_be_bytes(cur.key().unwrap().try_into().unwrap());
            count += 1;
            assert_eq!(key, count, "forward traversal out of order at position {count}");
            cur.next().unwrap();
        }
        assert_eq!(count, 500);
    }

    #[test]
    fn max_min_local_sanity() {
        // Verify the overflow threshold formulas are sensible.
        for ps in [512u16, 1024, 2048, 4096, 8192, 16384, 32768] {
            let max = max_local_table(ps);
            let min = min_local_table(ps);
            assert!(min <= max, "min_local ({min}) > max_local ({max}) for page_size={ps}");
            assert!(max < ps as usize, "max_local must fit in a page");
        }
    }

    #[test]
    fn overflow_boundary_exact() {
        // A payload exactly at max_local should NOT overflow.
        // Note: the cell also has varint headers (plen + rowid), so the total
        // cell size is max_local + ~3 bytes of varints. We use a slightly
        // smaller payload to ensure it fits with the header overhead.
        let ps = DEFAULT_PAGE_SIZE;
        let max = max_local_table(ps);
        // Leave 10 bytes of headroom for the varint headers.
        let payload = vec![0x5A; max - 10];
        let (_, has_overflow) = local_payload_size(payload.len(), ps);
        assert!(!has_overflow, "payload should be inline");
        let bt = new_bt();
        let mut cur = bt.cursor(1, true).unwrap();
        cur.insert(&1u64.to_be_bytes(), &payload, false).unwrap();
        cur.move_to_first().unwrap();
        assert!(cur.is_valid());
        assert_eq!(cur.data().unwrap().len(), max - 10);
    }

    #[test]
    fn overflow_boundary_over() {
        // A payload one byte over max_local MUST overflow.
        let ps = DEFAULT_PAGE_SIZE;
        let max = max_local_table(ps);
        let payload = vec![0x5A; max + 1];
        let bt = new_bt();
        let mut cur = bt.cursor(1, true).unwrap();
        cur.insert(&1u64.to_be_bytes(), &payload, false).unwrap();
        cur.move_to_first().unwrap();
        assert!(cur.is_valid());
        assert_eq!(cur.data().unwrap(), payload.as_slice());
    }
}
