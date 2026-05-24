//! VDBE bytecode generator for SQLite3-rs.
//!
//! Mirrors `select.c`, `insert.c`, `delete.c`, `update.c`, `trigger.c`.
//! Transforms a resolved + planned AST into a `Vdbe` program.

use sqlite3_ast::*;
use sqlite3_vdbe::{Opcode, P4, Vdbe, VdbeOp};
use std::sync::Arc;

use sqlite3_schema::Schema;

#[derive(Debug, thiserror::Error)]
pub enum CodegenError {
    #[error("not yet implemented")]
    NotImplemented,
    #[error("internal codegen error: {0}")]
    Internal(String),
    #[error("schema error: {0}")]
    Schema(String),
}

pub type CodegenResult<T> = Result<T, CodegenError>;

pub struct Compiler<'a> {
    vm: Vdbe,
    schema: Option<&'a Schema>,
}

impl<'a> Compiler<'a> {
    pub fn new() -> Self {
        Self { vm: Vdbe::new(), schema: None }
    }

    pub fn with_schema(schema: &'a Schema) -> Self {
        Self { vm: Vdbe::new(), schema: Some(schema) }
    }

    pub fn compile(mut self, stmt: &Stmt) -> CodegenResult<Vdbe> {
        match stmt {
            Stmt::Select(select) => self.compile_select(select)?,
            Stmt::Create(create) => self.compile_create(create)?,
            Stmt::Insert(insert) => self.compile_insert(insert)?,
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

        if let Some(from) = &body.from {
            self.compile_select_with_from(body, from)?;
        } else {
            // Literal projection: SELECT expr, expr, ...
            self.compile_select_literal(body)?;
        }

        Ok(())
    }

    /// Compile a `SELECT` with no `FROM` clause (pure expression projection).
    fn compile_select_literal(&mut self, body: &SimpleSelect) -> CodegenResult<()> {
        let num_cols = body.result_columns.len();
        let result_reg_start = self.vm.alloc_reg();
        for _ in 1..num_cols {
            self.vm.alloc_reg();
        }

        for (i, col) in body.result_columns.iter().enumerate() {
            let expr = match col {
                ResultColumn::Expr { expr, .. } => expr,
                _ => return Err(CodegenError::NotImplemented),
            };
            let r_val = self.compile_expr(expr, None)?;
            self.vm.emit(VdbeOp {
                opcode: Opcode::Copy,
                p1: r_val as i32,
                p2: (result_reg_start + i) as i32,
                p3: 0, p4: P4::None, p5: 0,
            });
        }

        self.vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: result_reg_start as i32,
            p2: num_cols as i32,
            p3: 0, p4: P4::None, p5: 0,
        });

        Ok(())
    }

    /// Compile a `SELECT ... FROM table [WHERE expr]` full-table scan.
    fn compile_select_with_from(
        &mut self,
        body: &SimpleSelect,
        from: &FromClause,
    ) -> CodegenResult<()> {
        // Only single-table scans for now; no joins.
        if from.tables.len() != 1 || !from.joins.is_empty() {
            return Err(CodegenError::NotImplemented);
        }
        let table_name = match &from.tables[0] {
            TableOrSubquery::Table { name, .. } => name.clone(),
            _ => return Err(CodegenError::NotImplemented),
        };

        // Look up the table in the schema to find its root_page and column list.
        let schema_obj = self.schema
            .ok_or_else(|| CodegenError::Schema("no schema context".to_string()))?
            .get(&table_name)
            .ok_or_else(|| CodegenError::Schema(format!("table '{}' not found", table_name)))?;
        let root_page = schema_obj.root_page;
        let schema_cols = schema_obj.columns.clone(); // Vec<ColumnDef>

        let cursor_id = self.vm.n_cursors;
        self.vm.n_cursors += 1;

        // Resolve output columns — expand Star into all schema columns.
        let resolved: Vec<(usize, String)> = {
            let mut cols = Vec::new();
            for rc in &body.result_columns {
                match rc {
                    ResultColumn::Star | ResultColumn::TableStar(_) => {
                        for (i, col) in schema_cols.iter().enumerate() {
                            cols.push((i, col.name.clone()));
                        }
                    }
                    ResultColumn::Expr { expr: Expr::Column { name, .. }, .. } => {
                        let idx = schema_cols.iter().position(|c| c.name.eq_ignore_ascii_case(name))
                            .ok_or_else(|| CodegenError::Schema(
                                format!("column '{}' not found in '{}'", name, table_name)))?;
                        cols.push((idx, name.clone()));
                    }
                    ResultColumn::Expr { .. } => {
                        // Computed expressions — handled as a generic expression below.
                        cols.push((usize::MAX, String::new()));
                    }
                }
            }
            cols
        };

        let num_cols = resolved.len();
        let result_reg_start = self.vm.alloc_reg();
        for _ in 1..num_cols {
            self.vm.alloc_reg();
        }

        // Emit: OpenRead cursor
        self.vm.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: cursor_id as i32,
            p2: root_page as i32,
            p3: 0, p4: P4::None, p5: 0,
        });

        // Emit: Rewind — jump to end_label if table is empty (patch p2 later)
        let rewind_addr = self.vm.emit(VdbeOp {
            opcode: Opcode::Rewind,
            p1: cursor_id as i32,
            p2: 0, // patched below
            p3: 0, p4: P4::None, p5: 0,
        });

        // Loop top — all body ops go here
        let loop_top = self.vm.ops.len();

        // Emit Column reads for each result column
        for (i, (col_idx, _col_name)) in resolved.iter().enumerate() {
            if *col_idx == usize::MAX {
                // Generic expr — evaluate it
                let expr = match &body.result_columns[i] {
                    ResultColumn::Expr { expr, .. } => expr,
                    _ => unreachable!(),
                };
                let r = self.compile_expr(expr, Some((cursor_id, &schema_cols)))?;
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Copy,
                    p1: r as i32,
                    p2: (result_reg_start + i) as i32,
                    p3: 0, p4: P4::None, p5: 0,
                });
            } else {
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Column,
                    p1: cursor_id as i32,
                    p2: *col_idx as i32,
                    p3: (result_reg_start + i) as i32,
                    p4: P4::None, p5: 0,
                });
            }
        }

        // Emit WHERE predicate check (jump to next_label if predicate is false)
        let next_label_patches = if let Some(where_expr) = &body.where_ {
            self.compile_where_expr(where_expr, cursor_id, &schema_cols)?
        } else {
            vec![]
        };

        // Emit: ResultRow
        self.vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: result_reg_start as i32,
            p2: num_cols as i32,
            p3: 0, p4: P4::None, p5: 0,
        });

        // next_label: jump target for WHERE-skipped rows
        let next_label = self.vm.ops.len();
        for patch_addr in next_label_patches {
            self.vm.ops[patch_addr].p2 = next_label as i32;
        }

        // Emit: Next — jumps back to loop_top if more rows
        self.vm.emit(VdbeOp {
            opcode: Opcode::Next,
            p1: cursor_id as i32,
            p2: loop_top as i32,
            p3: 0, p4: P4::None, p5: 0,
        });

        // end_label: where Rewind jumps on empty table
        let end_label = self.vm.ops.len();
        self.vm.ops[rewind_addr].p2 = end_label as i32;

        // Emit: Close cursor
        self.vm.emit(VdbeOp {
            opcode: Opcode::Close,
            p1: cursor_id as i32,
            p2: 0, p3: 0, p4: P4::None, p5: 0,
        });

        Ok(())
    }

    /// Compile a WHERE predicate. Returns the list of instruction addresses
    /// whose `p2` (jump target) must be patched to the "skip row" label.
    ///
    /// Strategy: evaluate the predicate into a register, then emit `IfNot` to
    /// skip the `ResultRow`. Complex AND/OR are short-circuited.
    fn compile_where_expr(
        &mut self,
        expr: &Expr,
        cursor_id: usize,
        schema_cols: &[sqlite3_ast::ColumnDef],
    ) -> CodegenResult<Vec<usize>> {
        match expr {
            // AND: both sides must be true; short-circuit on first false
            Expr::Binary { op: BinaryOp::And, left, right } => {
                let mut patches = self.compile_where_expr(left, cursor_id, schema_cols)?;
                patches.extend(self.compile_where_expr(right, cursor_id, schema_cols)?);
                Ok(patches)
            }
            // OR: at least one must be true — emit both and OR their results
            Expr::Binary { op: BinaryOp::Or, left, right } => {
                let r_left = self.compile_expr(left, Some((cursor_id, schema_cols)))?;
                let r_right = self.compile_expr(right, Some((cursor_id, schema_cols)))?;
                let r_or = self.vm.alloc_reg();
                // r_or = r_left OR r_right (truthy)
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Copy,
                    p1: r_left as i32, p2: r_or as i32,
                    p3: 0, p4: P4::None, p5: 0,
                });
                // if r_or is already truthy, skip checking right
                let skip_right = self.vm.emit(VdbeOp {
                    opcode: Opcode::If,
                    p1: r_or as i32, p2: 0, // patched
                    p3: 0, p4: P4::None, p5: 0,
                });
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Copy,
                    p1: r_right as i32, p2: r_or as i32,
                    p3: 0, p4: P4::None, p5: 0,
                });
                let after_or = self.vm.ops.len();
                self.vm.ops[skip_right].p2 = after_or as i32;

                let ifnot_addr = self.vm.emit(VdbeOp {
                    opcode: Opcode::IfNot,
                    p1: r_or as i32, p2: 0, // patched by caller
                    p3: 0, p4: P4::None, p5: 0,
                });
                Ok(vec![ifnot_addr])
            }
            // IsNull / NotNull checks
            Expr::IsNull { not, expr: inner } => {
                let r = self.compile_expr(inner, Some((cursor_id, schema_cols)))?;
                let opcode = if *not { Opcode::NotNull } else { Opcode::IsNull };
                // IsNull jumps to p2 when TRUE (it IS null), but we want to
                // skip when the condition fails.  Invert: if NOT (is null) skip.
                let check_addr = self.vm.emit(VdbeOp {
                    opcode: if *not { Opcode::IsNull } else { Opcode::NotNull },
                    p1: r as i32, p2: 0, // patched by caller to "skip row"
                    p3: 0, p4: P4::None, p5: 0,
                });
                let _ = opcode; // silence unused
                Ok(vec![check_addr])
            }
            // All other predicates: evaluate to a boolean register, then IfNot-skip
            _ => {
                let r_pred = self.compile_expr(expr, Some((cursor_id, schema_cols)))?;
                let ifnot_addr = self.vm.emit(VdbeOp {
                    opcode: Opcode::IfNot,
                    p1: r_pred as i32, p2: 0, // patched by caller
                    p3: 0, p4: P4::None, p5: 0,
                });
                Ok(vec![ifnot_addr])
            }
        }
    }

    /// Compile an expression into a fresh register and return its index.
    ///
    /// `cursor_ctx` is `Some((cursor_id, schema_cols))` when we're inside a
    /// `SELECT FROM` context and `Expr::Column` references can be resolved.
    fn compile_expr(
        &mut self,
        expr: &Expr,
        cursor_ctx: Option<(usize, &[sqlite3_ast::ColumnDef])>,
    ) -> CodegenResult<usize> {
        match expr {
            Expr::Literal(val) => {
                let r = self.vm.alloc_reg();
                match val {
                    LiteralValue::Integer(i) => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Integer,
                            p1: *i as i32,
                            p2: r as i32,
                            p3: 0, p4: P4::None, p5: 0,
                        });
                    }
                    LiteralValue::Float(f) => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Real,
                            p1: 0, p2: r as i32, p3: 0,
                            p4: P4::Real(*f), p5: 0,
                        });
                    }
                    LiteralValue::Text(s) => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::String8,
                            p1: 0, p2: r as i32, p3: 0,
                            p4: P4::Text(Arc::from(s.clone().into_boxed_str())),
                            p5: 0,
                        });
                    }
                    LiteralValue::True => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Integer,
                            p1: 1, p2: r as i32,
                            p3: 0, p4: P4::None, p5: 0,
                        });
                    }
                    LiteralValue::False => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Integer,
                            p1: 0, p2: r as i32,
                            p3: 0, p4: P4::None, p5: 0,
                        });
                    }
                    LiteralValue::Null => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Null,
                            p1: 0, p2: r as i32,
                            p3: 0, p4: P4::None, p5: 0,
                        });
                    }
                    _ => return Err(CodegenError::NotImplemented),
                }
                Ok(r)
            }

            Expr::Column { name, table: _table, .. } => {
                let (cursor_id, schema_cols) = cursor_ctx
                    .ok_or_else(|| CodegenError::Schema(
                        format!("column '{}' used outside FROM context", name)))?;
                let col_idx = schema_cols.iter().position(|c| c.name.eq_ignore_ascii_case(name))
                    .ok_or_else(|| CodegenError::Schema(
                        format!("column '{}' not found", name)))?;
                let r = self.vm.alloc_reg();
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Column,
                    p1: cursor_id as i32,
                    p2: col_idx as i32,
                    p3: r as i32,
                    p4: P4::None, p5: 0,
                });
                Ok(r)
            }

            Expr::Binary { op, left, right } => {
                let r_left = self.compile_expr(left, cursor_ctx)?;
                let r_right = self.compile_expr(right, cursor_ctx)?;
                let r_res = self.vm.alloc_reg();

                let opcode = match op {
                    BinaryOp::Add => Opcode::AddInt,
                    BinaryOp::Sub => Opcode::SubtractInt,
                    BinaryOp::Mul => Opcode::MultiplyInt,
                    BinaryOp::Div => Opcode::DivideInt,
                    BinaryOp::Mod => Opcode::RemainderInt,
                    BinaryOp::Eq  => Opcode::Eq,
                    BinaryOp::Ne  => Opcode::Ne,
                    BinaryOp::Lt  => Opcode::Lt,
                    BinaryOp::Le  => Opcode::Le,
                    BinaryOp::Gt  => Opcode::Gt,
                    BinaryOp::Ge  => Opcode::Ge,
                    _ => return Err(CodegenError::NotImplemented),
                };

                if self.is_comparison_op(op) {
                    // Comparison ops in SQLite VDBE are conditional jumps, not value producers.
                    // Materialise as: r_res = 0; if cond goto L1; goto L2; L1: r_res = 1; L2:
                    self.vm.emit(VdbeOp { opcode: Opcode::Integer, p1: 0, p2: r_res as i32, p3: 0, p4: P4::None, p5: 0 });
                    let jump_addr = self.vm.emit(VdbeOp { opcode, p1: r_left as i32, p2: 0, p3: r_right as i32, p4: P4::None, p5: 0 });
                    let end_addr  = self.vm.emit(VdbeOp { opcode: Opcode::Goto, p1: 0, p2: 0, p3: 0, p4: P4::None, p5: 0 });
                    let true_addr = self.vm.emit(VdbeOp { opcode: Opcode::Integer, p1: 1, p2: r_res as i32, p3: 0, p4: P4::None, p5: 0 });
                    let post_addr = self.vm.ops.len();
                    self.vm.ops[jump_addr].p2 = true_addr as i32;
                    self.vm.ops[end_addr].p2  = post_addr as i32;
                } else {
                    self.vm.emit(VdbeOp {
                        opcode,
                        p1: r_left as i32,
                        p2: r_right as i32,
                        p3: r_res as i32,
                        p4: P4::None, p5: 0,
                    });
                }
                Ok(r_res)
            }

            Expr::Unary { op: UnaryOp::Minus, operand } => {
                let r_inner = self.compile_expr(operand, cursor_ctx)?;
                let r = self.vm.alloc_reg();
                // Negate: emit 0 - inner
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Integer,
                    p1: 0, p2: r as i32,
                    p3: 0, p4: P4::None, p5: 0,
                });
                self.vm.emit(VdbeOp {
                    opcode: Opcode::SubtractInt,
                    p1: r as i32,
                    p2: r_inner as i32,
                    p3: r as i32,
                    p4: P4::None, p5: 0,
                });
                Ok(r)
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

pub fn compile_with_schema(stmt: &Stmt, schema: &Schema) -> CodegenResult<Vdbe> {
    let compiler = Compiler::with_schema(schema);
    compiler.compile(stmt)
}

impl Compiler<'_> {
    fn compile_create(&mut self, _create: &CreateStmt) -> CodegenResult<()> {
        let dest_reg = self.vm.alloc_reg();
        self.vm.emit(VdbeOp {
            opcode: Opcode::CreateTable,
            p1: 0,
            p2: dest_reg as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        self.vm.emit(VdbeOp {
            opcode: Opcode::ResultRow,
            p1: dest_reg as i32,
            p2: 1,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        Ok(())
    }

    fn compile_insert(&mut self, insert: &InsertStmt) -> CodegenResult<()> {
        let schema = self.schema.ok_or_else(|| CodegenError::Schema("No schema provided".to_string()))?;
        let table_name = &insert.table;
        let table = schema.get(table_name).ok_or_else(|| CodegenError::Schema(format!("Table not found: {}", table_name)))?;

        let root_page = table.root_page as i32;

        let cursor_id = self.vm.n_cursors;
        self.vm.n_cursors += 1;

        self.vm.emit(VdbeOp {
            opcode: Opcode::OpenWrite,
            p1: cursor_id as i32,
            p2: root_page,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        if let InsertSource::Values(values_list) = &insert.source {
            for values in values_list {
                let mut regs = Vec::new();
                for expr in values {
                    let reg = self.compile_expr(expr, None)?;
                    regs.push(reg);
                }

                if regs.is_empty() { continue; }

                let start_reg = regs[0];
                let count = regs.len();
                let record_reg = self.vm.alloc_reg();

                self.vm.emit(VdbeOp {
                    opcode: Opcode::MakeRecord,
                    p1: start_reg as i32,
                    p2: count as i32,
                    p3: record_reg as i32,
                    p4: P4::None,
                    p5: 0,
                });

                let rowid_reg = self.vm.alloc_reg();
                self.vm.emit(VdbeOp {
                    opcode: Opcode::NewRowid,
                    p1: cursor_id as i32,
                    p2: rowid_reg as i32,
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                });

                self.vm.emit(VdbeOp {
                    opcode: Opcode::Insert,
                    p1: cursor_id as i32,
                    p2: record_reg as i32,
                    p3: rowid_reg as i32,
                    p4: P4::None,
                    p5: 0,
                });
            }
        } else {
            return Err(CodegenError::NotImplemented);
        }

        self.vm.emit(VdbeOp {
            opcode: Opcode::Close,
            p1: cursor_id as i32,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        Ok(())
    }
}
