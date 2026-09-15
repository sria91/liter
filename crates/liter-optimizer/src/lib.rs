//! Query planner for Liter-rs.
//!
//! Mirrors `where.c` / `whereInt.h`. Analyses WHERE clauses, selects indexes,
//! and produces a query plan consumed by the code generator.

use liter_ast::*;
use liter_schema::Schema;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum OptimizeError {
    #[error("not yet implemented")]
    NotImplemented,
}

pub type OptimizeResult<T> = Result<T, OptimizeError>;

/// A single loop in the query plan (one scan or index lookup per table).
#[derive(Debug, Clone, PartialEq)]
pub struct QueryLoop {
    pub table: String,
    pub scan_kind: ScanKind,
    pub estimated_rows: f64,
}

/// How a table scan is performed.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanKind {
    /// Full sequential scan.
    FullScan,
    /// Index scan using the named index.
    IndexScan {
        index: String,
        constraints: Vec<String>,
    },
    /// Row-id lookup (single row).
    RowIdLookup,
}

/// The output of the query planner for a SELECT.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryPlan {
    pub loops: Vec<QueryLoop>,
}

impl QueryPlan {
    pub fn explain_string(&self) -> String {
        let mut out = String::new();
        for (i, lp) in self.loops.iter().enumerate() {
            let scan = match &lp.scan_kind {
                ScanKind::FullScan => format!("SCAN {}", lp.table),
                ScanKind::IndexScan { index, .. } => {
                    format!("SEARCH {} USING INDEX {}", lp.table, index)
                }
                ScanKind::RowIdLookup => format!("SEARCH {} USING INTEGER PRIMARY KEY", lp.table),
            };
            out.push_str(&format!("{i}: {scan} (~{:.0} rows)\n", lp.estimated_rows));
        }
        out
    }
}

pub struct Optimizer<'a> {
    schema: &'a Schema,
}

impl<'a> Optimizer<'a> {
    pub fn new(schema: &'a Schema) -> Self {
        Self { schema }
    }

    pub fn optimize_stmt(&self, stmt: &Stmt) -> OptimizeResult<QueryPlan> {
        match stmt {
            Stmt::Select(select) => self.optimize_select(select),
            _ => Err(OptimizeError::NotImplemented),
        }
    }

    fn optimize_select(&self, select: &SelectStmt) -> OptimizeResult<QueryPlan> {
        let body = match &select.body {
            SelectBody::Simple(s) => s,
            _ => return Err(OptimizeError::NotImplemented),
        };

        // For this phase, we only support optimizing a single table FROM clause.
        let (scan_table_name, physical_table_name) = if let Some(from) = &body.from {
            if from.tables.len() == 1 {
                match &from.tables[0] {
                    TableOrSubquery::Table { alias, name, .. } => {
                        (alias.clone().unwrap_or_else(|| name.clone()), name.clone())
                    }
                    _ => return Err(OptimizeError::NotImplemented),
                }
            } else {
                return Err(OptimizeError::NotImplemented); // Joins not supported yet
            }
        } else {
            return Ok(QueryPlan { loops: vec![] }); // SELECT without FROM
        };

        // Extract constraints targeting this table from the WHERE clause
        let mut constraints = Vec::new();
        if let Some(where_expr) = &body.where_ {
            self.extract_constraints(
                where_expr,
                &scan_table_name,
                &physical_table_name,
                &mut constraints,
            );
        }

        // Fetch schema indexes for the underlying physical table
        let indexes = self.schema.indexes_for(&physical_table_name);

        // Determine best scan path based on heuristics
        let scan_kind = if constraints.iter().any(|c| c == "id" || c == "rowid") {
            ScanKind::RowIdLookup
        } else if let Some(idx) = indexes.iter().find(|_idx| {
            // Very naive heuristic: check if any constraint matches an index
            // In a real optimizer, we'd look at index columns. Since our SchemaObject
            // stub doesn't map index columns yet, we just check if constraints are present.
            !constraints.is_empty()
        }) {
            ScanKind::IndexScan {
                index: idx.name.clone(),
                constraints: constraints.clone(),
            }
        } else {
            ScanKind::FullScan
        };

        let estimated_rows = match scan_kind {
            ScanKind::RowIdLookup => 1.0,
            ScanKind::IndexScan { .. } => 10.0,
            ScanKind::FullScan => 1000.0,
        };

        Ok(QueryPlan {
            loops: vec![QueryLoop {
                table: scan_table_name,
                scan_kind,
                estimated_rows,
            }],
        })
    }

    fn extract_constraints(
        &self,
        expr: &Expr,
        target_table: &str,
        physical_table: &str,
        constraints: &mut Vec<String>,
    ) {
        if let Expr::Binary { op, left, right } = expr {
            if matches!(
                op,
                BinaryOp::Eq | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Lt | BinaryOp::Le
            ) {
                if let Expr::Column { table, name, .. } = &**left {
                    if table.is_none()
                        || table.as_deref() == Some(target_table)
                        || table.as_deref() == Some(physical_table)
                    {
                        constraints.push(name.clone());
                    }
                }
                if let Expr::Column { table, name, .. } = &**right {
                    if table.is_none()
                        || table.as_deref() == Some(target_table)
                        || table.as_deref() == Some(physical_table)
                    {
                        constraints.push(name.clone());
                    }
                }
            }
            self.extract_constraints(left, target_table, physical_table, constraints);
            self.extract_constraints(right, target_table, physical_table, constraints);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liter_schema::{ObjectKind, SchemaObject};

    fn make_schema() -> Schema {
        let schema = Schema::new();
        schema.insert(SchemaObject {
            kind: ObjectKind::Table,
            name: "users".to_string(),
            tbl_name: "users".to_string(),
            root_page: 2,
            sql: None,
            columns: vec![],
        });
        schema
    }

    fn simple_select(body: SimpleSelect) -> SelectStmt {
        SelectStmt {
            with: None,
            body: SelectBody::Simple(body),
            order_by: vec![],
            limit: None,
        }
    }

    fn empty_simple_select() -> SimpleSelect {
        SimpleSelect {
            distinct: DistinctKind::All,
            result_columns: vec![],
            from: None,
            where_: None,
            group_by: vec![],
            having: None,
            window: vec![],
        }
    }

    fn table_ref(name: &str) -> TableOrSubquery {
        TableOrSubquery::Table {
            schema: None,
            name: name.to_string(),
            alias: None,
            indexed: IndexedKind::None,
        }
    }

    #[test]
    fn explain_string_formats_all_scan_kinds() {
        let plan = QueryPlan {
            loops: vec![
                QueryLoop {
                    table: "users".to_string(),
                    scan_kind: ScanKind::FullScan,
                    estimated_rows: 1000.0,
                },
                QueryLoop {
                    table: "users".to_string(),
                    scan_kind: ScanKind::IndexScan {
                        index: "idx_email".to_string(),
                        constraints: vec!["email".to_string()],
                    },
                    estimated_rows: 10.0,
                },
                QueryLoop {
                    table: "users".to_string(),
                    scan_kind: ScanKind::RowIdLookup,
                    estimated_rows: 1.0,
                },
            ],
        };
        let out = plan.explain_string();
        assert!(out.contains("SCAN users"));
        assert!(out.contains("SEARCH users USING INDEX idx_email"));
        assert!(out.contains("SEARCH users USING INTEGER PRIMARY KEY"));
    }

    #[test]
    fn optimize_stmt_non_select_not_implemented() {
        let schema = make_schema();
        let optimizer = Optimizer::new(&schema);
        let stmt = Stmt::Commit;
        assert_eq!(
            optimizer.optimize_stmt(&stmt),
            Err(OptimizeError::NotImplemented)
        );
    }

    #[test]
    fn optimize_select_compound_body_not_implemented() {
        let schema = make_schema();
        let optimizer = Optimizer::new(&schema);
        let select = SelectStmt {
            with: None,
            body: SelectBody::Compound {
                op: CompoundOp::Union,
                left: Box::new(SelectBody::Simple(empty_simple_select())),
                right: Box::new(SelectBody::Simple(empty_simple_select())),
            },
            order_by: vec![],
            limit: None,
        };
        assert_eq!(
            optimizer.optimize_stmt(&Stmt::Select(Box::new(select))),
            Err(OptimizeError::NotImplemented)
        );
    }

    #[test]
    fn optimize_select_subquery_from_not_implemented() {
        let schema = make_schema();
        let optimizer = Optimizer::new(&schema);
        let mut body = empty_simple_select();
        body.from = Some(FromClause {
            tables: vec![TableOrSubquery::Subquery {
                select: Box::new(simple_select(empty_simple_select())),
                alias: Some("s".to_string()),
            }],
            joins: vec![],
        });
        let select = Stmt::Select(Box::new(simple_select(body)));
        assert_eq!(
            optimizer.optimize_stmt(&select),
            Err(OptimizeError::NotImplemented)
        );
    }

    #[test]
    fn optimize_select_multiple_tables_not_implemented() {
        let schema = make_schema();
        let optimizer = Optimizer::new(&schema);
        let mut body = empty_simple_select();
        body.from = Some(FromClause {
            tables: vec![table_ref("users"), table_ref("users")],
            joins: vec![],
        });
        let select = Stmt::Select(Box::new(simple_select(body)));
        assert_eq!(
            optimizer.optimize_stmt(&select),
            Err(OptimizeError::NotImplemented)
        );
    }

    #[test]
    fn optimize_select_without_from_returns_empty_plan() {
        let schema = make_schema();
        let optimizer = Optimizer::new(&schema);
        let select = Stmt::Select(Box::new(simple_select(empty_simple_select())));
        let plan = optimizer.optimize_stmt(&select).unwrap();
        assert!(plan.loops.is_empty());
    }

    #[test]
    fn extract_constraints_matches_qualified_and_ignores_other_tables() {
        let schema = make_schema();
        let optimizer = Optimizer::new(&schema);

        // `users.id = 1 AND other.x = 2`: the qualified `users.id` reference
        // should be picked up on both the left and right sides, while a
        // reference qualified to a different table should be ignored.
        let matching = Expr::Binary {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Column {
                schema: None,
                table: Some("users".to_string()),
                name: "id".to_string(),
            }),
            right: Box::new(Expr::Column {
                schema: None,
                table: Some("users".to_string()),
                name: "id2".to_string(),
            }),
        };
        let mut constraints = Vec::new();
        optimizer.extract_constraints(&matching, "users", "users", &mut constraints);
        assert_eq!(constraints, vec!["id".to_string(), "id2".to_string()]);

        let other_table = Expr::Binary {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Column {
                schema: None,
                table: Some("other".to_string()),
                name: "x".to_string(),
            }),
            right: Box::new(Expr::Literal(LiteralValue::Integer(2))),
        };
        let mut constraints2 = Vec::new();
        optimizer.extract_constraints(&other_table, "users", "users", &mut constraints2);
        assert!(constraints2.is_empty());

        let other_table_right = Expr::Binary {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Literal(LiteralValue::Integer(2))),
            right: Box::new(Expr::Column {
                schema: None,
                table: Some("other".to_string()),
                name: "x".to_string(),
            }),
        };
        let mut constraints3 = Vec::new();
        optimizer.extract_constraints(&other_table_right, "users", "users", &mut constraints3);
        assert!(constraints3.is_empty());
    }

    #[test]
    fn extract_constraints_non_comparison_op_and_non_column_operands() {
        let schema = make_schema();
        let optimizer = Optimizer::new(&schema);

        // `1 AND id = 5`: the top-level op (And) is not a comparison, so the
        // constraint-matching block is skipped, but recursion still descends
        // into the right-hand `id = 5`, where the left operand of *that*
        // comparison is a non-Column literal.
        let expr = Expr::Binary {
            op: BinaryOp::And,
            left: Box::new(Expr::Literal(LiteralValue::Integer(1))),
            right: Box::new(Expr::Binary {
                op: BinaryOp::Eq,
                left: Box::new(Expr::Literal(LiteralValue::Integer(0))),
                right: Box::new(Expr::Column {
                    schema: None,
                    table: None,
                    name: "id".to_string(),
                }),
            }),
        };
        let mut constraints = Vec::new();
        optimizer.extract_constraints(&expr, "users", "users", &mut constraints);
        assert_eq!(constraints, vec!["id".to_string()]);
    }

    #[test]
    fn test_optimize_error_debug_and_display() {
        let err = OptimizeError::NotImplemented;
        assert_eq!(format!("{err:?}"), "NotImplemented");
        assert_eq!(format!("{err}"), "not yet implemented");
    }

    #[test]
    fn test_query_plan_and_loop_debug_and_clone() {
        let qloop = QueryLoop {
            table: "users".to_string(),
            scan_kind: ScanKind::RowIdLookup,
            estimated_rows: 1.0,
        };
        let plan = QueryPlan {
            loops: vec![qloop.clone()],
        };
        let plan_clone = plan.clone();
        assert_eq!(format!("{plan:?}"), format!("{plan_clone:?}"));
        assert_eq!(format!("{qloop:?}"), format!("{:?}", plan.loops[0]));
    }

    #[test]
    fn test_extract_constraints_all_comparison_operators() {
        let schema = make_schema();
        let optimizer = Optimizer::new(&schema);

        for op in [
            BinaryOp::Eq,
            BinaryOp::Gt,
            BinaryOp::Ge,
            BinaryOp::Lt,
            BinaryOp::Le,
        ] {
            let expr = Expr::Binary {
                op,
                left: Box::new(Expr::Column {
                    schema: None,
                    table: None,
                    name: "c1".to_string(),
                }),
                right: Box::new(Expr::Column {
                    schema: None,
                    table: None,
                    name: "c2".to_string(),
                }),
            };
            let mut constraints = Vec::new();
            optimizer.extract_constraints(&expr, "users", "users", &mut constraints);
            assert_eq!(constraints, vec!["c1".to_string(), "c2".to_string()]);
        }
    }

    #[test]
    fn test_optimize_rowid_constraint() {
        let schema = make_schema();
        let optimizer = Optimizer::new(&schema);
        let expr = Expr::Binary {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Column {
                schema: None,
                table: None,
                name: "rowid".to_string(),
            }),
            right: Box::new(Expr::Literal(LiteralValue::Integer(1))),
        };
        let mut body = empty_simple_select();
        body.from = Some(FromClause {
            tables: vec![table_ref("users")],
            joins: vec![],
        });
        body.where_ = Some(expr);
        let select = Stmt::Select(Box::new(simple_select(body)));
        let plan = optimizer.optimize_stmt(&select).unwrap();
        assert_eq!(plan.loops[0].scan_kind, ScanKind::RowIdLookup);
        assert_eq!(plan.loops[0].estimated_rows, 1.0);
    }

    #[test]
    fn test_optimize_table_with_alias_and_index_and_full_scan() {
        let schema = make_schema();
        schema.insert(SchemaObject {
            kind: ObjectKind::Index,
            name: "idx_users_email".to_string(),
            tbl_name: "users".to_string(),
            root_page: 3,
            sql: None,
            columns: vec![],
        });
        let optimizer = Optimizer::new(&schema);

        // Test with table alias and index scan
        let mut body = empty_simple_select();
        body.from = Some(FromClause {
            tables: vec![TableOrSubquery::Table {
                schema: None,
                name: "users".to_string(),
                alias: Some("u".to_string()),
                indexed: IndexedKind::None,
            }],
            joins: vec![],
        });
        body.where_ = Some(Expr::Binary {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Column {
                schema: None,
                table: Some("u".to_string()),
                name: "email".to_string(),
            }),
            right: Box::new(Expr::Literal(LiteralValue::Text("a@b.com".to_string()))),
        });
        let select = Stmt::Select(Box::new(simple_select(body)));
        let plan = optimizer.optimize_stmt(&select).unwrap();
        assert_eq!(plan.loops[0].table, "u");
        assert_eq!(
            plan.loops[0].scan_kind,
            ScanKind::IndexScan {
                index: "idx_users_email".to_string(),
                constraints: vec!["email".to_string()],
            }
        );
        assert_eq!(plan.loops[0].estimated_rows, 10.0);

        // Test full scan when there are no constraints matching indexes or rowid
        let mut body_full = empty_simple_select();
        body_full.from = Some(FromClause {
            tables: vec![table_ref("users")],
            joins: vec![],
        });
        let select_full = Stmt::Select(Box::new(simple_select(body_full)));
        let plan_full = optimizer.optimize_stmt(&select_full).unwrap();
        assert_eq!(plan_full.loops[0].table, "users");
        assert_eq!(plan_full.loops[0].scan_kind, ScanKind::FullScan);
        assert_eq!(plan_full.loops[0].estimated_rows, 1000.0);
    }
}
