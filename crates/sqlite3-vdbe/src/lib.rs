//! Virtual Database Engine (VDBE) for SQLite3-rs.
//!
//! Mirrors `vdbe.c`, `vdbeapi.c`, `vdbeaux.c`, `vdbemem.c`, `vdbesort.c`,
//! `vdbeblob.c`. Executes compiled bytecode programs against the B-tree layer.
//!
//! ## Status
//! Phase 3 — type definitions and VM skeleton only.

use sqlite3_btree::{BTree, BTreeCursor, PageKind};
use std::sync::Arc;

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
}

impl Mem {
    pub fn is_null(&self) -> bool { matches!(self, Mem::Null) }

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
    // Schema
    CreateTable,
    // Result output
    ResultRow,
    // Aggregation
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
    /// Holds the range of registers returned by the last ResultRow.
    last_result_row: Option<(usize, usize)>,
    /// Number of cursors required by this program.
    pub n_cursors: usize,
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

    pub fn step<'a>(&mut self, btree: &'a BTree, cursors: &mut [Option<BTreeCursor<'a>>]) -> VdbeResult<StepResult> {
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
                    if self.regs[op.p1 as usize] == self.regs[op.p3 as usize] {
                        self.pc = op.p2 as usize;
                    }
                }
                Opcode::Ne => {
                    if self.regs[op.p1 as usize] != self.regs[op.p3 as usize] {
                        self.pc = op.p2 as usize;
                    }
                }
                Opcode::Lt => {
                    if let (Some(a), Some(b)) = (self.regs[op.p3 as usize].to_real(), self.regs[op.p1 as usize].to_real()) {
                        if a < b { self.pc = op.p2 as usize; }
                    }
                }
                Opcode::Le => {
                    if let (Some(a), Some(b)) = (self.regs[op.p3 as usize].to_real(), self.regs[op.p1 as usize].to_real()) {
                        if a <= b { self.pc = op.p2 as usize; }
                    }
                }
                Opcode::Gt => {
                    if let (Some(a), Some(b)) = (self.regs[op.p3 as usize].to_real(), self.regs[op.p1 as usize].to_real()) {
                        if a > b { self.pc = op.p2 as usize; }
                    }
                }
                Opcode::Ge => {
                    if let (Some(a), Some(b)) = (self.regs[op.p3 as usize].to_real(), self.regs[op.p1 as usize].to_real()) {
                        if a >= b { self.pc = op.p2 as usize; }
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
                        return Err(VdbeError::Exec("cursor index out of bounds".to_string()));
                    }
                    cursors[cursor_idx] = Some(btree.cursor(root_page, true)?);
                }
                Opcode::NewRowid => {
                    let cursor_idx = op.p1 as usize;
                    let dest_reg = op.p2 as usize;
                    if let Some(cursor) = &mut cursors[cursor_idx] {
                        let max_rowid = if cursor.move_to_last()? {
                            let k = cursor.key()?;
                            u64::from_be_bytes(k.try_into().unwrap())
                        } else {
                            0
                        };
                        self.regs[dest_reg] = Mem::Int((max_rowid + 1) as i64);
                    } else {
                        return Err(VdbeError::Exec("invalid cursor".to_string()));
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
                        cursor.insert(&rowid.to_be_bytes(), &record, false)?;
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
        for r in &mut self.regs { *r = Mem::Null; }
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
