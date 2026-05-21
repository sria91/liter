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
    pub fn execute(&self, sql: &str, _params: impl IntoParams) -> SqliteResult<u64> {
        let _ = sql;
        Err(SqliteError::NotImplemented)
    }

    /// Execute a SQL query and return all result rows.
    pub fn query(&self, sql: &str, _params: impl IntoParams) -> SqliteResult<Vec<Vec<Value>>> {
        let _ = sql;
        Err(SqliteError::NotImplemented)
    }

    /// Execute a SQL query expected to return a single value.
    pub fn query_one<T: FromValue>(&self, sql: &str, params: impl IntoParams) -> SqliteResult<T> {
        let rows = self.query(sql, params)?;
        let row = rows.into_iter().next().ok_or_else(|| SqliteError::Sql("no rows returned".into()))?;
        let val = row.into_iter().next().ok_or_else(|| SqliteError::Sql("no columns returned".into()))?;
        T::from_value(val)
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
    fn open_in_memory() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(conn.path(), ":memory:");
    }

    #[test]
    fn execute_not_yet_implemented() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(matches!(
            conn.execute("SELECT 1", [] as [(); 0]),
            Err(SqliteError::NotImplemented)
        ));
    }
}
