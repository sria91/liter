//! VDBE bytecode generator for Liter-rs.
//!
//! Mirrors `select.c`, `insert.c`, `delete.c`, `update.c`, `trigger.c`.
//! Transforms a resolved + planned AST into a `Vdbe` program.

use liter_ast::*;
use liter_vdbe::{Opcode, Vdbe, VdbeOp, P4};
use std::sync::Arc;

use liter_schema::Schema;

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
    pub vm: Vdbe,
    pub schema: Option<&'a Schema>,
    agg_regs: Vec<(Expr, usize)>,
}

impl Default for Compiler<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Compiler<'a> {
    pub fn new() -> Self {
        Self {
            vm: Vdbe::new(),
            schema: None,
            agg_regs: Vec::new(),
        }
    }

    pub fn with_schema(schema: &'a Schema) -> Self {
        Self {
            vm: Vdbe::new(),
            schema: Some(schema),
            agg_regs: Vec::new(),
        }
    }

    pub fn compile(mut self, stmt: &Stmt) -> CodegenResult<Vdbe> {
        match stmt {
            Stmt::Select(select) => self.compile_select(select)?,
            Stmt::Create(create) => self.compile_create(create)?,
            Stmt::Insert(insert) => self.compile_insert(insert)?,
            Stmt::Delete(delete) => self.compile_delete(delete)?,
            Stmt::Update(update) => self.compile_update(update)?,
            // Transaction stmts produce no bytecode — handled by Connection.
            Stmt::Begin(_)
            | Stmt::Commit
            | Stmt::Rollback { .. }
            | Stmt::Savepoint(_)
            | Stmt::Release(_) => {}
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
            self.compile_select_with_from(select, body, from)?;
        } else {
            // Literal projection: SELECT expr, expr, ...
            self.compile_select_literal(select, body)?;
        }

        Ok(())
    }

    /// Compile a `SELECT` with no `FROM` clause (pure expression projection).
    fn compile_select_literal(
        &mut self,
        _select: &SelectStmt,
        body: &SimpleSelect,
    ) -> CodegenResult<()> {
        let num_cols = body.result_columns.len();
        let result_reg_start = self.vm.alloc_reg();
        for _ in 1..num_cols {
            self.vm.alloc_reg();
        }

        let skip_jumps = if let Some(where_expr) = &body.where_ {
            self.compile_where_expr(where_expr, None)?
        } else {
            Vec::new()
        };

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

        let halt_addr = self.vm.ops.len();
        for jump in skip_jumps {
            self.vm.ops[jump].p2 = halt_addr as i32;
        }

        Ok(())
    }

    /// Compile a `SELECT ... FROM table [WHERE expr]` full-table scan.
    fn compile_select_with_from(
        &mut self,
        select: &SelectStmt,
        body: &SimpleSelect,
        from: &FromClause,
    ) -> CodegenResult<()> {
        if Self::is_aggregate_query(body) {
            return self.compile_aggregate_select(select, body, from);
        }

        // Only single-table scans for now; no joins.
        if from.tables.len() != 1 || !from.joins.is_empty() {
            return Err(CodegenError::NotImplemented);
        }
        let table_name = match &from.tables[0] {
            TableOrSubquery::Table { name, .. } => name.clone(),
            _ => return Err(CodegenError::NotImplemented),
        };

        // Look up the table in the schema to find its root_page and column list.
        let schema_obj = self
            .schema
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
                    ResultColumn::Expr {
                        expr: Expr::Column { name, .. },
                        ..
                    } => {
                        let idx = schema_cols
                            .iter()
                            .position(|c| c.name.eq_ignore_ascii_case(name))
                            .ok_or_else(|| {
                                CodegenError::Schema(format!(
                                    "column '{}' not found in '{}'",
                                    name, table_name
                                ))
                            })?;
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
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        // Emit: Rewind — jump to end_label if table is empty (patch p2 later)
        let rewind_addr = self.vm.emit(VdbeOp {
            opcode: Opcode::Rewind,
            p1: cursor_id as i32,
            p2: 0, // patched below
            p3: 0,
            p4: P4::None,
            p5: 0,
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
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                });
            } else {
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Column,
                    p1: cursor_id as i32,
                    p2: *col_idx as i32,
                    p3: (result_reg_start + i) as i32,
                    p4: P4::None,
                    p5: 0,
                });
            }
        }

        // Emit WHERE predicate check (jump to next_label if predicate is false)
        let next_label_patches = if let Some(where_expr) = &body.where_ {
            self.compile_where_expr(where_expr, Some((cursor_id, &schema_cols)))?
        } else {
            vec![]
        };

        let has_order_by = !select.order_by.is_empty();

        let limit_reg = if let Some(limit) = &select.limit {
            let limit_reg = self.compile_expr(&limit.limit, None)?;
            Some(limit_reg)
        } else {
            None
        };

        let sorter_cursor = if has_order_by {
            let cursor = self.vm.n_cursors;
            self.vm.n_cursors += 1;
            self.vm.emit(VdbeOp {
                opcode: Opcode::SorterOpen,
                p1: cursor as i32,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            Some(cursor)
        } else {
            None
        };

        let limit_jump_addr = if sorter_cursor.is_none() {
            if let Some(l_reg) = limit_reg {
                Some(self.vm.emit(VdbeOp {
                    opcode: Opcode::DecrJumpZero,
                    p1: l_reg as i32,
                    p2: 0, // patch later
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                }))
            } else {
                None
            }
        } else {
            None
        };

        if let Some(s_cur) = sorter_cursor {
            let record_reg = self.vm.alloc_reg();
            self.vm.emit(VdbeOp {
                opcode: Opcode::MakeRecord,
                p1: result_reg_start as i32,
                p2: num_cols as i32,
                p3: record_reg as i32,
                p4: P4::None,
                p5: 0,
            });
            self.vm.emit(VdbeOp {
                opcode: Opcode::SorterInsert,
                p1: s_cur as i32,
                p2: record_reg as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
        } else {
            self.vm.emit(VdbeOp {
                opcode: Opcode::ResultRow,
                p1: result_reg_start as i32,
                p2: num_cols as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
        }

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
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let end_label = self.vm.ops.len();
        self.vm.ops[rewind_addr].p2 = end_label as i32;
        if let Some(l_jump) = limit_jump_addr {
            self.vm.ops[l_jump].p2 = end_label as i32;
        }

        // Output from Sorter if we used one
        if let Some(s_cur) = sorter_cursor {
            let _sort_top = self.vm.ops.len();
            let sorter_sort_addr = self.vm.emit(VdbeOp {
                opcode: Opcode::SorterSort,
                p1: s_cur as i32,
                p2: 0, // Patched later
                p3: 0,
                p4: P4::None,
                p5: 0,
            });

            let sorter_loop = self.vm.ops.len();

            // Limit check
            if let Some(l_reg) = limit_reg {
                self.vm.emit(VdbeOp {
                    opcode: Opcode::DecrJumpZero,
                    p1: l_reg as i32,
                    p2: 0, // Patched later (jump to end of sort)
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                });
            }

            for i in 0..num_cols {
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Column,
                    p1: s_cur as i32,
                    p2: i as i32,
                    p3: (result_reg_start + i) as i32,
                    p4: P4::None,
                    p5: 0,
                });
            }

            // ResultRow for sorted data
            self.vm.emit(VdbeOp {
                opcode: Opcode::ResultRow,
                p1: result_reg_start as i32,
                p2: num_cols as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });

            self.vm.emit(VdbeOp {
                opcode: Opcode::SorterNext,
                p1: s_cur as i32,
                p2: sorter_loop as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });

            let sort_end = self.vm.ops.len();
            self.vm.ops[sorter_sort_addr].p2 = sort_end as i32;

            if let Some(_l_reg) = limit_reg {
                // Patch the limit jump
                self.vm.ops[sorter_sort_addr + 1].p2 = sort_end as i32;
            }
        }

        // Emit: Close cursor
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

    fn extract_aggregates(expr: &Expr, aggs: &mut Vec<Expr>) {
        let is_agg = match expr {
            Expr::Function { name, .. } => {
                let n = name.to_ascii_lowercase();
                n == "count" || n == "sum" || n == "avg" || n == "min" || n == "max"
            }
            _ => false,
        };
        if is_agg {
            if !aggs.contains(expr) {
                aggs.push(expr.clone());
            }
        } else {
            match expr {
                Expr::Unary { operand, .. } => Self::extract_aggregates(operand, aggs),
                Expr::Binary { left, right, .. } => {
                    Self::extract_aggregates(left, aggs);
                    Self::extract_aggregates(right, aggs);
                }
                Expr::Cast { expr, .. } => Self::extract_aggregates(expr, aggs),
                Expr::Collate { expr, .. } => Self::extract_aggregates(expr, aggs),
                Expr::Like {
                    lhs, rhs, escape, ..
                } => {
                    Self::extract_aggregates(lhs, aggs);
                    Self::extract_aggregates(rhs, aggs);
                    if let Some(e) = escape {
                        Self::extract_aggregates(e, aggs);
                    }
                }
                Expr::IsNull { expr, .. } => Self::extract_aggregates(expr, aggs),
                Expr::Is { lhs, rhs, .. } => {
                    Self::extract_aggregates(lhs, aggs);
                    Self::extract_aggregates(rhs, aggs);
                }
                Expr::Between {
                    expr, low, high, ..
                } => {
                    Self::extract_aggregates(expr, aggs);
                    Self::extract_aggregates(low, aggs);
                    Self::extract_aggregates(high, aggs);
                }
                Expr::In { expr, .. } => Self::extract_aggregates(expr, aggs),
                Expr::Case { base, arms, else_ } => {
                    if let Some(b) = base {
                        Self::extract_aggregates(b, aggs);
                    }
                    for arm in arms {
                        Self::extract_aggregates(&arm.when, aggs);
                        Self::extract_aggregates(&arm.then, aggs);
                    }
                    if let Some(e) = else_ {
                        Self::extract_aggregates(e, aggs);
                    }
                }
                Expr::RowValue(exprs) => {
                    for e in exprs {
                        Self::extract_aggregates(e, aggs);
                    }
                }
                _ => {}
            }
        }
    }

    fn is_aggregate_query(body: &SimpleSelect) -> bool {
        if !body.group_by.is_empty() {
            return true;
        }
        if body.having.is_some() {
            return true;
        }
        let mut aggs = Vec::new();
        for rc in &body.result_columns {
            if let ResultColumn::Expr { expr, .. } = rc {
                Self::extract_aggregates(expr, &mut aggs);
            }
        }
        !aggs.is_empty()
    }

    fn compile_aggregate_select(
        &mut self,
        _select: &SelectStmt,
        body: &SimpleSelect,
        from: &FromClause,
    ) -> CodegenResult<()> {
        if from.tables.len() != 1 || !from.joins.is_empty() {
            return Err(CodegenError::NotImplemented);
        }
        let table_name = match &from.tables[0] {
            TableOrSubquery::Table { name, .. } => name.clone(),
            _ => return Err(CodegenError::NotImplemented),
        };

        let schema_obj = self
            .schema
            .ok_or_else(|| CodegenError::Schema("no schema context".to_string()))?
            .get(&table_name)
            .ok_or_else(|| CodegenError::Schema(format!("table '{}' not found", table_name)))?;
        let root_page = schema_obj.root_page;
        let schema_cols = schema_obj.columns.clone();

        let cursor_id = self.vm.n_cursors;
        self.vm.n_cursors += 1;

        let mut aggs = Vec::new();
        for rc in &body.result_columns {
            if let ResultColumn::Expr { expr, .. } = rc {
                Self::extract_aggregates(expr, &mut aggs);
            }
        }
        if let Some(having) = &body.having {
            Self::extract_aggregates(having, &mut aggs);
        }

        let mut agg_regs_assigned = Vec::new();
        for agg in &aggs {
            agg_regs_assigned.push((agg.clone(), self.vm.alloc_reg()));
        }
        self.agg_regs = agg_regs_assigned.clone();

        let has_group_by = !body.group_by.is_empty();
        let group_by_cursor = if has_group_by {
            let sorter = self.vm.n_cursors;
            self.vm.n_cursors += 1;
            self.vm.emit(VdbeOp {
                opcode: Opcode::SorterOpen,
                p1: sorter as i32,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            Some(sorter)
        } else {
            None
        };

        self.vm.emit(VdbeOp {
            opcode: Opcode::OpenRead,
            p1: cursor_id as i32,
            p2: root_page as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let rewind_addr = self.vm.emit(VdbeOp {
            opcode: Opcode::Rewind,
            p1: cursor_id as i32,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let loop_top = self.vm.ops.len();

        let next_label_patches = if let Some(where_expr) = &body.where_ {
            self.compile_where_expr(where_expr, Some((cursor_id, &schema_cols)))?
        } else {
            vec![]
        };

        if let Some(sorter) = group_by_cursor {
            let mut group_regs = Vec::new();
            for gb_expr in &body.group_by {
                group_regs.push(self.compile_expr(gb_expr, Some((cursor_id, &schema_cols)))?);
            }

            let mut all_agg_args = Vec::new();
            for agg in &aggs {
                if let Expr::Function {
                    args:
                        liter_ast::FunctionArgs::List(exprs) | liter_ast::FunctionArgs::Distinct(exprs),
                    ..
                } = agg
                {
                    for e in exprs {
                        all_agg_args.push(self.compile_expr(e, Some((cursor_id, &schema_cols)))?);
                    }
                }
            }

            let total_fields = group_regs.len() + all_agg_args.len();
            let record_start = if total_fields > 0 {
                let start = self.vm.alloc_reg();
                for _ in 1..total_fields {
                    self.vm.alloc_reg();
                }
                for (i, &r) in group_regs.iter().chain(all_agg_args.iter()).enumerate() {
                    self.vm.emit(VdbeOp {
                        opcode: Opcode::Copy,
                        p1: r as i32,
                        p2: (start + i) as i32,
                        p3: 0,
                        p4: P4::None,
                        p5: 0,
                    });
                }
                start
            } else {
                0
            };

            let record_reg = self.vm.alloc_reg();
            self.vm.emit(VdbeOp {
                opcode: Opcode::MakeRecord,
                p1: record_start as i32,
                p2: total_fields as i32,
                p3: record_reg as i32,
                p4: P4::None,
                p5: 0,
            });
            self.vm.emit(VdbeOp {
                opcode: Opcode::SorterInsert,
                p1: sorter as i32,
                p2: record_reg as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
        } else {
            for (agg, dest_reg) in &agg_regs_assigned {
                if let Expr::Function { name, args, .. } = agg {
                    let mut arg_regs = Vec::new();
                    let argc = match args {
                        liter_ast::FunctionArgs::List(exprs)
                        | liter_ast::FunctionArgs::Distinct(exprs) => {
                            for e in exprs {
                                arg_regs
                                    .push(self.compile_expr(e, Some((cursor_id, &schema_cols)))?);
                            }
                            exprs.len()
                        }
                        _ => 0,
                    };
                    let arg_start = if argc > 0 {
                        let start = self.vm.alloc_reg();
                        for _ in 1..argc {
                            self.vm.alloc_reg();
                        }
                        for (i, &r) in arg_regs.iter().enumerate() {
                            self.vm.emit(VdbeOp {
                                opcode: Opcode::Copy,
                                p1: r as i32,
                                p2: (start + i) as i32,
                                p3: 0,
                                p4: P4::None,
                                p5: 0,
                            });
                        }
                        start
                    } else {
                        0
                    };

                    self.vm.emit(VdbeOp {
                        opcode: Opcode::AggStep,
                        p1: argc as i32,
                        p2: arg_start as i32,
                        p3: *dest_reg as i32,
                        p4: P4::Text(std::sync::Arc::from(name.as_ref())),
                        p5: 0,
                    });
                }
            }
        }

        let next_addr = self.vm.emit(VdbeOp {
            opcode: Opcode::Next,
            p1: cursor_id as i32,
            p2: loop_top as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let end_table_loop = self.vm.ops.len();
        self.vm.ops[rewind_addr].p2 = end_table_loop as i32;
        for patch in next_label_patches {
            self.vm.ops[patch].p2 = next_addr as i32;
        }

        let result_reg_start = self.vm.alloc_reg();
        for _ in 1..body.result_columns.len() {
            self.vm.alloc_reg();
        }

        if let Some(sorter) = group_by_cursor {
            let _sort_top = self.vm.ops.len();
            let sorter_sort = self.vm.emit(VdbeOp {
                opcode: Opcode::SorterSort,
                p1: sorter as i32,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            let first_row_flag = self.vm.alloc_reg();
            self.vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: 1,
                p2: first_row_flag as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });

            let sorter_loop = self.vm.ops.len();

            let mut prev_keys = Vec::new();
            for _ in &body.group_by {
                prev_keys.push(self.vm.alloc_reg());
            }
            let mut curr_keys = Vec::new();
            for _ in &body.group_by {
                curr_keys.push(self.vm.alloc_reg());
            }

            for (i, reg) in curr_keys.iter().enumerate() {
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Column,
                    p1: sorter as i32,
                    p2: i as i32,
                    p3: *reg as i32,
                    p4: P4::None,
                    p5: 0,
                });
            }
            let z_reg = self.vm.alloc_reg();
            self.vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: 0,
                p2: z_reg as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            let jump_first = self.vm.emit(VdbeOp {
                opcode: Opcode::Ne,
                p1: first_row_flag as i32,
                p2: 0,
                p3: z_reg as i32,
                p4: P4::None,
                p5: 0,
            });

            let mut ne_jumps = Vec::new();
            for (curr, prev) in curr_keys.iter().zip(prev_keys.iter()) {
                ne_jumps.push(self.vm.emit(VdbeOp {
                    opcode: Opcode::Ne,
                    p1: *curr as i32,
                    p2: 0,
                    p3: *prev as i32,
                    p4: P4::None,
                    p5: 0,
                }));
            }
            let jump_same = self.vm.emit(VdbeOp {
                opcode: Opcode::Goto,
                p1: 0,
                p2: 0,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });

            let group_changed_addr = self.vm.ops.len();
            for j in ne_jumps {
                self.vm.ops[j].p2 = group_changed_addr as i32;
            }

            for (agg, dest_reg) in &agg_regs_assigned {
                if let Expr::Function { name, .. } = agg {
                    self.vm.emit(VdbeOp {
                        opcode: Opcode::AggFinal,
                        p1: *dest_reg as i32,
                        p2: 0,
                        p3: 0,
                        p4: P4::Text(std::sync::Arc::from(name.as_ref())),
                        p5: 0,
                    });
                }
            }

            let mut having_jump = None;

            // Map GROUP BY expressions to their extracted registers so that HAVING and SELECT don't access the table cursor.
            for (gb_expr, reg) in body.group_by.iter().zip(prev_keys.iter()) {
                println!("pushing group by to agg_regs: {:?} -> {}", gb_expr, *reg);
                self.agg_regs.push((gb_expr.clone(), *reg));
            }

            if let Some(having) = &body.having {
                println!("compiling having: {:?}", having);
                let h_reg = self.compile_expr(having, Some((cursor_id, &schema_cols)))?;
                having_jump = Some(self.vm.emit(VdbeOp {
                    opcode: Opcode::Eq,
                    p1: h_reg as i32,
                    p2: 0,
                    p3: z_reg as i32,
                    p4: P4::None,
                    p5: 0,
                }));
            }

            for (i, rc) in body.result_columns.iter().enumerate() {
                match rc {
                    ResultColumn::Expr { expr, .. } => {
                        println!("compiling result column expr: {:?}", expr);
                        let r = self.compile_expr(expr, Some((cursor_id, &schema_cols)))?;
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Copy,
                            p1: r as i32,
                            p2: (result_reg_start + i) as i32,
                            p3: 0,
                            p4: P4::None,
                            p5: 0,
                        });
                    }
                    _ => return Err(CodegenError::NotImplemented),
                }
            }

            self.agg_regs.truncate(agg_regs_assigned.len());

            self.vm.emit(VdbeOp {
                opcode: Opcode::ResultRow,
                p1: result_reg_start as i32,
                p2: body.result_columns.len() as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });

            let skip_yield = self.vm.ops.len();
            if let Some(hj) = having_jump {
                self.vm.ops[hj].p2 = skip_yield as i32;
            }

            for (_, dest_reg) in &agg_regs_assigned {
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Null,
                    p1: 0,
                    p2: *dest_reg as i32,
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                });
            }

            let first_row_addr = self.vm.ops.len();
            self.vm.ops[jump_first].p2 = first_row_addr as i32;
            self.vm.emit(VdbeOp {
                opcode: Opcode::Integer,
                p1: 0,
                p2: first_row_flag as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            for (curr, prev) in curr_keys.iter().zip(prev_keys.iter()) {
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Copy,
                    p1: *curr as i32,
                    p2: *prev as i32,
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                });
            }

            let same_group_addr = self.vm.ops.len();
            self.vm.ops[jump_same].p2 = same_group_addr as i32;

            let mut arg_col_idx = body.group_by.len();
            for (agg, dest_reg) in &agg_regs_assigned {
                if let Expr::Function { name, args, .. } = agg {
                    let argc = match args {
                        liter_ast::FunctionArgs::List(exprs)
                        | liter_ast::FunctionArgs::Distinct(exprs) => exprs.len(),
                        _ => 0,
                    };
                    let arg_start = if argc > 0 {
                        let start = self.vm.alloc_reg();
                        for _ in 1..argc {
                            self.vm.alloc_reg();
                        }
                        for i in 0..argc {
                            self.vm.emit(VdbeOp {
                                opcode: Opcode::Column,
                                p1: sorter as i32,
                                p2: (arg_col_idx + i) as i32,
                                p3: (start + i) as i32,
                                p4: P4::None,
                                p5: 0,
                            });
                        }
                        arg_col_idx += argc;
                        start
                    } else {
                        0
                    };
                    self.vm.emit(VdbeOp {
                        opcode: Opcode::AggStep,
                        p1: argc as i32,
                        p2: arg_start as i32,
                        p3: *dest_reg as i32,
                        p4: P4::Text(std::sync::Arc::from(name.as_ref())),
                        p5: 0,
                    });
                }
            }

            self.vm.emit(VdbeOp {
                opcode: Opcode::SorterNext,
                p1: sorter as i32,
                p2: sorter_loop as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });

            let end_sorter_loop = self.vm.ops.len();
            self.vm.ops[sorter_sort].p2 = end_sorter_loop as i32;

            // final yield for the last group
            for (agg, dest_reg) in &agg_regs_assigned {
                if let Expr::Function { name, .. } = agg {
                    self.vm.emit(VdbeOp {
                        opcode: Opcode::AggFinal,
                        p1: *dest_reg as i32,
                        p2: 0,
                        p3: 0,
                        p4: P4::Text(std::sync::Arc::from(name.as_ref())),
                        p5: 0,
                    });
                }
            }

            for (gb_expr, reg) in body.group_by.iter().zip(curr_keys.iter()) {
                self.agg_regs.push((gb_expr.clone(), *reg));
            }

            let mut having_jump_final = None;
            if let Some(having) = &body.having {
                let h_reg = self.compile_expr(having, Some((cursor_id, &schema_cols)))?;
                having_jump_final = Some(self.vm.emit(VdbeOp {
                    opcode: Opcode::Eq,
                    p1: h_reg as i32,
                    p2: 0,
                    p3: z_reg as i32,
                    p4: P4::None,
                    p5: 0,
                }));
            }
            for (i, rc) in body.result_columns.iter().enumerate() {
                match rc {
                    ResultColumn::Expr { expr, .. } => {
                        let r = self.compile_expr(expr, Some((cursor_id, &schema_cols)))?;
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Copy,
                            p1: r as i32,
                            p2: (result_reg_start + i) as i32,
                            p3: 0,
                            p4: P4::None,
                            p5: 0,
                        });
                    }
                    _ => return Err(CodegenError::NotImplemented),
                }
            }

            self.agg_regs.truncate(agg_regs_assigned.len());

            self.vm.emit(VdbeOp {
                opcode: Opcode::ResultRow,
                p1: result_reg_start as i32,
                p2: body.result_columns.len() as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            let skip_final_yield = self.vm.ops.len();
            if let Some(hj) = having_jump_final {
                self.vm.ops[hj].p2 = skip_final_yield as i32;
            }
        } else {
            for (agg, dest_reg) in &agg_regs_assigned {
                if let Expr::Function { name, .. } = agg {
                    self.vm.emit(VdbeOp {
                        opcode: Opcode::AggFinal,
                        p1: *dest_reg as i32,
                        p2: 0,
                        p3: 0,
                        p4: P4::Text(std::sync::Arc::from(name.as_ref())),
                        p5: 0,
                    });
                }
            }

            let mut having_jump = None;
            if let Some(having) = &body.having {
                let h_reg = self.compile_expr(having, Some((cursor_id, &schema_cols)))?;
                let z_reg = self.vm.alloc_reg();
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Integer,
                    p1: 0,
                    p2: z_reg as i32,
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                });
                having_jump = Some(self.vm.emit(VdbeOp {
                    opcode: Opcode::Eq,
                    p1: h_reg as i32,
                    p2: 0,
                    p3: z_reg as i32,
                    p4: P4::None,
                    p5: 0,
                }));
            }

            for (i, rc) in body.result_columns.iter().enumerate() {
                match rc {
                    ResultColumn::Expr { expr, .. } => {
                        let r = self.compile_expr(expr, Some((cursor_id, &schema_cols)))?;
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Copy,
                            p1: r as i32,
                            p2: (result_reg_start + i) as i32,
                            p3: 0,
                            p4: P4::None,
                            p5: 0,
                        });
                    }
                    _ => return Err(CodegenError::NotImplemented),
                }
            }
            self.vm.emit(VdbeOp {
                opcode: Opcode::ResultRow,
                p1: result_reg_start as i32,
                p2: body.result_columns.len() as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
            let skip_yield = self.vm.ops.len();
            if let Some(hj) = having_jump {
                self.vm.ops[hj].p2 = skip_yield as i32;
            }
        }

        Ok(())
    }

    /// Compile a generic WHERE expression, returning a list of instruction indicesses
    /// whose `p2` (jump target) must be patched to the "skip row" label.
    ///
    /// Strategy: evaluate the predicate into a register, then emit `IfNot` to
    /// skip the `ResultRow`. Complex AND/OR are short-circuited.
    fn compile_where_expr(
        &mut self,
        expr: &Expr,
        cursor_ctx: Option<(usize, &[liter_ast::ColumnDef])>,
    ) -> CodegenResult<Vec<usize>> {
        match expr {
            // AND: both sides must be true; short-circuit on first false
            Expr::Binary {
                op: BinaryOp::And,
                left,
                right,
            } => {
                let mut patches = self.compile_where_expr(left, cursor_ctx)?;
                patches.extend(self.compile_where_expr(right, cursor_ctx)?);
                Ok(patches)
            }
            // OR: at least one must be true — emit both and OR their results
            Expr::Binary {
                op: BinaryOp::Or,
                left,
                right,
            } => {
                let r_left = self.compile_expr(left, cursor_ctx)?;
                let r_right = self.compile_expr(right, cursor_ctx)?;
                let r_or = self.vm.alloc_reg();
                // r_or = r_left OR r_right (truthy)
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Copy,
                    p1: r_left as i32,
                    p2: r_or as i32,
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                });
                // if r_or is already truthy, skip checking right
                let skip_right = self.vm.emit(VdbeOp {
                    opcode: Opcode::If,
                    p1: r_or as i32,
                    p2: 0, // patched
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                });
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Copy,
                    p1: r_right as i32,
                    p2: r_or as i32,
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                });
                let after_or = self.vm.ops.len();
                self.vm.ops[skip_right].p2 = after_or as i32;

                let ifnot_addr = self.vm.emit(VdbeOp {
                    opcode: Opcode::IfNot,
                    p1: r_or as i32,
                    p2: 0, // patched by caller
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                });
                Ok(vec![ifnot_addr])
            }
            // IsNull / NotNull checks
            Expr::IsNull { not, expr: inner } => {
                let r = self.compile_expr(inner, cursor_ctx)?;
                let opcode = if *not {
                    Opcode::NotNull
                } else {
                    Opcode::IsNull
                };
                // IsNull jumps to p2 when TRUE (it IS null), but we want to
                // skip when the condition fails.  Invert: if NOT (is null) skip.
                let check_addr = self.vm.emit(VdbeOp {
                    opcode: if *not {
                        Opcode::IsNull
                    } else {
                        Opcode::NotNull
                    },
                    p1: r as i32,
                    p2: 0, // patched by caller to "skip row"
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                });
                let _ = opcode; // silence unused
                Ok(vec![check_addr])
            }
            // All other predicates: evaluate to a boolean register, then IfNot-skip
            _ => {
                let r_pred = self.compile_expr(expr, cursor_ctx)?;
                let ifnot_addr = self.vm.emit(VdbeOp {
                    opcode: Opcode::IfNot,
                    p1: r_pred as i32,
                    p2: 0, // patched by caller
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
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
        cursor_ctx: Option<(usize, &[liter_ast::ColumnDef])>,
    ) -> CodegenResult<usize> {
        println!("compile_expr: searching for expr: {:?}", expr);
        println!("compile_expr: self.agg_regs: {:?}", self.agg_regs);
        if let Some((_, reg)) = self.agg_regs.iter().find(|(e, _)| e == expr) {
            println!("compile_expr: found in agg_regs! returning {}", reg);
            return Ok(*reg);
        }

        match expr {
            Expr::Literal(val) => {
                let r = self.vm.alloc_reg();
                match val {
                    LiteralValue::Integer(i) => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Integer,
                            p1: *i as i32,
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
                    LiteralValue::Text(s) => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::String8,
                            p1: 0,
                            p2: r as i32,
                            p3: 0,
                            p4: P4::Text(Arc::from(s.clone().into_boxed_str())),
                            p5: 0,
                        });
                    }
                    LiteralValue::True => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Integer,
                            p1: 1,
                            p2: r as i32,
                            p3: 0,
                            p4: P4::None,
                            p5: 0,
                        });
                    }
                    LiteralValue::False => {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Integer,
                            p1: 0,
                            p2: r as i32,
                            p3: 0,
                            p4: P4::None,
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

            Expr::Column {
                name,
                table: _table,
                ..
            } => {
                let (cursor_id, schema_cols) = cursor_ctx.ok_or_else(|| {
                    CodegenError::Schema(format!("column '{}' used outside FROM context", name))
                })?;
                let col_idx = schema_cols
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(name))
                    .ok_or_else(|| CodegenError::Schema(format!("column '{}' not found", name)))?;
                let r = self.vm.alloc_reg();
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Column,
                    p1: cursor_id as i32,
                    p2: col_idx as i32,
                    p3: r as i32,
                    p4: P4::None,
                    p5: 0,
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
                    BinaryOp::Eq => Opcode::Eq,
                    BinaryOp::Ne => Opcode::Ne,
                    BinaryOp::Lt => Opcode::Lt,
                    BinaryOp::Le => Opcode::Le,
                    BinaryOp::Gt => Opcode::Gt,
                    BinaryOp::Ge => Opcode::Ge,
                    _ => return Err(CodegenError::NotImplemented),
                };

                if self.is_comparison_op(op) {
                    // Comparison ops in SQLite VDBE are conditional jumps, not value producers.
                    // Materialise as: r_res = 0; if cond goto L1; goto L2; L1: r_res = 1; L2:
                    self.vm.emit(VdbeOp {
                        opcode: Opcode::Integer,
                        p1: 0,
                        p2: r_res as i32,
                        p3: 0,
                        p4: P4::None,
                        p5: 0,
                    });
                    // Compare LHS(p3) OP RHS(p1). So p3 = r_left, p1 = r_right.
                    let jump_addr = self.vm.emit(VdbeOp {
                        opcode,
                        p1: r_right as i32,
                        p2: 0,
                        p3: r_left as i32,
                        p4: P4::None,
                        p5: 0,
                    });
                    let end_addr = self.vm.emit(VdbeOp {
                        opcode: Opcode::Goto,
                        p1: 0,
                        p2: 0,
                        p3: 0,
                        p4: P4::None,
                        p5: 0,
                    });
                    let true_addr = self.vm.emit(VdbeOp {
                        opcode: Opcode::Integer,
                        p1: 1,
                        p2: r_res as i32,
                        p3: 0,
                        p4: P4::None,
                        p5: 0,
                    });
                    let post_addr = self.vm.ops.len();
                    self.vm.ops[jump_addr].p2 = true_addr as i32;
                    self.vm.ops[end_addr].p2 = post_addr as i32;
                } else {
                    // Arithmetic ops: LHS(p2) OP RHS(p1) -> dest(p3)
                    self.vm.emit(VdbeOp {
                        opcode,
                        p1: r_right as i32,
                        p2: r_left as i32,
                        p3: r_res as i32,
                        p4: P4::None,
                        p5: 0,
                    });
                }
                Ok(r_res)
            }

            Expr::Unary {
                op: liter_ast::UnaryOp::Minus,
                operand,
            } => {
                let r_inner = self.compile_expr(operand, cursor_ctx)?;
                let r = self.vm.alloc_reg();
                // Negate: emit 0 - inner
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Integer,
                    p1: 0,
                    p2: r as i32,
                    p3: 0,
                    p4: P4::None,
                    p5: 0,
                });
                self.vm.emit(VdbeOp {
                    opcode: Opcode::SubtractInt,
                    p1: r as i32,
                    p2: r_inner as i32,
                    p3: r as i32,
                    p4: P4::None,
                    p5: 0,
                });
                Ok(r)
            }

            Expr::Function { name, args, .. } => {
                // Compile all arguments first
                let mut arg_regs = Vec::new();
                let argc = match args {
                    liter_ast::FunctionArgs::Star => 0, // COUNT(*)
                    liter_ast::FunctionArgs::None => 0,
                    liter_ast::FunctionArgs::List(exprs)
                    | liter_ast::FunctionArgs::Distinct(exprs) => {
                        for expr in exprs {
                            arg_regs.push(self.compile_expr(expr, cursor_ctx)?);
                        }
                        exprs.len()
                    }
                };

                let p2_start = if argc > 0 {
                    let start = self.vm.alloc_reg();
                    for _ in 1..argc {
                        self.vm.alloc_reg();
                    }
                    // Copy evaluated args into contiguous block
                    for (i, &r) in arg_regs.iter().enumerate() {
                        self.vm.emit(VdbeOp {
                            opcode: Opcode::Copy,
                            p1: r as i32,
                            p2: (start + i) as i32,
                            p3: 0,
                            p4: P4::None,
                            p5: 0,
                        });
                    }
                    start
                } else {
                    0
                };

                let dest_reg = self.vm.alloc_reg();

                // For COUNT(*), we pass argc=0 and let the function implementation handle it.
                self.vm.emit(VdbeOp {
                    opcode: Opcode::Function,
                    p1: argc as i32,
                    p2: p2_start as i32,
                    p3: dest_reg as i32,
                    p4: P4::Text(Arc::from(name.as_ref())),
                    p5: 0,
                });

                Ok(dest_reg)
            }

            _ => Err(CodegenError::NotImplemented),
        }
    }

    fn is_comparison_op(&self, op: &BinaryOp) -> bool {
        matches!(
            op,
            BinaryOp::Eq | BinaryOp::Ne | BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge
        )
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
        let schema = self
            .schema
            .ok_or_else(|| CodegenError::Schema("No schema provided".to_string()))?;
        let table_name = &insert.table;
        let table = schema
            .get(table_name)
            .ok_or_else(|| CodegenError::Schema(format!("Table not found: {}", table_name)))?;

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

                if regs.is_empty() {
                    continue;
                }

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

    /// Compile `DELETE FROM table [WHERE expr]`.
    ///
    /// Bytecode layout:
    /// ```text
    /// OpenWrite(cursor, root_page)
    /// Rewind(cursor, end_label)
    /// loop_top:
    ///   [WHERE: IfNot(pred, skip_label)]
    ///   Delete(cursor)
    ///   Next(cursor, loop_top, p5=1)   ← post-delete: cursor already repositioned
    ///   Goto(end_label)                ← Next fell through → done
    /// skip_label:
    ///   Next(cursor, loop_top, p5=0)   ← normal advance for non-deleted rows
    /// end_label:
    ///   Close(cursor)
    /// ```
    fn compile_delete(&mut self, delete: &DeleteStmt) -> CodegenResult<()> {
        let schema = self
            .schema
            .ok_or_else(|| CodegenError::Schema("no schema context".to_string()))?;
        let table_name = &delete.table.name;
        let schema_obj = schema
            .get(table_name)
            .ok_or_else(|| CodegenError::Schema(format!("table '{}' not found", table_name)))?;
        let root_page = schema_obj.root_page;
        let schema_cols = schema_obj.columns.clone();

        let cursor_id = self.vm.n_cursors;
        self.vm.n_cursors += 1;

        self.vm.emit(VdbeOp {
            opcode: Opcode::OpenWrite,
            p1: cursor_id as i32,
            p2: root_page as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let rewind_addr = self.vm.emit(VdbeOp {
            opcode: Opcode::Rewind,
            p1: cursor_id as i32,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let loop_top = self.vm.ops.len();

        // WHERE predicate: on mismatch jump to skip_label (patched below)
        let where_patches = if let Some(where_expr) = &delete.where_ {
            self.compile_where_expr(where_expr, Some((cursor_id, &schema_cols)))?
        } else {
            vec![]
        };

        // Delete current row (cursor repositions to next row or becomes Invalid)
        self.vm.emit(VdbeOp {
            opcode: Opcode::Delete,
            p1: cursor_id as i32,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        // Post-delete Next: p5=1 means "cursor may already be valid at next row"
        self.vm.emit(VdbeOp {
            opcode: Opcode::Next,
            p1: cursor_id as i32,
            p2: loop_top as i32,
            p3: 0,
            p4: P4::None,
            p5: 1,
        });
        // If Next fell through here, the table is exhausted — jump to end
        let goto_end_addr = self.vm.emit(VdbeOp {
            opcode: Opcode::Goto,
            p1: 0,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        // skip_label: WHERE-skipped rows land here and use a normal Next
        let skip_label = self.vm.ops.len();
        for addr in where_patches {
            self.vm.ops[addr].p2 = skip_label as i32;
        }
        self.vm.emit(VdbeOp {
            opcode: Opcode::Next,
            p1: cursor_id as i32,
            p2: loop_top as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let end_label = self.vm.ops.len();
        self.vm.ops[rewind_addr].p2 = end_label as i32;
        self.vm.ops[goto_end_addr].p2 = end_label as i32;

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

    /// Compile `UPDATE table SET col = expr [, ...] [WHERE expr]`.
    ///
    /// Strategy: full-table scan; for each matching row, read all columns,
    /// apply the assignment expressions, re-encode the record, delete the old
    /// row, and insert the new record at the same rowid.
    fn compile_update(&mut self, update: &UpdateStmt) -> CodegenResult<()> {
        let schema = self
            .schema
            .ok_or_else(|| CodegenError::Schema("no schema context".to_string()))?;
        let table_name = &update.table.name;
        let schema_obj = schema
            .get(table_name)
            .ok_or_else(|| CodegenError::Schema(format!("table '{}' not found", table_name)))?;
        let root_page = schema_obj.root_page;
        let schema_cols = schema_obj.columns.clone();
        let n_cols = schema_cols.len();

        let cursor_id = self.vm.n_cursors;
        self.vm.n_cursors += 1;

        self.vm.emit(VdbeOp {
            opcode: Opcode::OpenWrite,
            p1: cursor_id as i32,
            p2: root_page as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        let rewind_addr = self.vm.emit(VdbeOp {
            opcode: Opcode::Rewind,
            p1: cursor_id as i32,
            p2: 0,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let loop_top = self.vm.ops.len();

        // WHERE predicate
        let next_label_patches = if let Some(where_expr) = &update.where_ {
            self.compile_where_expr(where_expr, Some((cursor_id, &schema_cols)))?
        } else {
            vec![]
        };

        // Read all columns into contiguous registers, then overwrite with SET values.
        let base_reg = self.vm.alloc_reg();
        for _ in 1..n_cols {
            self.vm.alloc_reg();
        }

        for col_idx in 0..n_cols {
            self.vm.emit(VdbeOp {
                opcode: Opcode::Column,
                p1: cursor_id as i32,
                p2: col_idx as i32,
                p3: (base_reg + col_idx) as i32,
                p4: P4::None,
                p5: 0,
            });
        }

        // Apply SET assignments: overwrite the target column register.
        for assignment in &update.assignments {
            let col_name = assignment
                .columns
                .first()
                .ok_or_else(|| CodegenError::Internal("empty assignment".to_string()))?;
            let col_idx = schema_cols
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(col_name))
                .ok_or_else(|| CodegenError::Schema(format!("column '{}' not found", col_name)))?;
            // Compile new value expression; write into target register directly.
            let r_val = self.compile_expr(&assignment.value, Some((cursor_id, &schema_cols)))?;
            self.vm.emit(VdbeOp {
                opcode: Opcode::Copy,
                p1: r_val as i32,
                p2: (base_reg + col_idx) as i32,
                p3: 0,
                p4: P4::None,
                p5: 0,
            });
        }

        // Capture the current rowid before deleting.
        let rowid_reg = self.vm.alloc_reg();
        self.vm.emit(VdbeOp {
            opcode: Opcode::RowId,
            p1: cursor_id as i32,
            p2: rowid_reg as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        // Re-encode the updated row.
        let record_reg = self.vm.alloc_reg();
        self.vm.emit(VdbeOp {
            opcode: Opcode::MakeRecord,
            p1: base_reg as i32,
            p2: n_cols as i32,
            p3: record_reg as i32,
            p4: P4::None,
            p5: 0,
        });

        // Delete old row, then insert updated row at the same rowid.
        self.vm.emit(VdbeOp {
            opcode: Opcode::Delete,
            p1: cursor_id as i32,
            p2: 0,
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

        // After Delete+Insert the cursor is invalidated by Insert.
        // Use SeekGt(rowid_reg) to find the next row after the updated one.
        // If not found, SeekGt jumps to end_label (patched below).
        let seekgt_addr = self.vm.emit(VdbeOp {
            opcode: Opcode::SeekGt,
            p1: cursor_id as i32,
            p2: 0,
            p3: rowid_reg as i32,
            p4: P4::None,
            p5: 0,
        });

        // Found a row after the updated one — jump back to loop body.
        self.vm.emit(VdbeOp {
            opcode: Opcode::Goto,
            p1: 0,
            p2: loop_top as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });

        // Patch next_label: WHERE-skipped rows jump here (to the normal Next opcode).
        let next_label = self.vm.ops.len();
        for addr in next_label_patches {
            self.vm.ops[addr].p2 = next_label as i32;
        }

        // For rows that did NOT match WHERE, use normal Next to advance.
        let next_addr = self.vm.emit(VdbeOp {
            opcode: Opcode::Next,
            p1: cursor_id as i32,
            p2: loop_top as i32,
            p3: 0,
            p4: P4::None,
            p5: 0,
        });
        let _ = next_addr;

        let end_label = self.vm.ops.len();
        self.vm.ops[rewind_addr].p2 = end_label as i32;
        self.vm.ops[seekgt_addr].p2 = end_label as i32;

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
