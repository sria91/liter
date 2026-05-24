//! VDBE bytecode generator for SQLite3-rs.
//!
//! Mirrors `select.c`, `insert.c`, `delete.c`, `update.c`, `trigger.c`.
//! Transforms a resolved + planned AST into a `Vdbe` program.

use sqlite3_ast::*;
use sqlite3_vdbe::{Opcode, P4, Vdbe, VdbeOp};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum CodegenError {
    #[error("not yet implemented")]
    NotImplemented,
    #[error("internal codegen error: {0}")]
    Internal(String),
}

pub type CodegenResult<T> = Result<T, CodegenError>;

pub struct Compiler {
    vm: Vdbe,
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new()
    }
}

impl Compiler {
    pub fn new() -> Self {
        Self { vm: Vdbe::new() }
    }

    pub fn compile(mut self, stmt: &Stmt) -> CodegenResult<Vdbe> {
        match stmt {
            Stmt::Select(select) => self.compile_select(select)?,
            _ => return Err(CodegenError::NotImplemented),
        }
        
        self.vm.emit(VdbeOp {
            opcode: Opcode::Halt,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        Ok(self.vm)
    }

    fn compile_select(&mut self, select: &SelectStmt) -> CodegenResult<()> {
        let body = match &select.body {
            SelectBody::Simple(simple) => simple,
            _ => return Err(CodegenError::NotImplemented),
        };

        // We only support projection without FROM/WHERE right now.
        if body.from.is_some() || body.where_.is_some() {
            return Err(CodegenError::NotImplemented);
        }

        let num_cols = body.result_columns.len();
        
        // Allocate contiguous registers for the result row.
        let result_reg_start = self.vm.alloc_reg();
        for _ in 1..num_cols {
            self.vm.alloc_reg();
        }

        for (i, col) in body.result_columns.iter().enumerate() {
            let expr = match col {
                ResultColumn::Expr { expr, .. } => expr,
                _ => return Err(CodegenError::NotImplemented),
            };

            let r_val = self.compile_expr(expr)?;
            self.vm.emit(VdbeOp {
                opcode: Opcode::Copy,
                p1: r_val as i32,
                p2: (result_reg_start + i) as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
        }

        self.vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: result_reg_start as i32,
            p2: num_cols as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        Ok(())
    }

    fn compile_expr(&mut self, expr: &Expr) -> CodegenResult<usize> {
        match expr {
            Expr::Literal(val) => {
                let r = self.vm.alloc_reg();
                match val {
                    LiteralValue::Integer(i) => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Integer,
                            p1: *i as i32, // sqlite3 often stores 32-bit directly in p1, and 64-bit via other ops or p4
                            p2: r as i32,
                            p3: 0,
                            p4: P4::None,
                            p5: 0,
                        });
                    }
                    LiteralValue::Float(f) => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Real,
                            p1: 0,
                            p2: r as i32,
                            p3: 0,
                            p4: P4::Real(*f),
                            p5: 0,
                        });
                    }
                    LiteralValue::Text(t) => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::String8,
                            p1: 0,
                            p2: r as i32,
                            p3: 0,
                            p4: P4::Text(Arc::from(t.clone().into_boxed_str())),
                            p5: 0,
                        });
                    }
                    LiteralValue::Null => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Null,
                            p1: 0,
                            p2: r as i32,
                            p3: 0,
                            p4: P4::None,
                            p5: 0,
                        });
                    }
                    _ => return Err(CodegenError::NotImplemented),
                }
                Ok(r)
            }
            Expr::Binary { op, left, right } => {
                let r_left = self.compile_expr(left)?;
                let r_right = self.compile_expr(right)?;
                let r_res = self.vm.alloc_reg();

                let opcode = match op {
                    BinaryOp::Add => Opcode::AddInt,
                    BinaryOp::Sub => Opcode::SubtractInt,
                    BinaryOp::Mul => Opcode::MultiplyInt,
                    BinaryOp::Div => Opcode::DivideInt,
                    BinaryOp::Mod => Opcode::RemainderInt,
                    BinaryOp::Eq => Opcode::Eq,
                    BinaryOp::Ne => Opcode::Ne,
                    BinaryOp::Lt => Opcode::Lt,
                    BinaryOp::Le => Opcode::Le,
                    BinaryOp::Gt => Opcode::Gt,
                    BinaryOp::Ge => Opcode::Ge,
                    _ => return Err(CodegenError::NotImplemented),
                };

                // Wait, comparison operators in SQLite VDBE (Eq, Ne, etc) are conditional jumps.
                // If we use them in an expression (e.g. `SELECT 1 = 1`), they need to push 1 or 0 into a register.
                // SQLite uses `Eq` as a jump. So `1 = 1` evaluates to:
                // r_res = 0
                // If r_left == r_right goto L1
                // Goto L2
                // L1: r_res = 1
                // L2: ...
                if self.is_comparison_op(op) {
                    // r_res = 0
                    self.vm.emit(VdbeOp { opcode: Opcode::Integer, p1: 0, p2: r_res as i32, p3: 0, p4: P4::None, p5: 0 });
                    
                    // Cond jump
                    let jump_addr = self.vm.emit(VdbeOp { opcode, p1: r_left as i32, p2: 0 /* patch */, p3: r_right as i32, p4: P4::None, p5: 0 });
                    
                    // Goto end
                    let end_addr = self.vm.emit(VdbeOp { opcode: Opcode::Goto, p1: 0, p2: 0 /* patch */, p3: 0, p4: P4::None, p5: 0 });
                    
                    // L1: True case
                    let true_addr = self.vm.emit(VdbeOp { opcode: Opcode::Integer, p1: 1, p2: r_res as i32, p3: 0, p4: P4::None, p5: 0 });
                    let post_addr = self.vm.ops.len();
                    
                    // Patch
                    self.vm.ops[jump_addr].p2 = true_addr as i32;
                    self.vm.ops[end_addr].p2 = post_addr as i32;
                } else {
                    self.vm.emit(VdbeOp {
                        opcode,
                        p1: r_left as i32,
                        p2: r_right as i32,
                        p3: r_res as i32,
                        p4: P4::None,
                        p5: 0,
                    });
                }
                
                Ok(r_res)
            }
            _ => Err(CodegenError::NotImplemented),
        }
    }

    fn is_comparison_op(&self, op: &BinaryOp) -> bool {
        matches!(op, BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge)
    }
}

pub fn compile(stmt: &Stmt) -> CodegenResult<Vdbe> {
    let compiler = Compiler::new();
    compiler.compile(stmt)
}
