//! Query planner for SQLite3-rs.
//!
//! Mirrors `where.c` / `whereInt.h`. Analyses WHERE clauses, selects indexes,
//! and produces a query plan consumed by the code generator.

use sqlite3_ast::*;
use sqlite3_schema::Schema;

#[derive(Debug, thiserror::Error)]
pub enum OptimizeError {
    #[error("not yet implemented")]
    NotImplemented,
}

pub type OptimizeResult<T> = Result<T, OptimizeError>;

/// A single loop in the query plan (one scan or index lookup per table).
#[derive(Debug, Clone)]
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
    IndexScan { index: String, constraints: Vec<String> },
    /// Row-id lookup (single row).
    RowIdLookup,
}

/// The output of the query planner for a SELECT.
#[derive(Debug, Clone)]
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
        let table_name = if let Some(from) = &body.from {
            if from.tables.len() == 1 {
                match &from.tables[0] {
                    TableOrSubquery::Table { alias, name, .. } => {
                        alias.clone().unwrap_or_else(|| name.clone())
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
            self.extract_constraints(where_expr, &table_name, &mut constraints);
        }

        // Fetch schema indexes
        let indexes = self.schema.indexes_for(&table_name);

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
                table: table_name,
                scan_kind,
                estimated_rows,
            }],
        })
    }

    fn extract_constraints(&self, expr: &Expr, target_table: &str, constraints: &mut Vec<String>) {
        if let Expr::Binary { op, left, right } = expr {
            if matches!(op, BinaryOp::Eq | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Lt | BinaryOp::Le) {
                if let Expr::Column { table, name, .. } = &**left {
                    if table.is_none() || table.as_deref() == Some(target_table) {
                        constraints.push(name.clone());
                    }
                }
                if let Expr::Column { table, name, .. } = &**right {
                    if table.is_none() || table.as_deref() == Some(target_table) {
                        constraints.push(name.clone());
                    }
                }
            }
            self.extract_constraints(left, target_table, constraints);
            self.extract_constraints(right, target_table, constraints);
        }
    }
}
