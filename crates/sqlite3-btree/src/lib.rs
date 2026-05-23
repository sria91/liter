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

fn init_page(data: &mut Vec<u8>, kind: PageKind, page_size: u16) {
    init_page_at(data, kind, page_size, 0)
}

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

    // ── Write ops ─────────────────────────────────────────────────────────────

    /// Insert `(rowid, payload)` into a table leaf.
    /// Phase 2: no overflow, no page splits.
    pub fn insert(&mut self, key: &[u8], data: &[u8], _append: bool) -> BTreeResult<()> {
        if key.len() != 8 { return Err(BTreeError::Corrupt); }
        let rowid = u64::from_be_bytes(key.try_into().unwrap());
        let mut cell = Vec::new();
        put_varint(&mut cell, data.len() as u64)?;
        put_varint(&mut cell, rowid)?;
        cell.extend_from_slice(data);
        let pgno = self.root_page;
        let mut pg = self.btree.pager.lock().unwrap();
        insert_cell(&mut pg, pgno, &cell, rowid)?;
        self.state = CursorState::Invalid;
        Ok(())
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
                let e = s + plen as usize;
                if e > cell.len() { return Err(BTreeError::Corrupt); }
                self.current_key = rowid.to_be_bytes().to_vec();
                self.current_data = cell[s..e].to_vec();
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

fn insert_cell(pg: &mut Pager, pgno: PageNumber, cell: &[u8], rowid: u64) -> BTreeResult<()> {
    let data = pg.write_access(pgno)?;
    let hdr = PageHeader::parse(data, pgno)?;
    let ho = hdr.header_offset;
    let hs = hdr.kind.header_size();
    let cc = hdr.cell_count;
    let mut ccs = hdr.cell_content_start as usize;
    if ccs == 0 { ccs = 65536; }

    let ptr_end = ho + hs + (cc as usize + 1) * 2;
    if ccs < ptr_end + cell.len() { return Err(BTreeError::Corrupt); }

    let new_ccs = ccs - cell.len();
    data[new_ccs..new_ccs + cell.len()].copy_from_slice(cell);

    // Sorted insertion position.
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
        let bt = BTree::new_in_memory();
        bt.begin_write().unwrap();
        bt.allocate_page(PageKind::TableLeaf).unwrap();
        let mut cur = bt.cursor(1, true).unwrap();
        cur.insert(&42u64.to_be_bytes(), b"hello world", false).unwrap();
        cur.move_to_first().unwrap();
        assert!(cur.is_valid());
        assert_eq!(cur.key().unwrap(), &42u64.to_be_bytes());
        assert_eq!(cur.data().unwrap(), b"hello world");
    }

    #[test]
    fn sorted_traversal() {
        let bt = BTree::new_in_memory();
        bt.begin_write().unwrap();
        bt.allocate_page(PageKind::TableLeaf).unwrap();
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
        let bt = BTree::new_in_memory();
        bt.begin_write().unwrap();
        bt.allocate_page(PageKind::TableLeaf).unwrap();
        let mut cur = bt.cursor(1, true).unwrap();
        for rowid in 1u64..=4 { cur.insert(&rowid.to_be_bytes(), b"x", false).unwrap(); }
        cur.move_to_last().unwrap();
        assert_eq!(u64::from_be_bytes(cur.key().unwrap().try_into().unwrap()), 4);
        cur.previous().unwrap();
        assert_eq!(u64::from_be_bytes(cur.key().unwrap().try_into().unwrap()), 3);
    }

    #[test]
    fn seek_exact_and_miss() {
        let bt = BTree::new_in_memory();
        bt.begin_write().unwrap();
        bt.allocate_page(PageKind::TableLeaf).unwrap();
        let mut cur = bt.cursor(1, true).unwrap();
        for rowid in [1u64, 3, 5] { cur.insert(&rowid.to_be_bytes(), b"v", false).unwrap(); }
        assert_eq!(cur.move_to(&3u64.to_be_bytes(), SeekBias::Ge).unwrap(), SeekResult::Equal);
        assert_eq!(cur.move_to(&2u64.to_be_bytes(), SeekBias::Ge).unwrap(), SeekResult::Greater);
    }

    #[test]
    fn delete_middle_row() {
        let bt = BTree::new_in_memory();
        bt.begin_write().unwrap();
        bt.allocate_page(PageKind::TableLeaf).unwrap();
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
}
