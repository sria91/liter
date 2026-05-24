//! SQLite3-rs — unified Rust API.
//!
//! The public interface mirrors the ergonomics of the C `sqlite3.h` API while
//! being idiomatic Rust: ownership-based, `Result`-returning, and panic-free.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use sqlite3::{Connection, Value};
//!
//! let conn = Connection::open(":memory:").unwrap();
//! conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)", []).unwrap();
//! conn.execute("INSERT INTO t VALUES (1, 'Alice')", []).unwrap();
//!
//! let rows = conn.query("SELECT id, name FROM t", []).unwrap();
//! for row in rows {
//!     println!("{:?}", row);
//! }
//! ```
//!
//! ## Status
//! Phase 0/1 scaffold — `Connection::open` and `Connection::open_in_memory` are
//! available; all query methods return `Err(SqliteError::NotImplemented)` until
//! the VDBE and storage layers are wired together.

use std::path::Path;

pub use sqlite3_record::Value;

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
use wasm_bindgen::prelude::*;

#[cfg_attr(all(feature = "wasm", target_arch = "wasm32"), wasm_bindgen)]
pub struct WasmConnection {
    // We keep a standard Connection inside to provide basic API bindings
    #[allow(dead_code)]
    conn: Connection,
}

#[cfg_attr(all(feature = "wasm", target_arch = "wasm32"), wasm_bindgen)]
impl WasmConnection {
    #[cfg_attr(all(feature = "wasm", target_arch = "wasm32"), wasm_bindgen(constructor))]
    pub fn new() -> Result<WasmConnection, String> {
        match Connection::open_in_memory() {
            Ok(conn) => Ok(Self { conn }),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Top-level error type for sqlite3-rs.
#[derive(Debug, thiserror::Error)]
pub enum SqliteError {
    #[error("SQL error: {0}")]
    Sql(String),
    #[error("out of memory")]
    NoMem,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database is corrupt")]
    Corrupt,
    #[error("database is busy")]
    Busy,
    #[error("constraint violation: {0}")]
    Constraint(String),
    #[error("authorization denied")]
    Auth,
    #[error("not yet implemented")]
    NotImplemented,
    #[error("parse error: {0}")]
    Parse(String),
}

impl From<sqlite3_parser::ParseError> for SqliteError {
    fn from(e: sqlite3_parser::ParseError) -> Self {
        SqliteError::Parse(e.to_string())
    }
}

impl From<sqlite3_codegen::CodegenError> for SqliteError {
    fn from(e: sqlite3_codegen::CodegenError) -> Self {
        SqliteError::Sql(format!("codegen error: {}", e))
    }
}

impl From<sqlite3_vdbe::VdbeError> for SqliteError {
    fn from(e: sqlite3_vdbe::VdbeError) -> Self {
        SqliteError::Sql(format!("execution error: {}", e))
    }
}

pub type SqliteResult<T> = Result<T, SqliteError>;

/// An open database connection.
///
/// `Connection` is `Send` but not `Sync` (mirrors C SQLite `SQLITE_THREADSAFE=1`).
pub struct Connection {
    path: String,
    // pager, schema, prepared_stmts, ... — wired in Phase 2+
}

impl Connection {
    /// Open a database at the given path.
    /// Use `":memory:"` for an in-memory database.
    pub fn open(path: &str) -> SqliteResult<Self> {
        Ok(Self { path: path.to_owned() })
    }

    /// Open an in-memory database.
    pub fn open_in_memory() -> SqliteResult<Self> {
        Self::open(":memory:")
    }

    /// Execute a SQL statement, discarding any result rows.
    pub fn execute(&self, sql: &str, params: impl IntoParams) -> SqliteResult<u64> {
        // Run query and just discard the rows, returning 0 for now.
        // In a full implementation, this would return the number of rows modified.
        let _ = self.query(sql, params)?;
        Ok(0)
    }

    /// Execute a SQL query and return all result rows.
    pub fn query(&self, sql: &str, _params: impl IntoParams) -> SqliteResult<Vec<Vec<Value>>> {
        let ast = sqlite3_parser::parse_stmt(sql)?;
        
        let mut results = Vec::new();

        // Right now our AST root is a single Stmt. In the future it will be a list.
        let mut vm = sqlite3_codegen::compile(&ast)?;

        loop {
            match vm.step()? {
                sqlite3_vdbe::StepResult::Row => {
                    if let Some(row) = vm.current_result_row() {
                        let mut out_row = Vec::with_capacity(row.len());
                        for mem in row {
                            out_row.push(mem_to_value(mem));
                        }
                        results.push(out_row);
                    }
                }
                sqlite3_vdbe::StepResult::Done => break,
            }
        }

        Ok(results)
    }

    /// Execute a SQL query expected to return a single value.
    pub fn query_one<T: FromValue>(&self, sql: &str, params: impl IntoParams) -> SqliteResult<T> {
        let rows = self.query(sql, params)?;
        let row = rows.into_iter().next().ok_or_else(|| SqliteError::Sql("no rows returned".into()))?;
        let val = row.into_iter().next().ok_or_else(|| SqliteError::Sql("no columns returned".into()))?;
        T::from_value(val)
    }

    pub fn query_row(&self, sql: &str, _params: impl IntoParams) -> SqliteResult<Vec<Value>> {
        let mut rows = self.query(sql, _params)?;
        if rows.is_empty() {
            Err(SqliteError::Sql("Query returned no rows".into()))
        } else {
            Ok(rows.remove(0))
        }
    }

    /// Prepare a SQL statement for repeated execution.
    pub fn prepare(&self, sql: &str) -> SqliteResult<Statement<'_>> {
        let _ = sql;
        Err(SqliteError::NotImplemented)
    }

    /// Path this connection was opened with.
    pub fn path(&self) -> &str {
        &self.path
    }
}

fn mem_to_value(mem: &sqlite3_vdbe::Mem) -> Value {
    match mem {
        sqlite3_vdbe::Mem::Null => Value::Null,
        sqlite3_vdbe::Mem::Int(i) => Value::Int(*i),
        sqlite3_vdbe::Mem::Real(f) => Value::Real(*f),
        sqlite3_vdbe::Mem::Text(t) => Value::Text(t.as_bytes().to_vec()),
        sqlite3_vdbe::Mem::Blob(b) => Value::Blob(b.to_vec()),
        sqlite3_vdbe::Mem::ZeroBlob(n) => Value::Blob(vec![0; *n as usize]),
    }
}

/// A prepared SQL statement.
pub struct Statement<'conn> {
    _conn: &'conn Connection,
}

impl Statement<'_> {
    pub fn step(&mut self) -> SqliteResult<StepResult> {
        Err(SqliteError::NotImplemented)
    }

    pub fn reset(&mut self) -> SqliteResult<()> {
        Err(SqliteError::NotImplemented)
    }

    pub fn column_value(&self, _col: usize) -> SqliteResult<Value> {
        Err(SqliteError::NotImplemented)
    }
}

/// Result of a single `Statement::step()` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    Row,
    Done,
}

/// Trait for types that can be converted from a `Value`.
pub trait FromValue: Sized {
    fn from_value(v: Value) -> SqliteResult<Self>;
}

impl FromValue for i64 {
    fn from_value(v: Value) -> SqliteResult<Self> {
        match v {
            Value::Int(i) => Ok(i),
            Value::Real(f) => Ok(f as i64),
            _ => Err(SqliteError::Sql(format!("expected integer, got {v:?}"))),
        }
    }
}

impl FromValue for f64 {
    fn from_value(v: Value) -> SqliteResult<Self> {
        match v {
            Value::Real(f) => Ok(f),
            Value::Int(i) => Ok(i as f64),
            _ => Err(SqliteError::Sql(format!("expected real, got {v:?}"))),
        }
    }
}

impl FromValue for String {
    fn from_value(v: Value) -> SqliteResult<Self> {
        match v {
            Value::Text(b) => String::from_utf8(b).map_err(|e| SqliteError::Sql(e.to_string())),
            _ => Err(SqliteError::Sql(format!("expected text, got {v:?}"))),
        }
    }
}

/// Sealed trait for parameter lists.
pub trait IntoParams: private::Sealed {}

mod private {
    pub trait Sealed {}
    impl Sealed for [(); 0] {}
    impl<T: Into<super::Value>> Sealed for &[T] {}
    impl<T: Into<super::Value>> Sealed for Vec<T> {}
}

impl IntoParams for [(); 0] {}
impl<T: Into<Value>> IntoParams for &[T] {}
impl<T: Into<Value>> IntoParams for Vec<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execute_simple_select() {
        let conn = Connection::open_in_memory().unwrap();
        let rows = conn.query("SELECT 1 + 1;", [] as [(); 0]).unwrap();
        
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 1);
        assert_eq!(rows[0][0], Value::Int(2));
    }

    #[test]
    fn test_execute_multiple_columns() {
        let conn = Connection::open_in_memory().unwrap();
        let rows = conn.query("SELECT 42, 'hello', 3.14;", [] as [(); 0]).unwrap();
        
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 3);
        assert_eq!(rows[0][0], Value::Int(42));
        assert_eq!(rows[0][1], Value::Text(b"hello".to_vec()));
        assert_eq!(rows[0][2], Value::Real(3.14));
    }

    #[test]
    fn open_in_memory() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(conn.path(), ":memory:");
    }
}
