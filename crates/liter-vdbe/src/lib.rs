//! Virtual Database Engine (VDBE) for Liter-rs.
//!
//! Mirrors `vdbe.c`, `vdbeapi.c`, `vdbeaux.c`, `vdbemem.c`, `vdbesort.c`,
//! `vdbeblob.c`. Executes compiled bytecode programs against the B-tree layer.
//!
//! ## Status
//! Phase 3 — type definitions and VM skeleton only.

use liter_btree::PageKind;
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
    pub fn is_null(&self) -> bool {
        matches!(self, Mem::Null)
    }

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
    BTree(#[from] liter_btree::BTreeError),
    #[error("record error: {0}")]
    Record(#[from] liter_record::RecordError),
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
    BTree(liter_btree::BTreeCursor<'a>),
    Sorter(Sorter),
}

impl<'a> Default for VdbeCursor<'a> {
    fn default() -> Self {
        // Not used directly, but useful for arrays
        VdbeCursor::Sorter(Sorter {
            records: vec![],
            pos: 0,
        })
    }
}

impl<'a> VdbeCursor<'a> {
    pub fn as_btree(&self) -> VdbeResult<&liter_btree::BTreeCursor<'a>> {
        match self {
            VdbeCursor::BTree(c) => Ok(c),
            _ => Err(VdbeError::Exec("cursor is not a btree cursor".into())),
        }
    }

    pub fn as_btree_mut(&mut self) -> VdbeResult<&mut liter_btree::BTreeCursor<'a>> {
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

    /// Return the number of result columns defined by the VDBE program.
    pub fn num_result_cols(&self) -> usize {
        for op in &self.ops {
            if op.opcode == Opcode::ResultRow {
                return op.p2 as usize;
            }
        }
        0
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

    pub fn step<'a>(
        &mut self,
        btree: &'a liter_btree::BTree,
        cursors: &mut [Option<VdbeCursor<'a>>],
    ) -> VdbeResult<StepResult> {
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
                    if let (Some(a), Some(b)) = (
                        self.regs[op.p1 as usize].to_int(),
                        self.regs[op.p2 as usize].to_int(),
                    ) {
                        self.regs[op.p3 as usize] = Mem::Int(b + a);
                    } else {
                        return Err(VdbeError::Exec("Type mismatch in AddInt".to_string()));
                    }
                }
                Opcode::SubtractInt => {
                    if let (Some(a), Some(b)) = (
                        self.regs[op.p1 as usize].to_int(),
                        self.regs[op.p2 as usize].to_int(),
                    ) {
                        self.regs[op.p3 as usize] = Mem::Int(b - a);
                    } else {
                        return Err(VdbeError::Exec("Type mismatch in SubtractInt".to_string()));
                    }
                }
                Opcode::MultiplyInt => {
                    if let (Some(a), Some(b)) = (
                        self.regs[op.p1 as usize].to_int(),
                        self.regs[op.p2 as usize].to_int(),
                    ) {
                        self.regs[op.p3 as usize] = Mem::Int(b * a);
                    } else {
                        return Err(VdbeError::Exec("Type mismatch in MultiplyInt".to_string()));
                    }
                }
                Opcode::DivideInt => {
                    if let (Some(a), Some(b)) = (
                        self.regs[op.p1 as usize].to_int(),
                        self.regs[op.p2 as usize].to_int(),
                    ) {
                        if a == 0 {
                            return Err(VdbeError::Exec("Division by zero".to_string()));
                        }
                        self.regs[op.p3 as usize] = Mem::Int(b / a);
                    } else {
                        return Err(VdbeError::Exec("Type mismatch in DivideInt".to_string()));
                    }
                }
                Opcode::RemainderInt => {
                    if let (Some(a), Some(b)) = (
                        self.regs[op.p1 as usize].to_int(),
                        self.regs[op.p2 as usize].to_int(),
                    ) {
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
                    if eq {
                        self.pc = op.p2 as usize;
                    }
                }
                Opcode::Ne => {
                    let lhs = &self.regs[op.p3 as usize];
                    let rhs = &self.regs[op.p1 as usize];
                    let ne = match (lhs.to_real(), rhs.to_real()) {
                        (Some(a), Some(b)) => a != b,
                        _ => lhs != rhs,
                    };
                    if ne {
                        self.pc = op.p2 as usize;
                    }
                }
                Opcode::Lt => {
                    let lhs = &self.regs[op.p3 as usize];
                    let rhs = &self.regs[op.p1 as usize];
                    let lt = match (lhs.to_real(), rhs.to_real()) {
                        (Some(a), Some(b)) => a < b,
                        _ => lhs.cmp(rhs) == std::cmp::Ordering::Less,
                    };
                    if lt {
                        self.pc = op.p2 as usize;
                    }
                }
                Opcode::Le => {
                    let lhs = &self.regs[op.p3 as usize];
                    let rhs = &self.regs[op.p1 as usize];
                    let le = match (lhs.to_real(), rhs.to_real()) {
                        (Some(a), Some(b)) => a <= b,
                        _ => matches!(
                            lhs.cmp(rhs),
                            std::cmp::Ordering::Less | std::cmp::Ordering::Equal
                        ),
                    };
                    if le {
                        self.pc = op.p2 as usize;
                    }
                }
                Opcode::Gt => {
                    let lhs = &self.regs[op.p3 as usize];
                    let rhs = &self.regs[op.p1 as usize];
                    let gt = match (lhs.to_real(), rhs.to_real()) {
                        (Some(a), Some(b)) => a > b,
                        _ => lhs.cmp(rhs) == std::cmp::Ordering::Greater,
                    };
                    if gt {
                        self.pc = op.p2 as usize;
                    }
                }
                Opcode::Ge => {
                    let lhs = &self.regs[op.p3 as usize];
                    let rhs = &self.regs[op.p1 as usize];
                    let ge = match (lhs.to_real(), rhs.to_real()) {
                        (Some(a), Some(b)) => a >= b,
                        _ => matches!(
                            lhs.cmp(rhs),
                            std::cmp::Ordering::Greater | std::cmp::Ordering::Equal
                        ),
                    };
                    if ge {
                        self.pc = op.p2 as usize;
                    }
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
                            Mem::Null => liter_record::Value::Null,
                            Mem::Int(v) => liter_record::Value::Int(*v),
                            Mem::Real(v) => liter_record::Value::Real(*v),
                            Mem::Text(v) => liter_record::Value::Text(v.as_bytes().to_vec()),
                            Mem::Blob(v) => liter_record::Value::Blob(v.to_vec()),
                            Mem::ZeroBlob(n) => liter_record::Value::ZeroBlob(*n),
                            Mem::Agg(_) => {
                                return Err(VdbeError::Exec("Cannot serialize Aggregate".into()))
                            }
                        };
                        values.push(val);
                    }

                    let record = liter_record::encode_record(&values)?;
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
                        cursor
                            .as_btree_mut()?
                            .insert(&rowid.to_be_bytes(), &record, false)?;
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
                    let cursor = cursors[cursor_idx]
                        .as_mut()
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
                    let cursor = cursors[cursor_idx]
                        .as_mut()
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
                    let cursor = cursors[cursor_idx]
                        .as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;
                    if cursor.previous()? {
                        self.pc = loop_addr;
                    }
                }

                Opcode::Last => {
                    let cursor_idx = op.p1 as usize;
                    let jump_addr = op.p2 as usize;
                    let cursor = cursors[cursor_idx]
                        .as_mut()
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
                    let cursor = cursors[cursor_idx]
                        .as_ref()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?;
                    let data: &[u8] = match cursor {
                        VdbeCursor::BTree(c) => c.data()?,
                        VdbeCursor::Sorter(s) => &s.records[s.pos],
                    };
                    let fields = liter_record::decode_record(data)?;
                    let val = fields
                        .into_iter()
                        .nth(col_idx)
                        .unwrap_or(liter_record::Value::Null);
                    self.regs[dest_reg] = match val {
                        liter_record::Value::Null => Mem::Null,
                        liter_record::Value::Int(i) => Mem::Int(i),
                        liter_record::Value::Real(f) => Mem::Real(f),
                        liter_record::Value::Text(b) => {
                            let s = String::from_utf8_lossy(&b);
                            Mem::Text(Arc::from(s.as_ref()))
                        }
                        liter_record::Value::Blob(b) => Mem::Blob(Arc::from(b.into_boxed_slice())),
                        liter_record::Value::ZeroBlob(n) => Mem::ZeroBlob(n),
                    };
                }

                Opcode::RowId => {
                    let cursor_idx = op.p1 as usize;
                    let dest_reg = op.p2 as usize;
                    let cursor = cursors[cursor_idx]
                        .as_ref()
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
                    let cursor = cursors[cursor_idx]
                        .as_mut()
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
                    let cursor = cursors[cursor_idx]
                        .as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;

                    let result =
                        cursor.move_to(&key_val.to_be_bytes(), liter_btree::SeekBias::Ge)?;
                    if matches!(
                        result,
                        liter_btree::SeekResult::Empty | liter_btree::SeekResult::Less
                    ) {
                        self.pc = jump_addr;
                    }
                }

                Opcode::SeekGt => {
                    let cursor_idx = op.p1 as usize;
                    let jump_addr = op.p2 as usize;
                    let reg_idx = op.p3 as usize;

                    let key_val = self.regs[reg_idx].to_int().unwrap_or(0) as u64;
                    let cursor = cursors[cursor_idx]
                        .as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;

                    let result =
                        cursor.move_to(&key_val.to_be_bytes(), liter_btree::SeekBias::Gt)?;
                    if matches!(
                        result,
                        liter_btree::SeekResult::Empty
                            | liter_btree::SeekResult::Less
                            | liter_btree::SeekResult::Equal
                    ) {
                        if matches!(result, liter_btree::SeekResult::Equal) {
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
                    let cursor = cursors[cursor_idx]
                        .as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;

                    let result =
                        cursor.move_to(&key_val.to_be_bytes(), liter_btree::SeekBias::Ge)?;
                    match result {
                        liter_btree::SeekResult::Empty => {
                            self.pc = jump_addr;
                        }
                        liter_btree::SeekResult::Greater if !cursor.previous()? => {
                            self.pc = jump_addr;
                        }
                        liter_btree::SeekResult::Greater => {}
                        _ => {}
                    }
                }

                Opcode::SeekLt => {
                    let cursor_idx = op.p1 as usize;
                    let jump_addr = op.p2 as usize;
                    let reg_idx = op.p3 as usize;

                    let key_val = self.regs[reg_idx].to_int().unwrap_or(0) as u64;
                    let cursor = cursors[cursor_idx]
                        .as_mut()
                        .ok_or_else(|| VdbeError::Exec("invalid cursor".to_string()))?
                        .as_btree_mut()?;

                    let result =
                        cursor.move_to(&key_val.to_be_bytes(), liter_btree::SeekBias::Ge)?;
                    match result {
                        liter_btree::SeekResult::Empty => {
                            self.pc = jump_addr;
                        }
                        liter_btree::SeekResult::Equal | liter_btree::SeekResult::Greater => {
                            if !cursor.previous()? {
                                self.pc = jump_addr;
                            }
                        }
                        liter_btree::SeekResult::Less => {}
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
                        cursor
                            .as_btree_mut()?
                            .insert(&rowid.to_be_bytes(), &record, false)?;
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
                        _ => {
                            return Err(VdbeError::Exec("DecrJumpZero on non-integer".to_string()))
                        }
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
                        return Err(VdbeError::Exec(
                            "SorterInsert record must be a Blob".to_string(),
                        ));
                    };
                    if let Some(cursor) = &mut cursors[cursor_idx] {
                        let sorter = cursor.as_sorter_mut()?;
                        sorter.records.push(record);
                    } else {
                        return Err(VdbeError::Exec(
                            "cursor not open for SorterInsert".to_string(),
                        ));
                    }
                }

                Opcode::SorterSort => {
                    let cursor_idx = op.p1 as usize;
                    let jump_addr = op.p2 as usize;
                    if let Some(cursor) = &mut cursors[cursor_idx] {
                        let sorter = cursor.as_sorter_mut()?;
                        sorter.records.sort_by(|a, b| {
                            let fields_a = liter_record::decode_record(a).unwrap_or_default();
                            let fields_b = liter_record::decode_record(b).unwrap_or_default();
                            for (fa, fb) in fields_a.iter().zip(fields_b.iter()) {
                                let mem_a = match fa {
                                    liter_record::Value::Null => Mem::Null,
                                    liter_record::Value::Int(i) => Mem::Int(*i),
                                    liter_record::Value::Real(f) => Mem::Real(*f),
                                    liter_record::Value::Text(t) => {
                                        Mem::Text(Arc::from(String::from_utf8_lossy(t).as_ref()))
                                    }
                                    liter_record::Value::Blob(b) => {
                                        Mem::Blob(Arc::from(b.as_slice()))
                                    }
                                    liter_record::Value::ZeroBlob(n) => Mem::ZeroBlob(*n),
                                };
                                let mem_b = match fb {
                                    liter_record::Value::Null => Mem::Null,
                                    liter_record::Value::Int(i) => Mem::Int(*i),
                                    liter_record::Value::Real(f) => Mem::Real(*f),
                                    liter_record::Value::Text(t) => {
                                        Mem::Text(Arc::from(String::from_utf8_lossy(t).as_ref()))
                                    }
                                    liter_record::Value::Blob(b) => {
                                        Mem::Blob(Arc::from(b.as_slice()))
                                    }
                                    liter_record::Value::ZeroBlob(n) => Mem::ZeroBlob(*n),
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
                        return Err(VdbeError::Exec(
                            "cursor not open for SorterSort".to_string(),
                        ));
                    }
                }

                Opcode::SorterData => {
                    let cursor_idx = op.p1 as usize;
                    let dest_reg = op.p2 as usize;
                    let cursor = cursors[cursor_idx]
                        .as_ref()
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
                    let cursor = cursors[cursor_idx]
                        .as_mut()
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
                        _ => {
                            return Err(VdbeError::Exec(
                                "Function p4 must be func name".to_string(),
                            ))
                        }
                    };

                    let args = if argc > 0 {
                        &self.regs[arg_reg..(arg_reg + argc)]
                    } else {
                        &[]
                    };

                    if let Some(dispatcher) = self.func_dispatcher {
                        match dispatcher(&func_name, args) {
                            Ok(res) => self.regs[dest_reg] = res,
                            Err(e) => {
                                return Err(VdbeError::Exec(format!(
                                    "Function {}: {}",
                                    func_name, e
                                )))
                            }
                        }
                    } else {
                        return Err(VdbeError::Exec(format!(
                            "no function dispatcher available for {}",
                            func_name
                        )));
                    }
                }

                Opcode::AggStep => {
                    let argc = op.p1 as usize;
                    let arg_reg = op.p2 as usize;
                    let dest_reg = op.p3 as usize; // Holds Mem::Agg(idx)
                    let func_name = match &op.p4 {
                        P4::Text(s) => s.to_string(),
                        _ => {
                            return Err(VdbeError::Exec("AggStep p4 must be func name".to_string()))
                        }
                    };

                    // Initialize the accumulator if it is Null.
                    if self.regs[dest_reg].is_null() {
                        if let Some(dispatcher) = self.agg_dispatcher {
                            let agg_state = dispatcher(&func_name).map_err(|e| {
                                VdbeError::Exec(format!("AggStep {}: {}", func_name, e))
                            })?;
                            let idx = self.aggs.len();
                            self.aggs.push(agg_state);
                            self.regs[dest_reg] = Mem::Agg(idx);
                        } else {
                            return Err(VdbeError::Exec(format!(
                                "no agg dispatcher available for {}",
                                func_name
                            )));
                        }
                    }

                    let idx = match self.regs[dest_reg] {
                        Mem::Agg(i) => i,
                        _ => {
                            return Err(VdbeError::Exec(
                                "AggStep destination is not an aggregate".to_string(),
                            ))
                        }
                    };

                    let args = if argc > 0 {
                        &self.regs[arg_reg..(arg_reg + argc)]
                    } else {
                        &[]
                    };

                    if let Some(agg_state) = self.aggs.get_mut(idx) {
                        agg_state.step(args).map_err(|e| {
                            VdbeError::Exec(format!("AggStep {}: {}", func_name, e))
                        })?;
                    } else {
                        return Err(VdbeError::Exec("Invalid aggregate index".to_string()));
                    }
                }

                Opcode::AggFinal => {
                    let dest_reg = op.p1 as usize;
                    let func_name = match &op.p4 {
                        P4::Text(s) => s.to_string(),
                        _ => {
                            return Err(VdbeError::Exec(
                                "AggFinal p4 must be func name".to_string(),
                            ))
                        }
                    };

                    if self.regs[dest_reg].is_null() {
                        // If no rows were processed, initialize to compute empty-set final value.
                        if let Some(dispatcher) = self.agg_dispatcher {
                            let mut agg_state = dispatcher(&func_name).map_err(|e| {
                                VdbeError::Exec(format!("AggFinal {}: {}", func_name, e))
                            })?;
                            self.regs[dest_reg] = agg_state.finalize().map_err(|e| {
                                VdbeError::Exec(format!("AggFinal {}: {}", func_name, e))
                            })?;
                        } else {
                            return Err(VdbeError::Exec(format!(
                                "no agg dispatcher available for {}",
                                func_name
                            )));
                        }
                    } else if let Mem::Agg(idx) = self.regs[dest_reg] {
                        if let Some(agg_state) = self.aggs.get_mut(idx) {
                            let final_val = agg_state.finalize().map_err(|e| {
                                VdbeError::Exec(format!("AggFinal {}: {}", func_name, e))
                            })?;
                            self.regs[dest_reg] = final_val;
                        } else {
                            return Err(VdbeError::Exec("Invalid aggregate index".to_string()));
                        }
                    } else {
                        return Err(VdbeError::Exec(
                            "AggFinal destination is not an aggregate".to_string(),
                        ));
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

    // ── Test helpers ────────────────────────────────────────────────────────

    /// Insert `rows` (rowid, text) pairs into the table rooted at `pgno`,
    /// wrapping the write in its own transaction.
    fn insert_rows(btree: &liter_btree::BTree, pgno: u32, rows: &[(i64, &str)]) {
        btree.begin_write().unwrap();
        let mut vm = Vdbe::with_capacity(rows.len() * 4 + 2, 3);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::OpenWrite,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        for (rowid, text) in rows {
            // NOTE: register values must be loaded via opcodes (not by poking
            // `vm.regs` directly) because all these ops execute later, in a
            // single `step()` call, after every iteration has already been
            // emitted -- a direct poke here would only leave the *last*
            // iteration's value in place for every row.
            vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: *rowid as i32,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::String8,
                p1: 0,
                p2: 1,
                p3: 0,
                p4: P4::Text(Arc::from(*text)),
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::MakeRecord,
                p1: 1,
                p2: 1,
                p3: 2,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::Insert,
                p1: 0,
                p2: 2,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
        }
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(btree, &mut cursors).unwrap(), StepResult::Done);
        btree.commit().unwrap();
    }

    /// Run a two-operand comparison opcode with `lhs` in reg0 and `rhs` in
    /// reg1, returning whether the conditional jump was taken.
    fn eval_cond_jump(op: Opcode, lhs: Mem, rhs: Mem) -> bool {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(5, 2);
        vm.regs[0] = lhs;
        vm.regs[1] = rhs;
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: op,
            p1: 1,
            p2: 3,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Integer,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Integer,
            p1: 1,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        matches!(vm.current_result_row().unwrap()[0], Mem::Int(1))
    }

    /// Run a seek opcode against `pgno` seeking `key`; returns whether the
    /// "not found" jump was taken.
    fn seek_jumped(btree: &liter_btree::BTree, pgno: u32, op: Opcode, key: i64) -> bool {
        let mut vm = Vdbe::with_capacity(6, 1);
        vm.regs[0] = Mem::Int(key);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: op,
            p1: 0,
            p2: 4,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Integer,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Integer,
            p1: 1,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(btree, &mut cursors).unwrap(), StepResult::Row);
        matches!(vm.current_result_row().unwrap()[0], Mem::Int(1))
    }

    fn test_func_dispatcher(name: &str, args: &[Mem]) -> Result<Mem, String> {
        match name {
            "double" => match args.first() {
                Some(Mem::Int(i)) => Ok(Mem::Int(i * 2)),
                _ => Err("expected int arg".to_string()),
            },
            "boom" => Err("kaboom".to_string()),
            _ => Err(format!("unknown function {name}")),
        }
    }

    #[derive(Debug, Default)]
    struct SumAgg {
        total: i64,
    }
    impl AggregateState for SumAgg {
        fn step(&mut self, args: &[Mem]) -> Result<(), String> {
            match args.first() {
                Some(Mem::Int(i)) => {
                    self.total += i;
                    Ok(())
                }
                _ => Err("expected int".to_string()),
            }
        }
        fn finalize(&mut self) -> Result<Mem, String> {
            Ok(Mem::Int(self.total))
        }
    }

    #[derive(Debug)]
    struct FailFinalAgg;
    impl AggregateState for FailFinalAgg {
        fn step(&mut self, _args: &[Mem]) -> Result<(), String> {
            Ok(())
        }
        fn finalize(&mut self) -> Result<Mem, String> {
            Err("finalize boom".to_string())
        }
    }

    fn test_agg_dispatcher(name: &str) -> Result<Box<dyn AggregateState>, String> {
        match name {
            "sum" => Ok(Box::new(SumAgg::default())),
            "failfinal" => Ok(Box::new(FailFinalAgg)),
            "fail" => Err("no such aggregate".to_string()),
            _ => Err(format!("unknown aggregate {name}")),
        }
    }

    // ── Mem ─────────────────────────────────────────────────────────────────

    #[test]
    fn mem_is_truthy_variants() {
        assert!(!Mem::Null.is_truthy());
        assert!(Mem::Int(1).is_truthy());
        assert!(!Mem::Int(0).is_truthy());
        assert!(Mem::Real(1.5).is_truthy());
        assert!(!Mem::Real(0.0).is_truthy());
        assert!(Mem::Text(Arc::from("x")).is_truthy());
        assert!(!Mem::Text(Arc::from("")).is_truthy());
        assert!(Mem::Blob(Arc::from(vec![1u8])).is_truthy());
        assert!(!Mem::Blob(Arc::from(Vec::<u8>::new())).is_truthy());
        assert!(!Mem::ZeroBlob(5).is_truthy());
        assert!(!Mem::Agg(0).is_truthy());
    }

    #[test]
    fn mem_to_int_and_to_real_conversions() {
        assert_eq!(Mem::Int(7).to_int(), Some(7));
        assert_eq!(Mem::Real(7.9).to_int(), Some(7));
        assert_eq!(Mem::Null.to_int(), None);
        assert_eq!(Mem::Text(Arc::from("x")).to_int(), None);
        assert_eq!(Mem::Real(2.5).to_real(), Some(2.5));
        assert_eq!(Mem::Int(3).to_real(), Some(3.0));
        assert_eq!(Mem::Text(Arc::from("x")).to_real(), None);
    }

    #[test]
    fn mem_cmp_type_class_ordering() {
        use std::cmp::Ordering;
        assert_eq!(Mem::Null.cmp(&Mem::Int(0)), Ordering::Less);
        assert_eq!(Mem::Int(0).cmp(&Mem::Text(Arc::from(""))), Ordering::Less);
        assert_eq!(
            Mem::Text(Arc::from("")).cmp(&Mem::Blob(Arc::from(vec![]))),
            Ordering::Less
        );
        assert_eq!(
            Mem::Blob(Arc::from(vec![1u8])).cmp(&Mem::Agg(0)),
            Ordering::Less
        );
        assert_eq!(
            Mem::Blob(Arc::from(vec![1u8])).cmp(&Mem::Null),
            Ordering::Greater
        );
    }

    #[test]
    fn mem_cmp_same_class() {
        use std::cmp::Ordering;
        assert_eq!(Mem::Null.cmp(&Mem::Null), Ordering::Equal);
        assert_eq!(Mem::Int(1).cmp(&Mem::Int(2)), Ordering::Less);
        assert_eq!(Mem::Real(2.0).cmp(&Mem::Real(1.0)), Ordering::Greater);
        assert_eq!(Mem::Int(2).cmp(&Mem::Real(1.5)), Ordering::Greater);
        assert_eq!(Mem::Real(1.5).cmp(&Mem::Int(2)), Ordering::Less);
        assert_eq!(
            Mem::Text(Arc::from("abc")).cmp(&Mem::Text(Arc::from("abd"))),
            Ordering::Less
        );
        assert_eq!(
            Mem::Blob(Arc::from(vec![1u8, 2])).cmp(&Mem::Blob(Arc::from(vec![1u8, 3]))),
            Ordering::Less
        );
        assert_eq!(Mem::ZeroBlob(3).cmp(&Mem::ZeroBlob(5)), Ordering::Less);
        assert_eq!(Mem::Agg(0).cmp(&Mem::Agg(1)), Ordering::Equal);
    }

    #[test]
    fn mem_cmp_blob_zeroblob_both_directions() {
        use std::cmp::Ordering;
        assert_eq!(
            Mem::Blob(Arc::from(vec![0u8, 0, 0])).cmp(&Mem::ZeroBlob(3)),
            Ordering::Equal
        );
        assert_eq!(
            Mem::Blob(Arc::from(vec![0u8, 0])).cmp(&Mem::ZeroBlob(3)),
            Ordering::Less
        );
        assert_eq!(
            Mem::Blob(Arc::from(vec![0u8, 1])).cmp(&Mem::ZeroBlob(3)),
            Ordering::Greater
        );
        assert_eq!(
            Mem::ZeroBlob(3).cmp(&Mem::Blob(Arc::from(vec![0u8, 0, 0]))),
            Ordering::Equal
        );
        assert_eq!(
            Mem::ZeroBlob(3).cmp(&Mem::Blob(Arc::from(vec![0u8, 0]))),
            Ordering::Greater
        );
        assert_eq!(
            Mem::ZeroBlob(3).cmp(&Mem::Blob(Arc::from(vec![0u8, 1]))),
            Ordering::Less
        );
    }

    // ── VdbeCursor ──────────────────────────────────────────────────────────

    #[test]
    fn vdbe_cursor_default_is_sorter_and_accessors() {
        let cur: VdbeCursor = VdbeCursor::default();
        assert!(cur.as_btree().is_err());
        let mut cur = cur;
        assert!(cur.as_btree_mut().is_err());
        assert!(cur.as_sorter_mut().is_ok());
    }

    #[test]
    fn vdbe_cursor_btree_variant_accessors() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();
        let bcursor = btree.cursor(pgno, false).unwrap();
        let mut cur = VdbeCursor::BTree(bcursor);
        assert!(cur.as_btree().is_ok());
        assert!(cur.as_btree_mut().is_ok());
        assert!(cur.as_sorter_mut().is_err());
    }

    // ── Vdbe basics ─────────────────────────────────────────────────────────

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

    #[test]
    fn vdbe_default_and_current_result_row_none() {
        let vm = Vdbe::default();
        assert_eq!(vm.ops.len(), 0);
        assert!(vm.current_result_row().is_none());
    }

    // ── Control flow ────────────────────────────────────────────────────────

    #[test]
    fn vdbe_init_jumps_to_p2() {
        let mut vm = Vdbe::with_capacity(4, 1);
        let btree = liter_btree::BTree::new_in_memory();
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::Init,
            p1: 0,
            p2: 2,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Integer,
            p1: 99,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        }); // skipped
        vm.emit(VdbeOp {
            opcode: Opcode::Integer,
            p1: 7,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        assert_eq!(vm.current_result_row().unwrap()[0], Mem::Int(7));
    }

    #[test]
    fn vdbe_gosub_and_return() {
        let mut vm = Vdbe::with_capacity(6, 1);
        let btree = liter_btree::BTree::new_in_memory();
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::Gosub,
            p1: 0,
            p2: 4,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Integer,
            p1: 42,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Integer,
            p1: 1,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Return,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        assert_eq!(vm.current_result_row().unwrap()[0], Mem::Int(42));
    }

    #[test]
    fn vdbe_return_without_gosub_errors() {
        let mut vm = Vdbe::with_capacity(1, 0);
        let btree = liter_btree::BTree::new_in_memory();
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::Return,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err = vm.step(&btree, &mut cursors).unwrap_err();
        assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("Return without Gosub")));
    }

    // ── Register manipulation ───────────────────────────────────────────────

    #[test]
    fn vdbe_move_and_copy() {
        let mut vm = Vdbe::with_capacity(4, 3);
        let btree = liter_btree::BTree::new_in_memory();
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::Integer,
            p1: 5,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Copy,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Move,
            p1: 0,
            p2: 2,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 3,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.step(&btree, &mut cursors).unwrap();
        let row = vm.current_result_row().unwrap();
        assert_eq!(row[0], Mem::Null); // moved out of r0
        assert_eq!(row[1], Mem::Int(5)); // copied into r1
        assert_eq!(row[2], Mem::Int(5)); // moved into r2
    }

    #[test]
    fn vdbe_real_and_string8_invalid_p4_errors() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(1, 1);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::Real,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err = vm.step(&btree, &mut cursors).unwrap_err();
        assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("Real")));

        let mut vm2 = Vdbe::with_capacity(1, 1);
        let mut cursors2: Vec<Option<VdbeCursor>> = vec![];
        vm2.emit(VdbeOp {
            opcode: Opcode::String8,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err2 = vm2.step(&btree, &mut cursors2).unwrap_err();
        assert!(matches!(err2, VdbeError::Exec(msg) if msg.contains("String8")));
    }

    // ── Integer arithmetic ──────────────────────────────────────────────────

    #[test]
    fn vdbe_arithmetic_ops_correct_values() {
        let mut vm = Vdbe::with_capacity(10, 3);
        let btree = liter_btree::BTree::new_in_memory();
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::Integer,
            p1: 20,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Integer,
            p1: 6,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::SubtractInt,
            p1: 1,
            p2: 0,
            p3: 2,
            p4: P4::None,
            p5: 0,
        }); // r2 = r0 - r1 = 14
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 2,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::MultiplyInt,
            p1: 1,
            p2: 0,
            p3: 2,
            p4: P4::None,
            p5: 0,
        }); // r2 = r0 * r1 = 120
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 2,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::DivideInt,
            p1: 1,
            p2: 0,
            p3: 2,
            p4: P4::None,
            p5: 0,
        }); // r2 = r0 / r1 = 3
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 2,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::RemainderInt,
            p1: 1,
            p2: 0,
            p3: 2,
            p4: P4::None,
            p5: 0,
        }); // r2 = r0 % r1 = 2
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 2,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        for expected in [14, 120, 3, 2] {
            assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
            assert_eq!(vm.current_result_row().unwrap()[0], Mem::Int(expected));
        }
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
    }

    #[test]
    fn vdbe_int_arith_type_mismatch_and_zero_division_errors() {
        let cases: [(Opcode, &str); 5] = [
            (Opcode::AddInt, "AddInt"),
            (Opcode::SubtractInt, "SubtractInt"),
            (Opcode::MultiplyInt, "MultiplyInt"),
            (Opcode::DivideInt, "DivideInt"),
            (Opcode::RemainderInt, "RemainderInt"),
        ];
        let btree = liter_btree::BTree::new_in_memory();
        for (op, name) in cases {
            let mut vm = Vdbe::with_capacity(2, 3);
            let mut cursors: Vec<Option<VdbeCursor>> = vec![];
            // r1 = 5, r0 stays Null -> type mismatch when read as int.
            vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: 5,
                p2: 1,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: op,
                p1: 0,
                p2: 1,
                p3: 2,
                p4: P4::None,
                p5: 0,
            });
            let err = vm.step(&btree, &mut cursors).unwrap_err();
            match err {
                VdbeError::Exec(msg) => assert!(msg.contains(name), "unexpected message: {msg}"),
                other => panic!("expected Exec error, got {other:?}"),
            }
        }

        for op in [Opcode::DivideInt, Opcode::RemainderInt] {
            let mut vm = Vdbe::with_capacity(2, 3);
            let mut cursors: Vec<Option<VdbeCursor>> = vec![];
            vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: 0,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            }); // r0 = 0 (divisor)
            vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: 10,
                p2: 1,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: op,
                p1: 0,
                p2: 1,
                p3: 2,
                p4: P4::None,
                p5: 0,
            });
            let err = vm.step(&btree, &mut cursors).unwrap_err();
            assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("zero")));
        }
    }

    // ── Comparisons ─────────────────────────────────────────────────────────

    #[test]
    fn vdbe_comparison_opcodes_all() {
        assert!(eval_cond_jump(Opcode::Eq, Mem::Int(5), Mem::Int(5)));
        assert!(!eval_cond_jump(Opcode::Eq, Mem::Int(5), Mem::Int(6)));
        assert!(eval_cond_jump(Opcode::Eq, Mem::Real(1.0), Mem::Int(1)));
        assert!(eval_cond_jump(
            Opcode::Eq,
            Mem::Text(Arc::from("a")),
            Mem::Text(Arc::from("a"))
        ));
        assert!(!eval_cond_jump(
            Opcode::Eq,
            Mem::Text(Arc::from("a")),
            Mem::Text(Arc::from("b"))
        ));

        assert!(eval_cond_jump(Opcode::Ne, Mem::Int(5), Mem::Int(6)));
        assert!(!eval_cond_jump(Opcode::Ne, Mem::Int(5), Mem::Int(5)));
        assert!(eval_cond_jump(
            Opcode::Ne,
            Mem::Text(Arc::from("a")),
            Mem::Text(Arc::from("b"))
        ));

        assert!(eval_cond_jump(Opcode::Lt, Mem::Int(3), Mem::Int(5)));
        assert!(!eval_cond_jump(Opcode::Lt, Mem::Int(5), Mem::Int(3)));
        assert!(eval_cond_jump(
            Opcode::Lt,
            Mem::Text(Arc::from("a")),
            Mem::Text(Arc::from("b"))
        ));

        assert!(eval_cond_jump(Opcode::Le, Mem::Int(5), Mem::Int(5)));
        assert!(!eval_cond_jump(Opcode::Le, Mem::Int(6), Mem::Int(5)));
        assert!(eval_cond_jump(
            Opcode::Le,
            Mem::Blob(Arc::from(vec![1u8])),
            Mem::Blob(Arc::from(vec![1u8]))
        ));
        assert!(eval_cond_jump(
            Opcode::Le,
            Mem::Text(Arc::from("a")),
            Mem::Text(Arc::from("b"))
        ));
        assert!(!eval_cond_jump(
            Opcode::Le,
            Mem::Text(Arc::from("b")),
            Mem::Text(Arc::from("a"))
        ));

        assert!(eval_cond_jump(Opcode::Gt, Mem::Int(6), Mem::Int(5)));
        assert!(!eval_cond_jump(Opcode::Gt, Mem::Int(5), Mem::Int(6)));
        assert!(eval_cond_jump(
            Opcode::Gt,
            Mem::Text(Arc::from("b")),
            Mem::Text(Arc::from("a"))
        ));

        assert!(eval_cond_jump(Opcode::Ge, Mem::Int(5), Mem::Int(5)));
        assert!(!eval_cond_jump(Opcode::Ge, Mem::Int(4), Mem::Int(5)));
        assert!(eval_cond_jump(
            Opcode::Ge,
            Mem::Blob(Arc::from(vec![2u8])),
            Mem::Blob(Arc::from(vec![1u8]))
        ));
        assert!(eval_cond_jump(
            Opcode::Ge,
            Mem::Blob(Arc::from(vec![1u8])),
            Mem::Blob(Arc::from(vec![1u8]))
        ));
        assert!(!eval_cond_jump(
            Opcode::Ge,
            Mem::Text(Arc::from("a")),
            Mem::Text(Arc::from("b"))
        ));
    }

    // ── Schema / cursor open ────────────────────────────────────────────────

    #[test]
    fn vdbe_create_table_and_open_cursor_errors() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let mut vm = Vdbe::with_capacity(2, 1);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::CreateTable,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        let pgno = match vm.current_result_row().unwrap()[0] {
            Mem::Int(i) => i as u32,
            _ => panic!("expected int page number"),
        };
        assert!(pgno > 0);
        btree.commit().unwrap();

        let mut vm2 = Vdbe::with_capacity(1, 0);
        let mut cursors2: Vec<Option<VdbeCursor>> = vec![];
        vm2.emit(VdbeOp {
            opcode: Opcode::OpenWrite,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err = vm2.step(&btree, &mut cursors2).unwrap_err();
        assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("invalid cursor")));

        let mut vm3 = Vdbe::with_capacity(1, 0);
        let mut cursors3: Vec<Option<VdbeCursor>> = vec![];
        vm3.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err3 = vm3.step(&btree, &mut cursors3).unwrap_err();
        assert!(matches!(err3, VdbeError::Exec(_)));

        let mut vm4 = Vdbe::with_capacity(2, 0);
        let mut cursors4: Vec<Option<VdbeCursor>> = vec![None];
        vm4.emit(VdbeOp {
            opcode: Opcode::OpenWrite,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm4.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm4.step(&btree, &mut cursors4).unwrap(), StepResult::Done);
        assert!(cursors4[0].is_some());
    }

    #[test]
    fn vdbe_new_rowid_on_empty_and_missing_cursor() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();

        let mut vm = Vdbe::with_capacity(3, 1);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::OpenWrite,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::NewRowid,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        assert_eq!(vm.current_result_row().unwrap()[0], Mem::Int(1));

        let mut vm2 = Vdbe::with_capacity(1, 1);
        let mut cursors2: Vec<Option<VdbeCursor>> = vec![None];
        vm2.emit(VdbeOp {
            opcode: Opcode::NewRowid,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err = vm2.step(&btree, &mut cursors2).unwrap_err();
        assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("cursor not open")));
    }

    // ── MakeRecord ──────────────────────────────────────────────────────────

    #[test]
    fn vdbe_make_record_all_value_types() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(2, 6);
        vm.regs[0] = Mem::Null;
        vm.regs[1] = Mem::Int(42);
        vm.regs[2] = Mem::Real(1.5);
        vm.regs[3] = Mem::Text(Arc::from("hi"));
        vm.regs[4] = Mem::Blob(Arc::from(vec![9u8, 8]));
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::MakeRecord,
            p1: 0,
            p2: 5,
            p3: 5,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 5,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        match &vm.current_result_row().unwrap()[0] {
            Mem::Blob(b) => assert!(!b.is_empty()),
            other => panic!("expected blob, got {other:?}"),
        }
    }

    /// `ZeroBlob` is a recognized `Mem` variant translated by `MakeRecord`
    /// into `liter_record::Value::ZeroBlob`, but the record codec does not
    /// yet support encoding that value; this exercises the translation
    /// branch and confirms the resulting error propagates through `step()`.
    #[test]
    fn vdbe_make_record_zero_blob_translates_but_encoder_rejects_it() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(1, 2);
        vm.regs[0] = Mem::ZeroBlob(4);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::MakeRecord,
            p1: 0,
            p2: 1,
            p3: 1,
            p4: P4::None,
            p5: 0,
        });
        let err = vm.step(&btree, &mut cursors).unwrap_err();
        assert!(matches!(err, VdbeError::Record(_)));
    }

    #[test]
    fn vdbe_make_record_agg_errors() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(1, 2);
        vm.regs[0] = Mem::Agg(0);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::MakeRecord,
            p1: 0,
            p2: 1,
            p3: 1,
            p4: P4::None,
            p5: 0,
        });
        let err = vm.step(&btree, &mut cursors).unwrap_err();
        assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("Aggregate")));
    }

    // ── Insert / Column / RowId / scan ──────────────────────────────────────

    /// `Column` decodes every `liter_record::Value` variant it can produce
    /// (Null, Int, Real, Text, Blob) back into the matching `Mem` variant.
    /// `ZeroBlob` is deliberately not exercised here: the record codec never
    /// decodes that serial type (see `vdbe_make_record_zero_blob_translates_but_encoder_rejects_it`),
    /// so that arm of `Column`'s match is unreachable through the public API.
    #[test]
    fn vdbe_column_decodes_all_value_types() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();

        btree.begin_write().unwrap();
        {
            let mut vm = Vdbe::with_capacity(10, 7);
            let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
            vm.emit(VdbeOp {
                opcode: Opcode::OpenWrite,
                p1: 0,
                p2: pgno as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: 1,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            }); // r0 = rowid 1
            vm.emit(VdbeOp {
                opcode: Opcode::Null,
                p1: 0,
                p2: 1,
                p3: 0,
                p4: P4::None,
                p5: 0,
            }); // r1 = Null
            vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: 7,
                p2: 2,
                p3: 0,
                p4: P4::None,
                p5: 0,
            }); // r2 = Int(7)
            vm.emit(VdbeOp {
                opcode: Opcode::Real,
                p1: 0,
                p2: 3,
                p3: 0,
                p4: P4::Real(2.5),
                p5: 0,
            }); // r3 = Real(2.5)
            vm.emit(VdbeOp {
                opcode: Opcode::String8,
                p1: 0,
                p2: 4,
                p3: 0,
                p4: P4::Text(Arc::from("hi")),
                p5: 0,
            }); // r4 = Text("hi")
            vm.regs[5] = Mem::Blob(Arc::from(vec![9u8, 8])); // r5 = Blob; no opcode ever writes r5.
            vm.emit(VdbeOp {
                opcode: Opcode::MakeRecord,
                p1: 1,
                p2: 5,
                p3: 6,
                p4: P4::None,
                p5: 0,
            }); // record from r1..r5 into r6
            vm.emit(VdbeOp {
                opcode: Opcode::Insert,
                p1: 0,
                p2: 6,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::Halt,
                p1: 0,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
        }
        btree.commit().unwrap();

        let mut vm = Vdbe::with_capacity(10, 6);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Rewind,
            p1: 0,
            p2: 99,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        for (col_idx, dest) in [(0, 1), (1, 2), (2, 3), (3, 4), (4, 5)] {
            vm.emit(VdbeOp {
                opcode: Opcode::Column,
                p1: 0,
                p2: col_idx,
                p3: dest,
                p4: P4::None,
                p5: 0,
            });
        }
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 1,
            p2: 5,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        let row = vm.current_result_row().unwrap();
        assert_eq!(row[0], Mem::Null);
        assert_eq!(row[1], Mem::Int(7));
        assert_eq!(row[2], Mem::Real(2.5));
        assert_eq!(row[3], Mem::Text(Arc::from("hi")));
        assert_eq!(row[4], Mem::Blob(Arc::from(vec![9u8, 8])));
    }

    #[test]
    fn vdbe_insert_column_rowid_and_scan() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();
        insert_rows(&btree, pgno, &[(1, "a"), (2, "b"), (3, "c")]);

        let mut vm = Vdbe::with_capacity(20, 3);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let rewind_addr = vm.emit(VdbeOp {
            opcode: Opcode::Rewind,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let loop_start = vm.emit(VdbeOp {
            opcode: Opcode::Column,
            p1: 0,
            p2: 0,
            p3: 1,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::RowId,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 2,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Next,
            p1: 0,
            p2: loop_start as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let done_addr = vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.ops[rewind_addr].p2 = done_addr as i32;

        let mut got = Vec::new();
        while let StepResult::Row = vm.step(&btree, &mut cursors).unwrap() {
            let row = vm.current_result_row().unwrap();
            let rowid = match row[0] {
                Mem::Int(i) => i,
                _ => panic!("expected rowid int"),
            };
            assert!(matches!(row[1], Mem::Text(_)));
            got.push(rowid);
        }
        assert_eq!(got, vec![1, 2, 3]);
    }

    #[test]
    fn vdbe_insert_error_paths() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();

        {
            let mut vm = Vdbe::with_capacity(3, 2);
            let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
            vm.emit(VdbeOp {
                opcode: Opcode::OpenWrite,
                p1: 0,
                p2: pgno as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.regs[1] = Mem::Blob(Arc::from(vec![1u8]));
            vm.emit(VdbeOp {
                opcode: Opcode::Insert,
                p1: 0,
                p2: 1,
                p3: 0,
                p4: P4::None,
                p5: 0,
            }); // rowid_reg=0 is Null
            let err = vm.step(&btree, &mut cursors).unwrap_err();
            assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("rowid must be integer")));
        }
        {
            let mut vm = Vdbe::with_capacity(3, 2);
            let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
            vm.emit(VdbeOp {
                opcode: Opcode::OpenWrite,
                p1: 0,
                p2: pgno as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.regs[0] = Mem::Int(5);
            vm.emit(VdbeOp {
                opcode: Opcode::Insert,
                p1: 0,
                p2: 1,
                p3: 0,
                p4: P4::None,
                p5: 0,
            }); // record_reg=1 is Null
            let err = vm.step(&btree, &mut cursors).unwrap_err();
            assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("record must be blob")));
        }
        {
            let mut vm = Vdbe::with_capacity(1, 2);
            let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
            vm.regs[0] = Mem::Int(5);
            vm.regs[1] = Mem::Blob(Arc::from(vec![1u8]));
            vm.emit(VdbeOp {
                opcode: Opcode::Insert,
                p1: 0,
                p2: 1,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            let err = vm.step(&btree, &mut cursors).unwrap_err();
            assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("invalid cursor")));
        }
    }

    #[test]
    fn vdbe_close_and_null_row_clear_cursor() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();

        let mut vm = Vdbe::with_capacity(3, 0);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Close,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
        assert!(cursors[0].is_none());

        let mut vm2 = Vdbe::with_capacity(3, 0);
        let mut cursors2: Vec<Option<VdbeCursor>> = vec![None];
        vm2.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm2.emit(VdbeOp {
            opcode: Opcode::NullRow,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm2.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm2.step(&btree, &mut cursors2).unwrap(), StepResult::Done);
        assert!(cursors2[0].is_none());
    }

    // ── Conditional branches ────────────────────────────────────────────────

    #[test]
    fn vdbe_if_ifnot_isnull_notnull() {
        let btree = liter_btree::BTree::new_in_memory();

        fn run(op: Opcode, reg_val: Mem) -> bool {
            let btree = liter_btree::BTree::new_in_memory();
            let mut vm = Vdbe::with_capacity(4, 1);
            vm.regs[0] = reg_val;
            let mut cursors: Vec<Option<VdbeCursor>> = vec![];
            vm.emit(VdbeOp {
                opcode: op,
                p1: 0,
                p2: 3,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: 0,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::ResultRow,
                p1: 0,
                p2: 1,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: 1,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::ResultRow,
                p1: 0,
                p2: 1,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
            matches!(vm.current_result_row().unwrap()[0], Mem::Int(1))
        }
        let _ = &btree;

        assert!(run(Opcode::If, Mem::Int(1)));
        assert!(!run(Opcode::If, Mem::Int(0)));
        assert!(run(Opcode::IfNot, Mem::Int(0)));
        assert!(!run(Opcode::IfNot, Mem::Int(1)));
        assert!(run(Opcode::IsNull, Mem::Null));
        assert!(!run(Opcode::IsNull, Mem::Int(1)));
        assert!(run(Opcode::NotNull, Mem::Int(1)));
        assert!(!run(Opcode::NotNull, Mem::Null));
    }

    // ── Delete ──────────────────────────────────────────────────────────────

    #[test]
    fn vdbe_delete_opcode_and_invalid_cursor() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();
        insert_rows(&btree, pgno, &[(1, "a")]);

        btree.begin_write().unwrap();
        let mut vm = Vdbe::with_capacity(4, 0);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::OpenWrite,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Rewind,
            p1: 0,
            p2: 3,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Delete,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
        btree.commit().unwrap();

        let mut vm2 = Vdbe::with_capacity(1, 0);
        let mut cursors2: Vec<Option<VdbeCursor>> = vec![None];
        vm2.emit(VdbeOp {
            opcode: Opcode::Delete,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err = vm2.step(&btree, &mut cursors2).unwrap_err();
        assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("invalid cursor")));
    }

    // ── Rewind / Next / Prev / Last ─────────────────────────────────────────

    #[test]
    fn vdbe_rewind_and_last_on_empty_table_jump() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();

        for op in [Opcode::Rewind, Opcode::Last] {
            let mut vm = Vdbe::with_capacity(6, 1);
            let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
            vm.emit(VdbeOp {
                opcode: Opcode::OpenRead,
                p1: 0,
                p2: pgno as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            let cond = vm.emit(VdbeOp {
                opcode: op,
                p1: 0,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: 0,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::ResultRow,
                p1: 0,
                p2: 1,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::Halt,
                p1: 0,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            let empty_target = vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: 1,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::ResultRow,
                p1: 0,
                p2: 1,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.ops[cond].p2 = empty_target as i32;

            assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
            assert_eq!(vm.current_result_row().unwrap()[0], Mem::Int(1));
        }
    }

    #[test]
    fn vdbe_last_and_prev_scan_nonempty() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();
        insert_rows(&btree, pgno, &[(1, "a"), (2, "b"), (3, "c")]);

        let mut vm = Vdbe::with_capacity(10, 1);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Last,
            p1: 0,
            p2: 99,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let loop_start = vm.emit(VdbeOp {
            opcode: Opcode::RowId,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Prev,
            p1: 0,
            p2: loop_start as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let mut got = Vec::new();
        while let StepResult::Row = vm.step(&btree, &mut cursors).unwrap() {
            got.push(match vm.current_result_row().unwrap()[0] {
                Mem::Int(i) => i,
                _ => panic!("expected int"),
            });
        }
        assert_eq!(got, vec![3, 2, 1]);
    }

    #[test]
    fn vdbe_next_after_delete_flag_avoids_skipping_row() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();
        insert_rows(&btree, pgno, &[(1, "a"), (2, "b"), (3, "c")]);

        btree.begin_write().unwrap();
        let mut vm = Vdbe::with_capacity(10, 1);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::OpenWrite,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Rewind,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let loop_start = vm.emit(VdbeOp {
            opcode: Opcode::RowId,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Delete,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Next,
            p1: 0,
            p2: loop_start as i32,
            p3: 0,
            p4: P4::None,
            p5: 1,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let mut got = Vec::new();
        while let StepResult::Row = vm.step(&btree, &mut cursors).unwrap() {
            got.push(match vm.current_result_row().unwrap()[0] {
                Mem::Int(i) => i,
                _ => panic!("expected int"),
            });
        }
        btree.commit().unwrap();
        assert_eq!(got, vec![1, 2, 3]);
    }

    // ── Seek opcodes ────────────────────────────────────────────────────────

    #[test]
    fn vdbe_seek_opcodes_nonempty_table() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();
        insert_rows(&btree, pgno, &[(10, "a"), (20, "b"), (30, "c")]);

        // SeekGe
        assert!(!seek_jumped(&btree, pgno, Opcode::SeekGe, 5));
        assert!(!seek_jumped(&btree, pgno, Opcode::SeekGe, 20));
        assert!(seek_jumped(&btree, pgno, Opcode::SeekGe, 35));

        // SeekGt
        assert!(!seek_jumped(&btree, pgno, Opcode::SeekGt, 5));
        assert!(!seek_jumped(&btree, pgno, Opcode::SeekGt, 20));
        assert!(seek_jumped(&btree, pgno, Opcode::SeekGt, 30));
        assert!(seek_jumped(&btree, pgno, Opcode::SeekGt, 35));

        // SeekLe
        assert!(!seek_jumped(&btree, pgno, Opcode::SeekLe, 15));
        assert!(seek_jumped(&btree, pgno, Opcode::SeekLe, 5));
        assert!(!seek_jumped(&btree, pgno, Opcode::SeekLe, 20));
        assert!(!seek_jumped(&btree, pgno, Opcode::SeekLe, 35));

        // SeekLt
        assert!(!seek_jumped(&btree, pgno, Opcode::SeekLt, 15));
        assert!(seek_jumped(&btree, pgno, Opcode::SeekLt, 10));
        assert!(!seek_jumped(&btree, pgno, Opcode::SeekLt, 35));
    }

    #[test]
    fn vdbe_seek_opcodes_on_empty_table_all_jump() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();

        for op in [
            Opcode::SeekGe,
            Opcode::SeekGt,
            Opcode::SeekLe,
            Opcode::SeekLt,
        ] {
            assert!(
                seek_jumped(&btree, pgno, op, 5),
                "{op:?} should jump on an empty table"
            );
        }
    }

    // ── InsertInt ───────────────────────────────────────────────────────────

    #[test]
    fn vdbe_insert_int_ok_and_errors() {
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();

        btree.begin_write().unwrap();
        let mut vm = Vdbe::with_capacity(4, 2);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.regs[1] = Mem::Blob(Arc::from(vec![7u8]));
        vm.emit(VdbeOp {
            opcode: Opcode::OpenWrite,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::InsertInt,
            p1: 0,
            p2: 1,
            p3: 42,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
        btree.commit().unwrap();

        let mut vm2 = Vdbe::with_capacity(4, 1);
        let mut cursors2: Vec<Option<VdbeCursor>> = vec![None];
        vm2.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm2.emit(VdbeOp {
            opcode: Opcode::Rewind,
            p1: 0,
            p2: 3,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm2.emit(VdbeOp {
            opcode: Opcode::RowId,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm2.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm2.step(&btree, &mut cursors2).unwrap(), StepResult::Row);
        assert_eq!(vm2.current_result_row().unwrap()[0], Mem::Int(42));

        let mut vm3 = Vdbe::with_capacity(2, 1);
        let mut cursors3: Vec<Option<VdbeCursor>> = vec![None];
        vm3.emit(VdbeOp {
            opcode: Opcode::OpenWrite,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm3.emit(VdbeOp {
            opcode: Opcode::InsertInt,
            p1: 0,
            p2: 0,
            p3: 1,
            p4: P4::None,
            p5: 0,
        }); // reg0 is Null
        let err = vm3.step(&btree, &mut cursors3).unwrap_err();
        assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("record must be blob")));

        let mut vm4 = Vdbe::with_capacity(1, 1);
        let mut cursors4: Vec<Option<VdbeCursor>> = vec![None];
        vm4.regs[0] = Mem::Blob(Arc::from(vec![1u8]));
        vm4.emit(VdbeOp {
            opcode: Opcode::InsertInt,
            p1: 0,
            p2: 0,
            p3: 1,
            p4: P4::None,
            p5: 0,
        });
        let err4 = vm4.step(&btree, &mut cursors4).unwrap_err();
        assert!(matches!(err4, VdbeError::Exec(msg) if msg.contains("invalid cursor")));
    }

    // ── DecrJumpZero ────────────────────────────────────────────────────────

    #[test]
    fn vdbe_decr_jump_zero_loop_and_type_error() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(6, 1);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.regs[0] = Mem::Int(3);
        let loop_top = vm.emit(VdbeOp {
            opcode: Opcode::DecrJumpZero,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Goto,
            p1: 0,
            p2: loop_top as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let exit = vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.ops[loop_top].p2 = exit as i32;

        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        assert_eq!(vm.current_result_row().unwrap()[0], Mem::Int(2));
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        assert_eq!(vm.current_result_row().unwrap()[0], Mem::Int(1));
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);

        let mut vm2 = Vdbe::with_capacity(1, 1);
        let mut cursors2: Vec<Option<VdbeCursor>> = vec![];
        vm2.emit(VdbeOp {
            opcode: Opcode::DecrJumpZero,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err = vm2.step(&btree, &mut cursors2).unwrap_err();
        assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("DecrJumpZero")));
    }

    // ── Sorter ──────────────────────────────────────────────────────────────

    #[test]
    fn vdbe_sorter_full_lifecycle() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(20, 5);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::SorterOpen,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        // NOTE: register values are loaded via opcodes (not by poking
        // `vm.regs` directly), since all iterations' ops execute later in a
        // single `step()` call, after every iteration has been emitted.
        let rows: [(i32, &str); 3] = [(3, "c"), (1, "a"), (2, "b")];
        for (val, text) in rows {
            vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: val,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::String8,
                p1: 0,
                p2: 1,
                p3: 0,
                p4: P4::Text(Arc::from(text)),
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::MakeRecord,
                p1: 0,
                p2: 2,
                p3: 2,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::SorterInsert,
                p1: 0,
                p2: 2,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
        }

        vm.emit(VdbeOp {
            opcode: Opcode::SorterSort,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let loop_start = vm.emit(VdbeOp {
            opcode: Opcode::SorterData,
            p1: 0,
            p2: 3,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Column,
            p1: 0,
            p2: 0,
            p3: 4,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 4,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::SorterNext,
            p1: 0,
            p2: loop_start as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let mut got = Vec::new();
        while let StepResult::Row = vm.step(&btree, &mut cursors).unwrap() {
            got.push(match &vm.current_result_row().unwrap()[0] {
                Mem::Int(i) => *i,
                other => panic!("expected int, got {other:?}"),
            });
        }
        assert_eq!(got, vec![1, 2, 3]);
    }

    /// Exercises `SorterSort`'s comparator beyond the first zipped field: two
    /// records that tie on field 0 force the comparator to fall through to
    /// field 1 (and, for fully-identical records, to the length tie-break at
    /// the end of the closure).
    #[test]
    fn vdbe_sorter_sort_ties_compare_all_fields() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(40, 6);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::SorterOpen,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let rows: [(i32, &str); 4] = [(2, "z"), (1, "a"), (2, "b"), (1, "a")];
        for (val, text) in rows {
            vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: val,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::String8,
                p1: 0,
                p2: 1,
                p3: 0,
                p4: P4::Text(Arc::from(text)),
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::MakeRecord,
                p1: 0,
                p2: 2,
                p3: 2,
                p4: P4::None,
                p5: 0,
            });
            vm.emit(VdbeOp {
                opcode: Opcode::SorterInsert,
                p1: 0,
                p2: 2,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
        }

        vm.emit(VdbeOp {
            opcode: Opcode::SorterSort,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let loop_start = vm.emit(VdbeOp {
            opcode: Opcode::Column,
            p1: 0,
            p2: 0,
            p3: 4,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Column,
            p1: 0,
            p2: 1,
            p3: 5,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 4,
            p2: 2,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::SorterNext,
            p1: 0,
            p2: loop_start as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let mut got = Vec::new();
        while let StepResult::Row = vm.step(&btree, &mut cursors).unwrap() {
            let row = vm.current_result_row().unwrap();
            let val = match row[0] {
                Mem::Int(i) => i,
                ref other => panic!("expected int, got {other:?}"),
            };
            let text = match &row[1] {
                Mem::Text(t) => t.to_string(),
                other => panic!("expected text, got {other:?}"),
            };
            got.push((val, text));
        }
        assert_eq!(
            got,
            vec![
                (1, "a".to_string()),
                (1, "a".to_string()),
                (2, "b".to_string()),
                (2, "z".to_string()),
            ]
        );
    }

    /// Sorts single-field records of every comparable `Mem` type together so
    /// the comparator's per-type conversion arms (Null/Int/Real/Text/Blob)
    /// are exercised on both sides of the comparison.
    #[test]
    fn vdbe_sorter_sort_compares_all_field_types() {
        /// Emits a single-field record of the given `kind` ("null", "real",
        /// "text", or "blob") and inserts it into sorter cursor 0. Blob
        /// values are loaded via a direct register poke into r2 -- there is
        /// no dedicated "load blob literal" opcode, and no other opcode here
        /// ever writes r2, so the poke survives untouched until read.
        fn emit_record(vm: &mut Vdbe, kind: &str) {
            match kind {
                "null" => {
                    vm.emit(VdbeOp {
                        opcode: Opcode::Null,
                        p1: 0,
                        p2: 0,
                        p3: 0,
                        p4: P4::None,
                        p5: 0,
                    });
                    vm.emit(VdbeOp {
                        opcode: Opcode::MakeRecord,
                        p1: 0,
                        p2: 1,
                        p3: 1,
                        p4: P4::None,
                        p5: 0,
                    });
                }
                "real" => {
                    vm.emit(VdbeOp {
                        opcode: Opcode::Real,
                        p1: 0,
                        p2: 0,
                        p3: 0,
                        p4: P4::Real(3.5),
                        p5: 0,
                    });
                    vm.emit(VdbeOp {
                        opcode: Opcode::MakeRecord,
                        p1: 0,
                        p2: 1,
                        p3: 1,
                        p4: P4::None,
                        p5: 0,
                    });
                }
                "text" => {
                    vm.emit(VdbeOp {
                        opcode: Opcode::String8,
                        p1: 0,
                        p2: 0,
                        p3: 0,
                        p4: P4::Text(Arc::from("m")),
                        p5: 0,
                    });
                    vm.emit(VdbeOp {
                        opcode: Opcode::MakeRecord,
                        p1: 0,
                        p2: 1,
                        p3: 1,
                        p4: P4::None,
                        p5: 0,
                    });
                }
                "blob" => {
                    vm.regs[2] = Mem::Blob(Arc::from(vec![5u8]));
                    vm.emit(VdbeOp {
                        opcode: Opcode::MakeRecord,
                        p1: 2,
                        p2: 1,
                        p3: 1,
                        p4: P4::None,
                        p5: 0,
                    });
                }
                other => panic!("unknown kind {other}"),
            }
            vm.emit(VdbeOp {
                opcode: Opcode::SorterInsert,
                p1: 0,
                p2: 1,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
        }

        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(60, 3);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::SorterOpen,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        // Insert every type in both ascending and descending order so the
        // sort algorithm's pairwise comparisons place each type on both
        // sides of the comparator (as `fa` and as `fb`).
        for kind in [
            "null", "real", "text", "blob", "blob", "text", "real", "null",
        ] {
            emit_record(&mut vm, kind);
        }

        vm.emit(VdbeOp {
            opcode: Opcode::SorterSort,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let loop_start = vm.emit(VdbeOp {
            opcode: Opcode::Column,
            p1: 0,
            p2: 0,
            p3: 1,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 1,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::SorterNext,
            p1: 0,
            p2: loop_start as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let mut got = Vec::new();
        while let StepResult::Row = vm.step(&btree, &mut cursors).unwrap() {
            got.push(vm.current_result_row().unwrap()[0].clone());
        }
        assert_eq!(
            got,
            vec![
                Mem::Null,
                Mem::Null,
                Mem::Real(3.5),
                Mem::Real(3.5),
                Mem::Text(Arc::from("m")),
                Mem::Text(Arc::from("m")),
                Mem::Blob(Arc::from(vec![5u8])),
                Mem::Blob(Arc::from(vec![5u8])),
            ]
        );
    }

    #[test]
    fn vdbe_sorter_sort_empty_jumps() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(4, 0);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::SorterOpen,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::SorterSort,
            p1: 0,
            p2: 3,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        }); // would run if the jump were not taken
        vm.emit(VdbeOp {
            opcode: Opcode::Noop,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        }); // jump target
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
    }

    #[test]
    fn vdbe_sorter_error_paths() {
        let btree = liter_btree::BTree::new_in_memory();

        let mut vm = Vdbe::with_capacity(1, 0);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::SorterOpen,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err = vm.step(&btree, &mut cursors).unwrap_err();
        assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("invalid cursor")));

        let mut vm2 = Vdbe::with_capacity(2, 1);
        let mut cursors2: Vec<Option<VdbeCursor>> = vec![None];
        vm2.emit(VdbeOp {
            opcode: Opcode::SorterOpen,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm2.emit(VdbeOp {
            opcode: Opcode::SorterInsert,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        }); // reg0 Null, not a blob
        let err2 = vm2.step(&btree, &mut cursors2).unwrap_err();
        assert!(matches!(err2, VdbeError::Exec(msg) if msg.contains("must be a Blob")));

        let mut vm3 = Vdbe::with_capacity(1, 1);
        let mut cursors3: Vec<Option<VdbeCursor>> = vec![None];
        vm3.regs[0] = Mem::Blob(Arc::from(vec![1u8]));
        vm3.emit(VdbeOp {
            opcode: Opcode::SorterInsert,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err3 = vm3.step(&btree, &mut cursors3).unwrap_err();
        assert!(matches!(err3, VdbeError::Exec(msg) if msg.contains("cursor not open")));

        let mut vm4 = Vdbe::with_capacity(1, 0);
        let mut cursors4: Vec<Option<VdbeCursor>> = vec![None];
        vm4.emit(VdbeOp {
            opcode: Opcode::SorterSort,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err4 = vm4.step(&btree, &mut cursors4).unwrap_err();
        assert!(matches!(err4, VdbeError::Exec(msg) if msg.contains("cursor not open")));

        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        btree.commit().unwrap();

        let mut vm5 = Vdbe::with_capacity(2, 1);
        let mut cursors5: Vec<Option<VdbeCursor>> = vec![None];
        vm5.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm5.emit(VdbeOp {
            opcode: Opcode::SorterData,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err5 = vm5.step(&btree, &mut cursors5).unwrap_err();
        assert!(matches!(err5, VdbeError::Exec(msg) if msg.contains("not a sorter")));

        let mut vm6 = Vdbe::with_capacity(2, 0);
        let mut cursors6: Vec<Option<VdbeCursor>> = vec![None];
        vm6.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm6.emit(VdbeOp {
            opcode: Opcode::SorterNext,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err6 = vm6.step(&btree, &mut cursors6).unwrap_err();
        assert!(matches!(err6, VdbeError::Exec(msg) if msg.contains("not a sorter")));

        let mut vm7 = Vdbe::with_capacity(1, 0);
        let mut cursors7: Vec<Option<VdbeCursor>> = vec![None];
        vm7.emit(VdbeOp {
            opcode: Opcode::SorterData,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err7 = vm7.step(&btree, &mut cursors7).unwrap_err();
        assert!(matches!(err7, VdbeError::Exec(msg) if msg.contains("invalid cursor")));
    }

    // ── Function ────────────────────────────────────────────────────────────

    #[test]
    fn vdbe_function_opcode_all_branches() {
        let btree = liter_btree::BTree::new_in_memory();

        let mut vm = Vdbe::with_capacity(2, 2);
        vm.func_dispatcher = Some(test_func_dispatcher);
        vm.regs[0] = Mem::Int(21);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::Function,
            p1: 1,
            p2: 0,
            p3: 1,
            p4: P4::Text(Arc::from("double")),
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 1,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        assert_eq!(vm.current_result_row().unwrap()[0], Mem::Int(42));

        let mut vm2 = Vdbe::with_capacity(1, 1);
        vm2.func_dispatcher = Some(test_func_dispatcher);
        let mut cursors2: Vec<Option<VdbeCursor>> = vec![];
        vm2.emit(VdbeOp {
            opcode: Opcode::Function,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("boom")),
            p5: 0,
        });
        let err = vm2.step(&btree, &mut cursors2).unwrap_err();
        assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("kaboom")));

        let mut vm3 = Vdbe::with_capacity(1, 1);
        let mut cursors3: Vec<Option<VdbeCursor>> = vec![];
        vm3.emit(VdbeOp {
            opcode: Opcode::Function,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("double")),
            p5: 0,
        });
        let err3 = vm3.step(&btree, &mut cursors3).unwrap_err();
        assert!(matches!(err3, VdbeError::Exec(msg) if msg.contains("no function dispatcher")));

        let mut vm4 = Vdbe::with_capacity(1, 1);
        let mut cursors4: Vec<Option<VdbeCursor>> = vec![];
        vm4.emit(VdbeOp {
            opcode: Opcode::Function,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err4 = vm4.step(&btree, &mut cursors4).unwrap_err();
        assert!(matches!(err4, VdbeError::Exec(msg) if msg.contains("func name")));

        let mut vm5 = Vdbe::with_capacity(1, 1);
        vm5.func_dispatcher = Some(test_func_dispatcher);
        let mut cursors5: Vec<Option<VdbeCursor>> = vec![];
        vm5.emit(VdbeOp {
            opcode: Opcode::Function,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("double")),
            p5: 0,
        });
        let err5 = vm5.step(&btree, &mut cursors5).unwrap_err();
        assert!(matches!(err5, VdbeError::Exec(msg) if msg.contains("Function double")));
    }

    // ── AggStep / AggFinal ──────────────────────────────────────────────────

    #[test]
    fn vdbe_agg_step_and_final_lifecycle() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(6, 3);
        vm.agg_dispatcher = Some(test_agg_dispatcher);
        vm.regs[0] = Mem::Int(10);
        vm.regs[1] = Mem::Int(5);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::AggStep,
            p1: 1,
            p2: 0,
            p3: 2,
            p4: P4::Text(Arc::from("sum")),
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::AggStep,
            p1: 1,
            p2: 1,
            p3: 2,
            p4: P4::Text(Arc::from("sum")),
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::AggFinal,
            p1: 2,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("sum")),
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 2,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        assert_eq!(vm.current_result_row().unwrap()[0], Mem::Int(15));
    }

    #[test]
    fn vdbe_agg_final_on_untouched_register_initializes_fresh() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(2, 1);
        vm.agg_dispatcher = Some(test_agg_dispatcher);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::AggFinal,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("sum")),
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 0,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        assert_eq!(vm.current_result_row().unwrap()[0], Mem::Int(0));
    }

    #[test]
    fn vdbe_agg_step_and_final_error_paths() {
        let btree = liter_btree::BTree::new_in_memory();

        let mut vm = Vdbe::with_capacity(1, 1);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::AggStep,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err = vm.step(&btree, &mut cursors).unwrap_err();
        assert!(matches!(err, VdbeError::Exec(msg) if msg.contains("func name")));

        let mut vm2 = Vdbe::with_capacity(1, 1);
        let mut cursors2: Vec<Option<VdbeCursor>> = vec![];
        vm2.emit(VdbeOp {
            opcode: Opcode::AggStep,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("sum")),
            p5: 0,
        });
        let err2 = vm2.step(&btree, &mut cursors2).unwrap_err();
        assert!(matches!(err2, VdbeError::Exec(msg) if msg.contains("no agg dispatcher")));

        let mut vm3 = Vdbe::with_capacity(1, 1);
        vm3.agg_dispatcher = Some(test_agg_dispatcher);
        let mut cursors3: Vec<Option<VdbeCursor>> = vec![];
        vm3.emit(VdbeOp {
            opcode: Opcode::AggStep,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("fail")),
            p5: 0,
        });
        let err3 = vm3.step(&btree, &mut cursors3).unwrap_err();
        assert!(matches!(err3, VdbeError::Exec(msg) if msg.contains("no such aggregate")));

        let mut vm4 = Vdbe::with_capacity(1, 1);
        vm4.agg_dispatcher = Some(test_agg_dispatcher);
        vm4.regs[0] = Mem::Int(99); // pre-populated, non-null, non-agg
        let mut cursors4: Vec<Option<VdbeCursor>> = vec![];
        vm4.emit(VdbeOp {
            opcode: Opcode::AggStep,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("sum")),
            p5: 0,
        });
        let err4 = vm4.step(&btree, &mut cursors4).unwrap_err();
        assert!(matches!(err4, VdbeError::Exec(msg) if msg.contains("not an aggregate")));

        let mut vm5 = Vdbe::with_capacity(1, 2);
        vm5.agg_dispatcher = Some(test_agg_dispatcher);
        vm5.regs[0] = Mem::Text(Arc::from("not an int"));
        let mut cursors5: Vec<Option<VdbeCursor>> = vec![];
        vm5.emit(VdbeOp {
            opcode: Opcode::AggStep,
            p1: 1,
            p2: 0,
            p3: 1,
            p4: P4::Text(Arc::from("sum")),
            p5: 0,
        });
        let err5 = vm5.step(&btree, &mut cursors5).unwrap_err();
        assert!(matches!(err5, VdbeError::Exec(msg) if msg.contains("expected int")));

        let mut vm6 = Vdbe::with_capacity(1, 1);
        let mut cursors6: Vec<Option<VdbeCursor>> = vec![];
        vm6.emit(VdbeOp {
            opcode: Opcode::AggFinal,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err6 = vm6.step(&btree, &mut cursors6).unwrap_err();
        assert!(matches!(err6, VdbeError::Exec(msg) if msg.contains("func name")));

        let mut vm7 = Vdbe::with_capacity(1, 1);
        let mut cursors7: Vec<Option<VdbeCursor>> = vec![];
        vm7.emit(VdbeOp {
            opcode: Opcode::AggFinal,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("sum")),
            p5: 0,
        });
        let err7 = vm7.step(&btree, &mut cursors7).unwrap_err();
        assert!(matches!(err7, VdbeError::Exec(msg) if msg.contains("no agg dispatcher")));

        let mut vm8 = Vdbe::with_capacity(1, 1);
        vm8.regs[0] = Mem::Int(7);
        let mut cursors8: Vec<Option<VdbeCursor>> = vec![];
        vm8.emit(VdbeOp {
            opcode: Opcode::AggFinal,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("sum")),
            p5: 0,
        });
        let err8 = vm8.step(&btree, &mut cursors8).unwrap_err();
        assert!(matches!(err8, VdbeError::Exec(msg) if msg.contains("not an aggregate")));

        let mut vm9 = Vdbe::with_capacity(1, 1);
        vm9.agg_dispatcher = Some(test_agg_dispatcher);
        let mut cursors9: Vec<Option<VdbeCursor>> = vec![];
        vm9.emit(VdbeOp {
            opcode: Opcode::AggFinal,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("fail")),
            p5: 0,
        });
        let err9 = vm9.step(&btree, &mut cursors9).unwrap_err();
        assert!(matches!(err9, VdbeError::Exec(msg) if msg.contains("no such aggregate")));

        let mut vm10 = Vdbe::with_capacity(2, 1);
        vm10.agg_dispatcher = Some(test_agg_dispatcher);
        let mut cursors10: Vec<Option<VdbeCursor>> = vec![];
        vm10.emit(VdbeOp {
            opcode: Opcode::AggStep,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("failfinal")),
            p5: 0,
        });
        vm10.emit(VdbeOp {
            opcode: Opcode::AggFinal,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("failfinal")),
            p5: 0,
        });
        let err10 = vm10.step(&btree, &mut cursors10).unwrap_err();
        assert!(matches!(err10, VdbeError::Exec(msg) if msg.contains("finalize boom")));

        // AggFinal on a still-Null register (no prior AggStep) whose freshly
        // dispatched aggregate's finalize() itself errors.
        let mut vm11 = Vdbe::with_capacity(1, 1);
        vm11.agg_dispatcher = Some(test_agg_dispatcher);
        let mut cursors11: Vec<Option<VdbeCursor>> = vec![];
        vm11.emit(VdbeOp {
            opcode: Opcode::AggFinal,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("failfinal")),
            p5: 0,
        });
        let err11 = vm11.step(&btree, &mut cursors11).unwrap_err();
        assert!(matches!(err11, VdbeError::Exec(msg) if msg.contains("finalize boom")));
    }

    // ── ResultRow / Noop / Halt / unimplemented ─────────────────────────────

    #[test]
    fn vdbe_noop_and_halted_is_idempotent() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(3, 0);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::Noop,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
        // Calling step() again after Halt returns Done immediately.
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
    }

    #[test]
    fn vdbe_unimplemented_opcode_errors() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(1, 0);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::OpenEphemeral,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let err = vm.step(&btree, &mut cursors).unwrap_err();
        assert!(matches!(err, VdbeError::NotImplemented));
    }

    // ── reset ───────────────────────────────────────────────────────────────

    #[test]
    fn vdbe_reset_clears_pc_halted_and_agg_registers() {
        let btree = liter_btree::BTree::new_in_memory();
        let mut vm = Vdbe::with_capacity(4, 2);
        vm.agg_dispatcher = Some(test_agg_dispatcher);
        vm.regs[0] = Mem::Int(5); // AggStep argument
        let mut cursors: Vec<Option<VdbeCursor>> = vec![];
        vm.emit(VdbeOp {
            opcode: Opcode::AggStep,
            p1: 1,
            p2: 0,
            p3: 1,
            p4: P4::Text(Arc::from("sum")),
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: 1,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
        assert!(matches!(vm.current_result_row().unwrap()[0], Mem::Agg(_)));
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);

        vm.reset().unwrap();
        assert_eq!(vm.regs[1], Mem::Null); // the Agg register was cleared
        assert!(vm.current_result_row().is_none());
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
    }

    #[test]
    fn vdbe_mem_agg_cmp_and_zeroblob_column() {
        use std::cmp::Ordering;
        // Test Mem::Agg in cmp
        let m_agg = Mem::Agg(0);
        let m_int = Mem::Int(10);
        assert_eq!(m_agg.cmp(&m_int), Ordering::Greater);
        assert_eq!(m_int.cmp(&m_agg), Ordering::Less);
        assert_eq!(m_agg.cmp(&Mem::Agg(1)), Ordering::Equal);

        // Test Column decoding Blob
        let btree = liter_btree::BTree::new_in_memory();
        btree.begin_write().unwrap();
        let pgno = btree.allocate_page(PageKind::TableLeaf).unwrap();
        let payload = liter_record::encode_record(&[
            liter_record::Value::Blob(vec![1, 2, 3]),
            liter_record::Value::Null,
        ])
        .unwrap();
        let mut cur = btree.cursor(pgno, true).unwrap();
        cur.insert(&1u64.to_be_bytes(), &payload, false).unwrap();
        btree.commit().unwrap();

        let mut vm = Vdbe::with_capacity(3, 2);
        let mut cursors: Vec<Option<VdbeCursor>> = vec![None];
        vm.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: 0,
            p2: pgno as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Rewind,
            p1: 0,
            p2: 5,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Column,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Column,
            p1: 0,
            p2: 1,
            p3: 1,
            p4: P4::None,
            p5: 0,
        });
        vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
        assert_eq!(
            vm.regs[0],
            Mem::Blob(Arc::from(vec![1, 2, 3].into_boxed_slice()))
        );
        assert_eq!(vm.regs[1], Mem::Null);
    }

    #[test]
    fn vdbe_agg_invalid_index_and_unknown_names() {
        let btree = liter_btree::BTree::new_in_memory();

        // 1. AggStep with Mem::Agg(999) pointing to invalid index in self.aggs
        let mut vm1 = Vdbe::with_capacity(1, 1);
        vm1.agg_dispatcher = Some(test_agg_dispatcher);
        vm1.regs[0] = Mem::Agg(999);
        let mut cursors1: Vec<Option<VdbeCursor>> = vec![];
        vm1.emit(VdbeOp {
            opcode: Opcode::AggStep,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("sum")),
            p5: 0,
        });
        let err1 = vm1.step(&btree, &mut cursors1).unwrap_err();
        assert!(matches!(err1, VdbeError::Exec(msg) if msg.contains("Invalid aggregate index")));

        // 2. AggFinal with Mem::Agg(999) pointing to invalid index in self.aggs
        let mut vm2 = Vdbe::with_capacity(1, 1);
        vm2.agg_dispatcher = Some(test_agg_dispatcher);
        vm2.regs[0] = Mem::Agg(999);
        let mut cursors2: Vec<Option<VdbeCursor>> = vec![];
        vm2.emit(VdbeOp {
            opcode: Opcode::AggFinal,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::Text(Arc::from("sum")),
            p5: 0,
        });
        let err2 = vm2.step(&btree, &mut cursors2).unwrap_err();
        assert!(matches!(err2, VdbeError::Exec(msg) if msg.contains("Invalid aggregate index")));

        // 3. Unknown function name in test dispatcher
        assert!(test_func_dispatcher("unknown_func", &[]).is_err());

        // 4. Unknown aggregate name in test dispatcher
        assert!(test_agg_dispatcher("unknown_agg").is_err());
    }
}
