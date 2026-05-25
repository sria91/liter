//! Virtual Database Engine (VDBE) for SQLite3-rs.
//!
//! Mirrors `vdbe.c`, `vdbeapi.c`, `vdbeaux.c`, `vdbemem.c`, `vdbesort.c`,
//! `vdbeblob.c`. Executes compiled bytecode programs against the B-tree layer.
//!
//! ## Status
//! Phase 3 — type definitions and VM skeleton only.

use sqlite3_btree::PageKind;
use std::sync::Arc;


/// Trait for aggregate function states (e.g., SUM, COUNT).
pub trait AggregateState: std::fmt::Debug {
    fn step(&mut self, args: &[Mem]) -> Result<(), String>;
    fn finalize(&mut self) -> Result<Mem, String>;
}

/// A VDBE register value.
#[derive(Debug, Clone, PartialEq)]
pub enum Mem {
    Null,
    Int(i64),
    Real(f64),
    Text(Arc<str>),
    Blob(Arc<[u8]>),
    /// Zero-blob placeholder: a blob of `n` zero bytes, materialized on demand.
    ZeroBlob(i64),
    /// Aggregate state index (internal pointer to Vdbe's aggs list).
    Agg(usize),
}

impl Mem {
    pub fn is_null(&self) -> bool { matches!(self, Mem::Null) }

    /// True if the value is considered "true" in a boolean context
    /// (non-null, non-zero integer, non-zero real).
    pub fn is_truthy(&self) -> bool {
        match self {
            Mem::Null => false,
            Mem::Int(i) => *i != 0,
            Mem::Real(f) => *f != 0.0,
            Mem::Text(s) => !s.is_empty(),
            Mem::Blob(b) => !b.is_empty(),
            Mem::ZeroBlob(_) => false,
            Mem::Agg(_) => false,
        }
    }

    pub fn to_int(&self) -> Option<i64> {
        match self {
            Mem::Int(i) => Some(*i),
            Mem::Real(f) => Some(*f as i64),
            _ => None,
        }
    }

    pub fn to_real(&self) -> Option<f64> {
        match self {
            Mem::Real(f) => Some(*f),
            Mem::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Compare two Mem values using SQLite sorting rules:
    /// Null < Integer/Real < Text < Blob
    #[allow(clippy::should_implement_trait)]
    pub fn cmp(&self, other: &Mem) -> std::cmp::Ordering {
        use std::cmp::Ordering;

        fn type_class(m: &Mem) -> u8 {
            match m {
                Mem::Null => 1,
                Mem::Int(_) | Mem::Real(_) => 2,
                Mem::Text(_) => 3,
                Mem::Blob(_) | Mem::ZeroBlob(_) => 4,
                Mem::Agg(_) => 5,
            }
        }

        let tc1 = type_class(self);
        let tc2 = type_class(other);

        if tc1 != tc2 {
            return tc1.cmp(&tc2);
        }

        match (self, other) {
            (Mem::Null, Mem::Null) => Ordering::Equal,
            (Mem::Int(i1), Mem::Int(i2)) => i1.cmp(i2),
            (Mem::Real(f1), Mem::Real(f2)) => f1.total_cmp(f2),
            (Mem::Int(i1), Mem::Real(f2)) => (*i1 as f64).total_cmp(f2),
            (Mem::Real(f1), Mem::Int(i2)) => f1.total_cmp(&(*i2 as f64)),
            (Mem::Text(s1), Mem::Text(s2)) => s1.cmp(s2), // simple binary string cmp for now
            (Mem::Blob(b1), Mem::Blob(b2)) => b1.cmp(b2),
            (Mem::ZeroBlob(a), Mem::ZeroBlob(b)) => a.cmp(b),
            (Mem::Blob(b1), Mem::ZeroBlob(n2)) => {
                let zero_len = *n2 as usize;
                for i in 0..std::cmp::min(b1.len(), zero_len) {
                    if b1[i] != 0 {
                        return b1[i].cmp(&0);
                    }
                }
                b1.len().cmp(&zero_len)
            }
            (Mem::ZeroBlob(n1), Mem::Blob(b2)) => {
                let zero_len = *n1 as usize;
                for i in 0..std::cmp::min(zero_len, b2.len()) {
                    if b2[i] != 0 {
                        return 0.cmp(&b2[i]);
                    }
                }
                zero_len.cmp(&b2.len())
            }
            (Mem::Agg(_), _) => Ordering::Equal,
            (_, Mem::Agg(_)) => Ordering::Equal,
            _ => Ordering::Equal,
        }
    }
}

/// VDBE error type.
#[derive(Debug, thiserror::Error)]
pub enum VdbeError {
    #[error("execution error: {0}")]
    Exec(String),
    #[error("out of memory")]
    NoMem,
    #[error("constraint violation: {0}")]
    Constraint(String),
    #[error("not yet implemented")]
    NotImplemented,
    #[error("btree error: {0}")]
    BTree(#[from] sqlite3_btree::BTreeError),
    #[error("record error: {0}")]
    Record(#[from] sqlite3_record::RecordError),
}

pub type VdbeResult<T> = Result<T, VdbeError>;

/// Result of a single `step()` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepResult {
    /// A result row is available.
    Row,
    /// Execution is complete.
    Done,
}

/// Cursor kinds used by the VDBE.
#[derive(Debug)]
pub enum VdbeCursorKind {
    BTree,
    Sorter,
    Pseudo,
}

pub struct Sorter {
    pub records: Vec<Vec<u8>>,
    pub pos: usize,
}

pub enum VdbeCursor<'a> {
    BTree(sqlite3_btree::BTreeCursor<'a>),
    Sorter(Sorter),
}

impl<'a> Default for VdbeCursor<'a> {
    fn default() -> Self {
        // Not used directly, but useful for arrays
        VdbeCursor::Sorter(Sorter { records: vec![], pos: 0 })
    }
}

impl<'a> VdbeCursor<'a> {
    pub fn as_btree(&self) -> VdbeResult<&sqlite3_btree::BTreeCursor<'a>> {
        match self {
            VdbeCursor::BTree(c) => Ok(c),
            _ => Err(VdbeError::Exec("cursor is not a btree cursor".into())),
        }
    }

    pub fn as_btree_mut(&mut self) -> VdbeResult<&mut sqlite3_btree::BTreeCursor<'a>> {
        match self {
            VdbeCursor::BTree(c) => Ok(c),
            _ => Err(VdbeError::Exec("cursor is not a btree cursor".into())),
        }
    }

    pub fn as_sorter_mut(&mut self) -> VdbeResult<&mut Sorter> {
        match self {
            VdbeCursor::Sorter(s) => Ok(s),
            _ => Err(VdbeError::Exec("cursor is not a sorter cursor".into())),
        }
    }
}

/// A single VDBE opcode.
///
/// Based on the ~200 opcodes in `vdbe.c`. Most fields are register indices.
#[derive(Debug, Clone)]
pub struct VdbeOp {
    pub opcode: Opcode,
    pub p1: i32,
    pub p2: i32,
    pub p3: i32,
    pub p4: P4,
    pub p5: u16,
}

/// Opcode enumeration. Variants will be filled in during Phase 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
#[repr(u8)]
pub enum Opcode {
    // Control flow
    Init = 0,
    Halt,
    Goto,
    Gosub,
    Return,
    // Integer arithmetic
    AddInt,
    SubtractInt,
    MultiplyInt,
    DivideInt,
    RemainderInt,
    // Register manipulation
    Integer,
    Real,
    String8,
    Null,
    Move,
    Copy,
    // Comparison
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    // Cursor ops
    OpenRead,
    OpenWrite,
    OpenEphemeral,
    Close,
    SeekGe,
    SeekGt,
    SeekLe,
    SeekLt,
    Next,
    Prev,
    Rewind,
    Last,
    // Row access
    Column,
    MakeRecord,
    Insert,
    InsertInt,
    Delete,
    RowId,
    NewRowid,
    NullRow,
    // Schema
    CreateTable,
    ParseSchema,
    // Transactions
    AutoCommit,
    Transaction,
    // Output
    ResultRow,
    // Conditionals
    If,
    IfNot,
    IsNull,
    NotNull,
    // Limit and Loop control
    DecrJumpZero,
    // Sorting
    SorterOpen,
    SorterInsert,
    SorterSort,
    SorterData,
    SorterNext,
    // Functions & Aggregates
    Function,
    AggStep,
    AggFinal,
    // Misc
    Noop,
}

/// The P4 operand of a VDBE instruction (can hold various large values).
#[derive(Debug, Clone)]
pub enum P4 {
    None,
    Int32(i32),
    Int64(i64),
    Real(f64),
    Text(Arc<str>),
    Blob(Arc<[u8]>),
    // FuncDef, CollSeq, etc. — Phase 3
}

/// The VDBE virtual machine.
pub struct Vdbe {
    /// The compiled program.
    pub ops: Vec<VdbeOp>,
    /// Program counter.
    pc: usize,
    /// Register file.
    regs: Vec<Mem>,
    /// Subroutine call stack.
    call_stack: Vec<usize>,
    /// Whether the VM has halted.
    halted: bool,
    last_result_row: Option<(usize, usize)>,
    /// Number of cursors required by this program.
    pub n_cursors: usize,
    /// Function dispatcher to handle Opcode::Function.
    #[allow(clippy::type_complexity)]
    pub func_dispatcher: Option<fn(&str, &[Mem]) -> Result<Mem, String>>,
    /// Dispatcher to instantiate aggregate states for Opcode::AggStep.
    #[allow(clippy::type_complexity)]
    pub agg_dispatcher: Option<fn(&str) -> Result<Box<dyn AggregateState>, String>>,
    /// Instantiated aggregate states.
    aggs: Vec<Box<dyn AggregateState>>,
}

impl Default for Vdbe {
    fn default() -> Self {
        Self::new()
    }
}

impl Vdbe {
    /// Create a new VDBE with an empty program.
    pub fn new() -> Self {
        Self {
            ops: Vec::new(),
            pc: 0,
            regs: Vec::new(),
            call_stack: Vec::new(),
            halted: false,
            last_result_row: None,
            n_cursors: 0,
            func_dispatcher: None,
            agg_dispatcher: None,
            aggs: Vec::new(),
        }
    }

    pub fn with_capacity(n_ops: usize, n_regs: usize) -> Self {
        Self {
            ops: Vec::with_capacity(n_ops),
            pc: 0,
            regs: vec![Mem::Null; n_regs],
            call_stack: Vec::new(),
            halted: false,
            last_result_row: None,
            n_cursors: 0,
            func_dispatcher: None,
            agg_dispatcher: None,
            aggs: Vec::new(),
        }
    }

    /// Extract the slice of registers from the last `ResultRow` emission.
    pub fn current_result_row(&self) -> Option<&[Mem]> {
        if let Some((start, count)) = self.last_result_row {
            Some(&self.regs[start..start + count])
        } else {
            None
        }
    }

    /// Allocate a register and return its index.
    pub fn alloc_reg(&mut self) -> usize {
        let idx = self.regs.len();
        self.regs.push(Mem::Null);
        idx
    }

    /// Emit an instruction and return its address (index in `ops`).
    pub fn emit(&mut self, op: VdbeOp) -> usize {
        let addr = self.ops.len();
        self.ops.push(op);
        addr
    }

    pub fn step<'a>(&mut self, btree: &'a sqlite3_btree::BTree, cursors: &mut [Option<VdbeCursor<'a>>]) -> VdbeResult<StepResult> {
        if self.halted {
            return Ok(StepResult::Done);
        }

        while self.pc < self.ops.len() {
            let op = &self.ops[self.pc];
            self.pc += 1;

            match op.opcode {
                Opcode::Init => {
                    self.pc = op.p2 as usize;
                }
                Opcode::Halt => {
                    self.halted = true;
                    return Ok(StepResult::Done);
                }
                Opcode::Goto => {
                    self.pc = op.p2 as usize;
                }
                Opcode::Gosub => {
                    self.call_stack.push(self.pc);
                    self.pc = op.p2 as usize;
                }
                Opcode::Return => {
                    if let Some(ret) = self.call_stack.pop() {
                        self.pc = ret;
                    } else {
                        return Err(VdbeError::Exec("Return without Gosub".to_string()));
                    }
                }
                Opcode::Integer => {
                    self.regs[op.p2 as usize] = Mem::Int(op.p1 as i64);
                }
                Opcode::Real => {
                    if let P4::Real(f) = op.p4 {
                        self.regs[op.p2 as usize] = Mem::Real(f);
                    } else {
                        return Err(VdbeError::Exec("Invalid P4 for Real opcode".to_string()));
                    }
                }
                Opcode::String8 => {
                    if let P4::Text(ref t) = op.p4 {
                        self.regs[op.p2 as usize] = Mem::Text(t.clone());
                    } else {
                        return Err(VdbeError::Exec("Invalid P4 for String8 opcode".to_string()));
                    }
                }
                Opcode::Null => {
                    self.regs[op.p2 as usize] = Mem::Null;
                }
                Opcode::Move => {
                    let mut val = Mem::Null;
                    std::mem::swap(&mut val, &mut self.regs[op.p1 as usize]);
                    self.regs[op.p2 as usize] = val;
                }
                Opcode::Copy => {
                    self.regs[op.p2 as usize] = self.regs[op.p1 as usize].clone();
                }
                Opcode::AddInt => {
                    if let (Some(a), Some(b)) = (self.regs[op.p1 as usize].to_int(), self.regs[op.p2 as usize].to_int()) {
                        self.regs[op.p3 as usize] = Mem::Int(b + a);
                    } else {
                        return Err(VdbeError::Exec("Type mismatch in AddInt".to_string()));
                    }
                }
                Opcode::SubtractInt => {
                    if let (Some(a), Some(b)) = (self.regs[op.p1 as usize].to_int(), self.regs[op.p2 as usize].to_int()) {
                        self.regs[op.p3 as usize] = Mem::Int(b - a);
                    } else {
                        return Err(VdbeError::Exec("Type mismatch in SubtractInt".to_string()));
                    }
                }
                Opcode::MultiplyInt => {
                    if let (Some(a), Some(b)) = (self.regs[op.p1 as usize].to_int(), self.regs[op.p2 as usize].to_int()) {
                        self.regs[op.p3 as usize] = Mem::Int(b * a);
                    } else {
                        return Err(VdbeError::Exec("Type mismatch in MultiplyInt".to_string()));
                    }
                }
                Opcode::DivideInt => {
                    if let (Some(a), Some(b)) = (self.regs[op.p1 as usize].to_int(), self.regs[op.p2 as usize].to_int()) {
                        if a == 0 {
                            return Err(VdbeError::Exec("Division by zero".to_string()));
                        }
                        self.regs[op.p3 as usize] = Mem::Int(b / a);
                    } else {
                        return Err(VdbeError::Exec("Type mismatch in DivideInt".to_string()));
                    }
                }
                Opcode::RemainderInt => {
                    if let (Some(a), Some(b)) = (self.regs[op.p1 as usize].to_int(), self.regs[op.p2 as usize].to_int()) {
                        if a == 0 {
                            return Err(VdbeError::Exec("Division by zero".to_string()));
                        }
                        self.regs[op.p3 as usize] = Mem::Int(b % a);
                    } else {
                        return Err(VdbeError::Exec("Type mismatch in RemainderInt".to_string()));
                    }
                }
                Opcode::Eq => {
                    let lhs = &self.regs[op.p3 as usize];
                    let rhs = &self.regs[op.p1 as usize];
                    let eq = match (lhs.to_real(), rhs.to_real()) {
                        (Some(a), Some(b)) => a == b,
                        _ => lhs == rhs,
                    };
                    if eq { self.pc = op.p2 as usize; }
                }
                Opcode::Ne => {
                    let lhs = &self.regs[op.p3 as usize];
                    let rhs = &self.regs[op.p1 as usize];
                    let ne = match (lhs.to_real(), rhs.to_real()) {
                        (Some(a), Some(b)) => a != b,
                        _ => lhs != rhs,
                    };
                    if ne { self.pc = op.p2 as usize; }
                }
                Opcode::Lt => {
                    let lhs = &self.regs[op.p3 as usize];
                    let rhs = &self.regs[op.p1 as usize];
                    let lt = match (lhs.to_real(), rhs.to_real()) {
                        (Some(a), Some(b)) => a < b,
                        _ => lhs.cmp(rhs) == std::cmp::Ordering::Less,
                    };
                    if lt { self.pc = op.p2 as usize; }
                }
                Opcode::Le => {
                    let lhs = &self.regs[op.p3 as usize];
                    let rhs = &self.regs[op.p1 as usize];
                    let le = match (lhs.to_real(), rhs.to_real()) {
                        (Some(a), Some(b)) => a <= b,
                        _ => matches!(lhs.cmp(rhs), std::cmp::Ordering::Less | std::cmp::Ordering::Equal),
                    };
                    if le { self.pc = op.p2 as usize; }
                }
                Opcode::Gt => {
                    let lhs = &self.regs[op.p3 as usize];
                    let rhs = &self.regs[op.p1 as usize];
                    let gt = match (lhs.to_real(), rhs.to_real()) {
                        (Some(a), Some(b)) => a > b,
                        _ => lhs.cmp(rhs) == std::cmp::Ordering::Greater,
                    };
                    if gt { self.pc = op.p2 as usize; }
                }
                Opcode::Ge => {
                    let lhs = &self.regs[op.p3 as usize];
                    let rhs = &self.regs[op.p1 as usize];
                    let ge = match (lhs.to_real(), rhs.to_real()) {
                        (Some(a), Some(b)) => a >= b,
                        _ => matches!(lhs.cmp(rhs), std::cmp::Ordering::Greater | std::cmp::Ordering::Equal),
                    };
                    if ge { self.pc = op.p2 as usize; }
                }
                Opcode::CreateTable => {
                    let pgno = btree.allocate_page(PageKind::TableLeaf)?;
                    self.regs[op.p2 as usize] = Mem::Int(pgno as i64);
                }
                Opcode::OpenWrite => {
                    let cursor_idx = op.p1 as usize;
                    let root_page = op.p2 as u32;
                    if cursor_idx >= cursors.len() {
                        return Err(VdbeError::Exec(format!("invalid cursor {}", cursor_idx)));
                    }
                    cursors[cursor_idx] = Some(VdbeCursor::BTree(btree.cursor(root_page, true)?));
                }
                Opcode::NewRowid => {
                    let cursor_idx = op.p1 as usize;
                    let dest_reg = op.p2 as usize;
                    if let Some(cursor) = &mut cursors[cursor_idx] {
                        let cursor = cursor.as_btree_mut()?;
                        let max_id = cursor.max_rowid()?;
                        self.regs[dest_reg] = Mem::Int(max_id + 1);
                    } else {
                        return Err(VdbeError::Exec("cursor not open".into()));
                    }
                }
                Opcode::MakeRecord => {
                    let start_reg = op.p1 as usize;
                    let count = op.p2 as usize;
                    let dest_reg = op.p3 as usize;

                    let mut values = Vec::with_capacity(count);
                    for i in 0..count {
                        let mem = &self.regs[start_reg + i];
                        let val = match mem {
                            Mem::Null => sqlite3_record::Value::Null,
                            Mem::Int(v) => sqlite3_record::Value::Int(*v),
                            Mem::Real(v) => sqlite3_record::Value::Real(*v),
                            Mem::Text(v) => sqlite3_record::Value::Text(v.as_bytes().to_vec()),
                            Mem::Blob(v) => sqlite3_record::Value::Blob(v.to_vec()),
                            Mem::ZeroBlob(n) => sqlite3_record::Value::ZeroBlob(*n),
                            Mem::Agg(_) => return Err(VdbeError::Exec("Cannot serialize Aggregate".into())),
                        };
                        values.push(val);
                    }

                    let record = sqlite3_record::encode_record(&values)?;
                    self.regs[dest_reg] = Mem::Blob(Arc::from(record.into_boxed_slice()));
                }
                Opcode::Insert => {
                    let cursor_idx = op.p1 as usize;
                    let record_reg = op.p2 as usize;
                    let rowid_reg = op.p3 as usize;

                    let rowid = if let Mem::Int(i) = self.regs[rowid_reg] {
                        i as u64
                    } else {
                        return Err(VdbeError::Exec("rowid must be integer".to_string()));
                    };

                    let record = if let Mem::Blob(b) = &self.regs[record_reg] {
                        b.clone()
                    } else {
                        return Err(VdbeError::Exec("record must be blob".to_string()));
                    };

                    if let Some(cursor) = &mut cursors[cursor_idx] {
                        cursor.as_btree_mut()?.insert(&rowid.to_be_bytes(), &record, false)?;
                    } else {
                        return Err(VdbeError::Exec("invalid cursor".to_string()));
                    }
                }
                Opcode::Close => {
                    let cursor_idx = op.p1 as usize;
                    if cursor_idx < cursors.len() {
                        cursors[cursor_idx] = None;
                    }
                }

                // ── Read-scan opcodes ──────────────────────────────────────────

                Opcode::OpenRead => {
                    let cursor_idx = op.p1 as usize;
                    let root_page = op.p2 as u32;
                    if cursor_idx >= cursors.len() {
                        return Err(VdbeError::Exec(format!("invalid cursor {}", cursor_idx)));
                    }
                    cursors[cursor_idx] = Some(VdbeCursor::BTree(btree.cursor(root_page, false)?));
                }

                Opcode::Rewind => {
                    let cursor_idx = op.p1 as usize;
                    let jump_addr = op.p2 as usize;
                    let cursor = cursors[cursor_idx].as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;
                    let has_rows = cursor.move_to_first()?;
                    if !has_rows {
                        self.pc = jump_addr;
                    }
                }

                Opcode::Next => {
                    let cursor_idx = op.p1 as usize;
                    let loop_addr = op.p2 as usize;
                    let after_delete = op.p5 != 0;
                    let cursor = cursors[cursor_idx].as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;
                    let has_next = if after_delete && cursor.is_valid() {
                        true
                    } else {
                        cursor.next()?
                    };
                    if has_next {
                        self.pc = loop_addr;
                    }
                }

                Opcode::Prev => {
                    let cursor_idx = op.p1 as usize;
                    let loop_addr = op.p2 as usize;
                    let cursor = cursors[cursor_idx].as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;
                    if cursor.previous()? {
                        self.pc = loop_addr;
                    }
                }

                Opcode::Last => {
                    let cursor_idx = op.p1 as usize;
                    let jump_addr = op.p2 as usize;
                    let cursor = cursors[cursor_idx].as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;
                    let has_rows = cursor.move_to_last()?;
                    if !has_rows {
                        self.pc = jump_addr;
                    }
                }

                Opcode::Column => {
                    let cursor_idx = op.p1 as usize;
                    let col_idx = op.p2 as usize;
                    let dest_reg = op.p3 as usize;
                    let cursor = cursors[cursor_idx].as_ref()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?;
                    let data: &[u8] = match cursor {
                        VdbeCursor::BTree(c) => c.data()?,
                        VdbeCursor::Sorter(s) => &s.records[s.pos],
                    };
                    let fields = sqlite3_record::decode_record(data)?;
                    let val = fields.into_iter().nth(col_idx)
                        .unwrap_or(sqlite3_record::Value::Null);
                    self.regs[dest_reg] = match val {
                        sqlite3_record::Value::Null => Mem::Null,
                        sqlite3_record::Value::Int(i) => Mem::Int(i),
                        sqlite3_record::Value::Real(f) => Mem::Real(f),
                        sqlite3_record::Value::Text(b) => {
                            let s = String::from_utf8_lossy(&b);
                            Mem::Text(Arc::from(s.as_ref()))
                        }
                        sqlite3_record::Value::Blob(b) => Mem::Blob(Arc::from(b.into_boxed_slice())),
                        sqlite3_record::Value::ZeroBlob(n) => Mem::ZeroBlob(n),
                    };
                }

                Opcode::RowId => {
                    let cursor_idx = op.p1 as usize;
                    let dest_reg = op.p2 as usize;
                    let cursor = cursors[cursor_idx].as_ref()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree()?;
                    self.regs[dest_reg] = Mem::Int(cursor.rowid()?);
                }

                Opcode::NullRow => {
                    let cursor_idx = op.p1 as usize;
                    if cursor_idx < cursors.len() {
                        cursors[cursor_idx] = None;
                    }
                }

                // ── Conditional branching ──────────────────────────────────────

                Opcode::If => {
                    if self.regs[op.p1 as usize].is_truthy() {
                        self.pc = op.p2 as usize;
                    }
                }

                Opcode::IfNot => {
                    if !self.regs[op.p1 as usize].is_truthy() {
                        self.pc = op.p2 as usize;
                    }
                }

                Opcode::IsNull => {
                    if self.regs[op.p1 as usize].is_null() {
                        self.pc = op.p2 as usize;
                    }
                }

                Opcode::NotNull => {
                    if !self.regs[op.p1 as usize].is_null() {
                        self.pc = op.p2 as usize;
                    }
                }

                // ── Mutation opcodes ───────────────────────────────────────────

                Opcode::Delete => {
                    let cursor_idx = op.p1 as usize;
                    let cursor = cursors[cursor_idx].as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;
                    cursor.delete()?;
                }

                // ── Seek opcodes ───────────────────────────────────────────────

                Opcode::SeekGe => {
                    let cursor_idx = op.p1 as usize;
                    let jump_addr = op.p2 as usize;
                    let reg_idx = op.p3 as usize;
                    
                    let key_val = self.regs[reg_idx].to_int().unwrap_or(0) as u64;
                    let cursor = cursors[cursor_idx].as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;
                    
                    let result = cursor.move_to(&key_val.to_be_bytes(), sqlite3_btree::SeekBias::Ge)?;
                    if matches!(result, sqlite3_btree::SeekResult::Empty | sqlite3_btree::SeekResult::Less) {
                        self.pc = jump_addr;
                    }
                }

                Opcode::SeekGt => {
                    let cursor_idx = op.p1 as usize;
                    let jump_addr = op.p2 as usize;
                    let reg_idx = op.p3 as usize;
                    
                    let key_val = self.regs[reg_idx].to_int().unwrap_or(0) as u64;
                    let cursor = cursors[cursor_idx].as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;
                    
                    let result = cursor.move_to(&key_val.to_be_bytes(), sqlite3_btree::SeekBias::Gt)?;
                    if matches!(result, sqlite3_btree::SeekResult::Empty | sqlite3_btree::SeekResult::Less | sqlite3_btree::SeekResult::Equal) {
                        if matches!(result, sqlite3_btree::SeekResult::Equal) {
                            if !cursor.next()? {
                                self.pc = jump_addr;
                            }
                        } else {
                            self.pc = jump_addr;
                        }
                    }
                }

                Opcode::SeekLe => {
                    let cursor_idx = op.p1 as usize;
                    let jump_addr = op.p2 as usize;
                    let reg_idx = op.p3 as usize;
                    
                    let key_val = self.regs[reg_idx].to_int().unwrap_or(0) as u64;
                    let cursor = cursors[cursor_idx].as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;
                    
                    let result = cursor.move_to(&key_val.to_be_bytes(), sqlite3_btree::SeekBias::Ge)?;
                    match result {
                        sqlite3_btree::SeekResult::Empty => { self.pc = jump_addr; }
                        sqlite3_btree::SeekResult::Greater if !cursor.previous()? => { self.pc = jump_addr; }
                        sqlite3_btree::SeekResult::Greater => {}
                        _ => {}
                    }
                }

                Opcode::SeekLt => {
                    let cursor_idx = op.p1 as usize;
                    let jump_addr = op.p2 as usize;
                    let reg_idx = op.p3 as usize;
                    
                    let key_val = self.regs[reg_idx].to_int().unwrap_or(0) as u64;
                    let cursor = cursors[cursor_idx].as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;
                    
                    let result = cursor.move_to(&key_val.to_be_bytes(), sqlite3_btree::SeekBias::Ge)?;
                    match result {
                        sqlite3_btree::SeekResult::Empty => { self.pc = jump_addr; }
                        sqlite3_btree::SeekResult::Equal | sqlite3_btree::SeekResult::Greater => {
                            if !cursor.previous()? { self.pc = jump_addr; }
                        }
                        sqlite3_btree::SeekResult::Less => {}
                    }
                }

                Opcode::InsertInt => {
                    let cursor_idx = op.p1 as usize;
                    let record_reg = op.p2 as usize;
                    let rowid = op.p3 as u64;
                    let record = if let Mem::Blob(b) = &self.regs[record_reg] {
                        b.clone()
                    } else {
                        return Err(VdbeError::Exec("record must be blob".to_string()));
                    };
                    if let Some(cursor) = &mut cursors[cursor_idx] {
                        cursor.as_btree_mut()?.insert(&rowid.to_be_bytes(), &record, false)?;
                    } else {
                        return Err(VdbeError::Exec("invalid cursor".to_string()));
                    }
                }

                // ── Limit & Loop Control ───────────────────────────────────────

                Opcode::DecrJumpZero => {
                    let reg_idx = op.p1 as usize;
                    let jump_addr = op.p2 as usize;
                    let count = match &mut self.regs[reg_idx] {
                        Mem::Int(i) => {
                            *i -= 1;
                            *i
                        }
                        _ => return Err(VdbeError::Exec("DecrJumpZero on non-integer".to_string())),
                    };
                    if count <= 0 {
                        self.pc = jump_addr;
                    }
                }

                // ── Sorting ────────────────────────────────────────────────────

                Opcode::SorterOpen => {
                    let cursor_idx = op.p1 as usize;
                    if cursor_idx >= cursors.len() {
                        return Err(VdbeError::Exec(format!("invalid cursor {}", cursor_idx)));
                    }
                    cursors[cursor_idx] = Some(VdbeCursor::Sorter(Sorter {
                        records: Vec::new(),
                        pos: 0,
                    }));
                }

                Opcode::SorterInsert => {
                    let cursor_idx = op.p1 as usize;
                    let record_reg = op.p2 as usize;
                    let record = if let Mem::Blob(b) = &self.regs[record_reg] {
                        b.to_vec()
                    } else {
                        return Err(VdbeError::Exec("SorterInsert record must be a Blob".to_string()));
                    };
                    if let Some(cursor) = &mut cursors[cursor_idx] {
                        let sorter = cursor.as_sorter_mut()?;
                        sorter.records.push(record);
                    } else {
                        return Err(VdbeError::Exec("cursor not open for SorterInsert".to_string()));
                    }
                }

                Opcode::SorterSort => {
                    let cursor_idx = op.p1 as usize;
                    let jump_addr = op.p2 as usize;
                    if let Some(cursor) = &mut cursors[cursor_idx] {
                        let sorter = cursor.as_sorter_mut()?;
                        sorter.records.sort_by(|a, b| {
                            let fields_a = sqlite3_record::decode_record(a).unwrap_or_default();
                            let fields_b = sqlite3_record::decode_record(b).unwrap_or_default();
                            for (fa, fb) in fields_a.iter().zip(fields_b.iter()) {
                                let mem_a = match fa {
                                    sqlite3_record::Value::Null => Mem::Null,
                                    sqlite3_record::Value::Int(i) => Mem::Int(*i),
                                    sqlite3_record::Value::Real(f) => Mem::Real(*f),
                                    sqlite3_record::Value::Text(t) => Mem::Text(Arc::from(String::from_utf8_lossy(t).as_ref())),
                                    sqlite3_record::Value::Blob(b) => Mem::Blob(Arc::from(b.as_slice())),
                                    sqlite3_record::Value::ZeroBlob(n) => Mem::ZeroBlob(*n),
                                };
                                let mem_b = match fb {
                                    sqlite3_record::Value::Null => Mem::Null,
                                    sqlite3_record::Value::Int(i) => Mem::Int(*i),
                                    sqlite3_record::Value::Real(f) => Mem::Real(*f),
                                    sqlite3_record::Value::Text(t) => Mem::Text(Arc::from(String::from_utf8_lossy(t).as_ref())),
                                    sqlite3_record::Value::Blob(b) => Mem::Blob(Arc::from(b.as_slice())),
                                    sqlite3_record::Value::ZeroBlob(n) => Mem::ZeroBlob(*n),
                                };
                                let cmp = mem_a.cmp(&mem_b);
                                if cmp != std::cmp::Ordering::Equal {
                                    return cmp;
                                }
                            }
                            fields_a.len().cmp(&fields_b.len())
                        });
                        sorter.pos = 0;
                        if sorter.records.is_empty() {
                            self.pc = jump_addr;
                        }
                    } else {
                        return Err(VdbeError::Exec("cursor not open for SorterSort".to_string()));
                    }
                }

                Opcode::SorterData => {
                    let cursor_idx = op.p1 as usize;
                    let dest_reg = op.p2 as usize;
                    let cursor = cursors[cursor_idx].as_ref()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?;
                    if let VdbeCursor::Sorter(sorter) = cursor {
                        let record = &sorter.records[sorter.pos];
                        self.regs[dest_reg] = Mem::Blob(Arc::from(record.as_slice()));
                    } else {
                        return Err(VdbeError::Exec("cursor is not a sorter".to_string()));
                    }
                }

                Opcode::SorterNext => {
                    let cursor_idx = op.p1 as usize;
                    let loop_addr = op.p2 as usize;
                    let cursor = cursors[cursor_idx].as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?;
                    if let VdbeCursor::Sorter(sorter) = cursor {
                        sorter.pos += 1;
                        if sorter.pos < sorter.records.len() {
                            self.pc = loop_addr;
                        }
                    } else {
                        return Err(VdbeError::Exec("cursor is not a sorter".to_string()));
                    }
                }

                // ── Functions ──────────────────────────────────────────────────

                Opcode::Function => {
                    let argc = op.p1 as usize;
                    let arg_reg = op.p2 as usize;
                    let dest_reg = op.p3 as usize;
                    let func_name = match &op.p4 {
                        P4::Text(s) => s.to_string(),
                        _ => return Err(VdbeError::Exec("Function p4 must be func name".to_string())),
                    };
                    
                    let args = if argc > 0 {
                        &self.regs[arg_reg..(arg_reg + argc)]
                    } else {
                        &[]
                    };

                    if let Some(dispatcher) = self.func_dispatcher {
                        match dispatcher(&func_name, args) {
                            Ok(res) => self.regs[dest_reg] = res,
                            Err(e) => return Err(VdbeError::Exec(format!("Function {}: {}", func_name, e))),
                        }
                    } else {
                        return Err(VdbeError::Exec(format!("no function dispatcher available for {}", func_name)));
                    }
                }

                Opcode::AggStep => {
                    let argc = op.p1 as usize;
                    let arg_reg = op.p2 as usize;
                    let dest_reg = op.p3 as usize; // Holds Mem::Agg(idx)
                    let func_name = match &op.p4 {
                        P4::Text(s) => s.to_string(),
                        _ => return Err(VdbeError::Exec("AggStep p4 must be func name".to_string())),
                    };

                    // Initialize the accumulator if it is Null.
                    if self.regs[dest_reg].is_null() {
                        if let Some(dispatcher) = self.agg_dispatcher {
                            let agg_state = dispatcher(&func_name).map_err(|e| VdbeError::Exec(format!("AggStep {}: {}", func_name, e)))?;
                            let idx = self.aggs.len();
                            self.aggs.push(agg_state);
                            self.regs[dest_reg] = Mem::Agg(idx);
                        } else {
                            return Err(VdbeError::Exec(format!("no agg dispatcher available for {}", func_name)));
                        }
                    }

                    let idx = match self.regs[dest_reg] {
                        Mem::Agg(i) => i,
                        _ => return Err(VdbeError::Exec("AggStep destination is not an aggregate".to_string())),
                    };

                    let args = if argc > 0 {
                        &self.regs[arg_reg..(arg_reg + argc)]
                    } else {
                        &[]
                    };

                    if let Some(agg_state) = self.aggs.get_mut(idx) {
                        agg_state.step(args).map_err(|e| VdbeError::Exec(format!("AggStep {}: {}", func_name, e)))?;
                    } else {
                        return Err(VdbeError::Exec("Invalid aggregate index".to_string()));
                    }
                }

                Opcode::AggFinal => {
                    let dest_reg = op.p1 as usize;
                    let func_name = match &op.p4 {
                        P4::Text(s) => s.to_string(),
                        _ => return Err(VdbeError::Exec("AggFinal p4 must be func name".to_string())),
                    };

                    if self.regs[dest_reg].is_null() {
                        // If no rows were processed, initialize to compute empty-set final value.
                        if let Some(dispatcher) = self.agg_dispatcher {
                            let mut agg_state = dispatcher(&func_name).map_err(|e| VdbeError::Exec(format!("AggFinal {}: {}", func_name, e)))?;
                            self.regs[dest_reg] = agg_state.finalize().map_err(|e| VdbeError::Exec(format!("AggFinal {}: {}", func_name, e)))?;
                        } else {
                            return Err(VdbeError::Exec(format!("no agg dispatcher available for {}", func_name)));
                        }
                    } else if let Mem::Agg(idx) = self.regs[dest_reg] {
                        if let Some(agg_state) = self.aggs.get_mut(idx) {
                            let final_val = agg_state.finalize().map_err(|e| VdbeError::Exec(format!("AggFinal {}: {}", func_name, e)))?;
                            self.regs[dest_reg] = final_val;
                        } else {
                            return Err(VdbeError::Exec("Invalid aggregate index".to_string()));
                        }
                    } else {
                        return Err(VdbeError::Exec("AggFinal destination is not an aggregate".to_string()));
                    }
                }

                Opcode::ResultRow => {
                    self.last_result_row = Some((op.p1 as usize, op.p2 as usize));
                    return Ok(StepResult::Row);
                }
                Opcode::Noop => {}
                _ => return Err(VdbeError::NotImplemented),
            }

        }
        
        self.halted = true;
        self.last_result_row = None;
        Ok(StepResult::Done)
    }

    /// Reset the VM for re-execution (bindings remain).
    pub fn reset(&mut self) -> VdbeResult<()> {
        self.pc = 0;
        self.halted = false;
        self.last_result_row = None;
        self.aggs.clear();
        for r in &mut self.regs {
            if let Mem::Agg(_) = r {
                *r = Mem::Null;
            }
        }
        Ok(())
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_int_to_real() {
        let m = Mem::Int(42);
        assert_eq!(m.to_real(), Some(42.0));
    }

    #[test]
    fn mem_null_is_null() {
        assert!(Mem::Null.is_null());
        assert!(!Mem::Int(0).is_null());
    }

    #[test]
    fn vdbe_alloc_reg() {
        let mut vm = Vdbe::new();
        let r0 = vm.alloc_reg();
        let r1 = vm.alloc_reg();
        assert_eq!(r0, 0);
        assert_eq!(r1, 1);
    }
}
