//! Query planner for SQLite3-rs.
//!
//! Mirrors `where.c` / `whereInt.h`. Analyses WHERE clauses, selects indexes,
//! and produces a query plan consumed by the code generator.
//!
//! ## Status
//! Phase 3 — stub.

#[derive(Debug, thiserror::Error)]
pub enum OptimizeError {
    #[error("not yet implemented")]
    NotImplemented,
}

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
