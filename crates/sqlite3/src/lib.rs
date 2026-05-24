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

impl From<sqlite3_btree::BTreeError> for SqliteError {
    fn from(e: sqlite3_btree::BTreeError) -> Self {
        SqliteError::Sql(format!("btree error: {}", e))
    }
}

pub type SqliteResult<T> = Result<T, SqliteError>;

/// An open database connection.
///
/// `Connection` is `Send` but not `Sync` (mirrors C SQLite `SQLITE_THREADSAFE=1`).
pub struct Connection {
    path: String,
    pub btree: sqlite3_btree::BTree,
    pub schema: sqlite3_schema::Schema,
}

impl Connection {
    /// Open a database at the given path.
    /// Use `":memory:"` for an in-memory database.
    pub fn open(path: &str) -> SqliteResult<Self> {
        let btree = if path == ":memory:" {
            sqlite3_btree::BTree::new_in_memory()
        } else {
            sqlite3_btree::BTree::open(Path::new(path), false)?
        };
        Ok(Self { 
            path: path.to_owned(),
            btree,
            schema: sqlite3_schema::Schema::new(),
        })
    }

    /// Open an in-memory database.
    pub fn open_in_memory() -> SqliteResult<Self> {
        Self::open(":memory:")
    }

    /// Execute a SQL statement, discarding any result rows.
    pub fn execute(&self, sql: &str, params: impl IntoParams) -> SqliteResult<u64> {
        let ast = sqlite3_parser::parse_stmt(sql)?;
        let is_create = matches!(&ast, sqlite3_ast::Stmt::Create(_));
        let mut vm = sqlite3_codegen::compile_with_schema(&ast, &self.schema)?;

        self.btree.begin_write()?;
        
        let mut cursors: Vec<Option<sqlite3_btree::BTreeCursor>> = Vec::with_capacity(vm.n_cursors);
        for _ in 0..vm.n_cursors { cursors.push(None); }

        let mut root_page = None;

        let res = (|| -> SqliteResult<()> {
            loop {
                match vm.step(&self.btree, &mut cursors)? {
                    sqlite3_vdbe::StepResult::Row => {
                        if is_create {
                            if let Some(row) = vm.current_result_row() {
                                if let Some(pgno) = row[0].to_int() {
                                    root_page = Some(pgno as u32);
                                }
                            }
                        }
                    }
                    sqlite3_vdbe::StepResult::Done => break,
                }
            }
            Ok(())
        })();

        if res.is_err() {
            let _ = self.btree.rollback();
            return Err(res.unwrap_err());
        }

        self.btree.commit()?;

        // If it was a CREATE TABLE statement, insert into the schema catalog.
        if is_create {
            if let Some(rp) = root_page {
                if let sqlite3_ast::Stmt::Create(create_stmt) = ast {
                    if let sqlite3_ast::CreateStmt::Table(create_table) = *create_stmt {
                        let columns = match create_table.body {
                            sqlite3_ast::CreateTableBody::Columns { columns, .. } => columns,
                            _ => Vec::new(),
                        };

                        self.schema.insert(sqlite3_schema::SchemaObject {
                            kind: sqlite3_schema::ObjectKind::Table,
                            name: create_table.name.clone(),
                            tbl_name: create_table.name.clone(),
                            root_page: rp,
                            sql: Some(sql.to_owned()),
                            columns,
                        });
                    }
                }
            }
        }

        Ok(0)
    }

    /// Execute a SQL query and return all result rows.
    pub fn query(&self, sql: &str, _params: impl IntoParams) -> SqliteResult<Vec<Vec<Value>>> {
        let ast = sqlite3_parser::parse_stmt(sql)?;
        
        let mut results = Vec::new();

        let mut vm = sqlite3_codegen::compile_with_schema(&ast, &self.schema)?;

        let mut cursors: Vec<Option<sqlite3_btree::BTreeCursor>> = Vec::with_capacity(vm.n_cursors);
        for _ in 0..vm.n_cursors { cursors.push(None); }

        loop {
            match vm.step(&self.btree, &mut cursors)? {
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
    fn test_create_table_and_insert() {
        let conn = Connection::open_in_memory().unwrap();
        
        // Create table
        let res = conn.execute("CREATE TABLE users (id INTEGER, name TEXT);", [] as [(); 0]);
        assert!(res.is_ok(), "CREATE TABLE failed: {:?}", res.err());

        // Verify it was added to schema
        let tables = conn.schema.tables();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].name, "users");
        
        // Insert a row
        let res = conn.execute("INSERT INTO users VALUES (1, 'alice');", [] as [(); 0]);
        assert!(res.is_ok(), "INSERT failed: {:?}", res.err());
        
        // Insert another row
        let res = conn.execute("INSERT INTO users VALUES (2, 'bob');", [] as [(); 0]);
        assert!(res.is_ok(), "INSERT failed: {:?}", res.err());
    }

    #[test]
    fn open_in_memory() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(conn.path(), ":memory:");
    }
}
