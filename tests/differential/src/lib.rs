//! Differential testing harness.
//!
//! Runs the same SQL against both C SQLite (via `rusqlite`) and our Rust
//! implementation simultaneously, then diffs the results.

use liter::{Connection as RConn, Value};

/// Run a SQL statement against both implementations and assert equal results.
///
/// `setup_sql` can be multiple statements executed one by one to set up tables/data.
/// `sql` is the final query whose results are diffed.
///
/// # Panics
/// Panics if the results differ or if the C SQLite call fails.
#[cfg(feature = "differential")]
pub fn diff_exec(setup_sql: &[&str], sql: &str) {
    use rusqlite::Connection as CConn;

    let c_conn = CConn::open_in_memory().expect("C sqlite open failed");
    let r_conn = RConn::open_in_memory().expect("Rust sqlite open failed");

    for setup in setup_sql {
        c_conn.execute(setup, []).expect("C setup failed");
        r_conn
            .execute(setup, [] as [(); 0])
            .expect("Rust setup failed");
    }

    // Execute against C SQLite.
    let mut c_rows: Vec<Vec<String>> = Vec::new();
    {
        let mut stmt = c_conn.prepare(sql).expect("C prepare failed");
        let col_count = stmt.column_count();
        let mut rows_iter = stmt.query([]).expect("C query failed");
        while let Some(row) = rows_iter.next().expect("C row error") {
            let cells: Vec<String> = (0..col_count)
                .map(|i| match row.get::<_, rusqlite::types::Value>(i) {
                    Ok(rusqlite::types::Value::Null) => "Null".into(),
                    Ok(rusqlite::types::Value::Integer(v)) => format!("Int({v})"),
                    Ok(rusqlite::types::Value::Real(v)) => format!("Real({v})"),
                    Ok(rusqlite::types::Value::Text(v)) => format!("Text({:?})", v),
                    Ok(rusqlite::types::Value::Blob(v)) => format!("Blob({:?})", v),
                    Err(_) => "ERROR".into(),
                })
                .collect();
            c_rows.push(cells);
        }
    }

    let r_rows_raw = match r_conn.query(sql, [] as [(); 0]) {
        Ok(rows) => rows,
        Err(e) => panic!("Rust sqlite error for SQL:\n{sql}\nError: {e}"),
    };

    let r_rows: Vec<Vec<String>> = r_rows_raw
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|v| match v {
                    Value::Null => "Null".into(),
                    Value::Int(i) => format!("Int({i})"),
                    Value::Real(f) => format!("Real({f})"),
                    Value::Text(t) => {
                        let s = String::from_utf8_lossy(&t);
                        format!("Text({:?})", s.to_string())
                    }
                    Value::Blob(b) => format!("Blob({:?})", b),
                    Value::ZeroBlob(n) => format!("Blob({:?})", vec![0u8; n as usize]),
                })
                .collect()
        })
        .collect();

    assert_eq!(c_rows, r_rows, "Differential failure for SQL:\n{sql}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_select_literal() {
        diff_exec(&[], "SELECT 1, 2 + 3, 'hello', NULL;");
    }

    #[test]
    fn test_diff_select_math() {
        diff_exec(&[], "SELECT 10 * 5, 20 / 4, 15 % 4;");
    }

    #[test]
    fn test_diff_where_logic() {
        diff_exec(&[], "SELECT 1 WHERE 1 = 1;");
        diff_exec(&[], "SELECT 1 WHERE 1 = 0;");
        diff_exec(&[], "SELECT 'yes' WHERE 5 > 3 AND 2 < 4;");
    }

    #[test]
    fn test_diff_create_insert_select() {
        diff_exec(
            &[
                "CREATE TABLE t(a, b);",
                "INSERT INTO t VALUES (1, 'hello');",
                "INSERT INTO t VALUES (2, 'world');",
            ],
            "SELECT * FROM t;",
        );
    }

    #[test]
    fn test_diff_group_by() {
        diff_exec(
            &[
                "CREATE TABLE t(a, b);",
                "INSERT INTO t VALUES (1, 10);",
                "INSERT INTO t VALUES (1, 20);",
                "INSERT INTO t VALUES (2, 50);",
            ],
            "SELECT a, SUM(b) FROM t GROUP BY a;",
        );
    }

    #[test]
    fn test_format_compatibility() {
        let db_path = "test_format_compat.db";
        // Clean up any old file
        let _ = std::fs::remove_file(db_path);

        // 1. Create a DB file using C SQLite
        {
            let c_conn = rusqlite::Connection::open(db_path).expect("C open failed");
            c_conn
                .execute(
                    "CREATE TABLE users(id INTEGER PRIMARY KEY, name TEXT, score REAL)",
                    [],
                )
                .unwrap();
            c_conn
                .execute("INSERT INTO users VALUES (1, 'Alice', 95.5)", [])
                .unwrap();
            c_conn
                .execute("INSERT INTO users VALUES (2, 'Bob', 80.0)", [])
                .unwrap();
        }

        // 2. Open it with Rust SQLite and verify we can read it
        {
            let r_conn = RConn::open(db_path).expect("Rust open failed");
            let rows = r_conn
                .query("SELECT id, name, score FROM users", [] as [(); 0])
                .unwrap();
            assert_eq!(rows.len(), 2);

            // Just some quick validation of the data we extracted
            if let Value::Text(name1) = &rows[0][1] {
                assert_eq!(name1, b"Alice");
            } else {
                panic!("Expected text for Alice");
            }
            if let Value::Text(name2) = &rows[1][1] {
                assert_eq!(name2, b"Bob");
            } else {
                panic!("Expected text for Bob");
            }
        }

        // Clean up
        let _ = std::fs::remove_file(db_path);
    }
}
