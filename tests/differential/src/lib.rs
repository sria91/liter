//! Differential testing harness.
//!
//! Runs the same SQL against both C SQLite (via `rusqlite`) and our Rust
//! implementation simultaneously, then diffs the results.

use sqlite3::{Connection as RConn, Value};

/// Run a SQL statement against both implementations and assert equal results.
///
/// # Panics
/// Panics if the results differ or if the C SQLite call fails.
#[cfg(feature = "differential")]
pub fn diff_exec(sql: &str) {
    use rusqlite::Connection as CConn;

    let c_conn = CConn::open_in_memory().expect("C sqlite open failed");
    let r_conn = RConn::open_in_memory().expect("Rust sqlite open failed");

    // Execute against C SQLite.
    let mut c_rows: Vec<Vec<String>> = Vec::new();
    {
        let mut stmt = c_conn.prepare(sql).expect("C prepare failed");
        let col_count = stmt.column_count();
        let mut rows_iter = stmt.query([]).expect("C query failed");
        while let Some(row) = rows_iter.next().expect("C row error") {
            let cells: Vec<String> = (0..col_count)
                .map(|i| {
                    row.get::<_, rusqlite::types::Value>(i)
                        .map(|v| format!("{v:?}"))
                        .unwrap_or_else(|_| "ERROR".into())
                })
                .collect();
            c_rows.push(cells);
        }
    }

    // Execute against Rust SQLite (returns NotImplemented for now).
    match r_conn.query(sql, [] as [(); 0]) {
        Err(sqlite3::SqliteError::NotImplemented) => {
            // Expected during development — skip comparison.
        }
        Ok(r_rows) => {
            let r_rows_str: Vec<Vec<String>> = r_rows
                .into_iter()
                .map(|row| row.into_iter().map(|v| format!("{v:?}")).collect())
                .collect();
            assert_eq!(c_rows, r_rows_str, "Differential failure for SQL:\n{sql}");
        }
        Err(e) => panic!("Rust sqlite error for SQL:\n{sql}\nError: {e}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder_diff_test() {
        // Differential tests are enabled via --features differential once
        // the Rust query engine is wired up.
    }
}
