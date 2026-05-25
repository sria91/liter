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
//! Phase 9 — `Connection::prepare` and `Statement` are fully implemented.
//! `SELECT … FROM table [WHERE …]`, `CREATE TABLE`, and `INSERT INTO` all work end-to-end.

use std::cell::Cell;
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
    /// True when the user has issued an explicit `BEGIN` and we must not
    /// auto-commit individual DML statements.
    in_txn: Cell<bool>,
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
            in_txn: Cell::new(false),
        })
    }

    /// Open an in-memory database.
    pub fn open_in_memory() -> SqliteResult<Self> {
        Self::open(":memory:")
    }

    /// Execute a SQL statement, discarding any result rows.
    pub fn execute(&self, sql: &str, _params: impl IntoParams) -> SqliteResult<u64> {
        let ast = sqlite3_parser::parse_stmt(sql)?;

        // ── Transaction control (no bytecode needed) ──────────────────────────
        match &ast {
            sqlite3_ast::Stmt::Begin(_) => {
                self.btree.begin_write()?;
                self.in_txn.set(true);
                return Ok(0);
            }
            sqlite3_ast::Stmt::Commit => {
                self.btree.commit()?;
                self.in_txn.set(false);
                return Ok(0);
            }
            sqlite3_ast::Stmt::Rollback { .. } => {
                self.btree.rollback()?;
                self.in_txn.set(false);
                return Ok(0);
            }
            // Savepoint/Release: forward to btree if supported, else no-op for now.
            sqlite3_ast::Stmt::Savepoint(_) | sqlite3_ast::Stmt::Release(_) => {
                return Ok(0);
            }
            _ => {}
        }

        let is_create = matches!(&ast, sqlite3_ast::Stmt::Create(_));
        let mut vm = sqlite3_codegen::compile_with_schema(&ast, &self.schema)?;
        vm.func_dispatcher = Some(|name, args| {
            sqlite3_functions::dispatch_function(name, args).map_err(|e| e.to_string())
        });

        // Only auto-begin/commit when NOT inside a user transaction.
        let auto_txn = !self.in_txn.get();
        if auto_txn {
            self.btree.begin_write()?;
        }
        
        let mut cursors: Vec<Option<sqlite3_vdbe::VdbeCursor>> = Vec::with_capacity(vm.n_cursors);
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
            if auto_txn { let _ = self.btree.rollback(); }
            return Err(res.unwrap_err());
        }

        if auto_txn { self.btree.commit()?; }

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
        vm.func_dispatcher = Some(|name, args| {
            sqlite3_functions::dispatch_function(name, args).map_err(|e| e.to_string())
        });

        let mut cursors: Vec<Option<sqlite3_vdbe::VdbeCursor>> = Vec::with_capacity(vm.n_cursors);
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
    ///
    /// The returned `Statement` borrows from this connection. Call
    /// `stmt.step()` to drive execution row by row.
    pub fn prepare<'c>(&'c self, sql: &str) -> SqliteResult<Statement<'c>> {
        let ast = sqlite3_parser::parse_stmt(sql)?;
        let vm = sqlite3_codegen::compile_with_schema(&ast, &self.schema)?;
        let n = vm.n_cursors;
        Ok(Statement {
            conn: self,
            vm,
            cursors: (0..n).map(|_| None).collect(),
            column_cache: Vec::new(),
            done: false,
        })
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

/// A prepared SQL statement tied to its originating `Connection`.
///
/// Call `step()` repeatedly: it returns `Ok(StepResult::Row)` for each
/// result row (retrieve values via `column_value`) and `Ok(StepResult::Done)`
/// when execution is complete. Call `reset()` to rewind for re-execution.
pub struct Statement<'conn> {
    conn: &'conn Connection,
    vm: sqlite3_vdbe::Vdbe,
    /// One slot per cursor allocated by the compiled program.
    /// Cursor lifetime is tied to `conn.btree` via `'conn`.
    cursors: Vec<Option<sqlite3_vdbe::VdbeCursor<'conn>>>,
    /// Snapshot of the last yielded row (avoids borrow on vm after step).
    column_cache: Vec<Value>,
    done: bool,
}

impl Statement<'_> {
    /// Drive the VM one step.
    ///
    /// Returns `Ok(StepResult::Row)` when a row is available and
    /// `Ok(StepResult::Done)` when all rows have been produced.
    pub fn step(&mut self) -> SqliteResult<StepResult> {
        if self.done {
            return Ok(StepResult::Done);
        }
        // SAFETY-note: we transmute the cursor lifetime from 'conn to 'static
        // here only conceptually — the cursors actually reference conn.btree
        // which lives at least as long as Statement<'conn>. We express this
        // through the 'conn lifetime on Statement. The transmute below is the
        // standard workaround for the self-referential pattern until Rust gains
        // async-fn-in-trait with captured borrows.
        //
        // ALTERNATIVE (chosen): Pass `&self.conn.btree` and let Rust verify the
        // lifetime matches the cursor slice's element lifetime 'conn. This works
        // because both Statement and cursors carry the same 'conn lifetime.
        let result = self.vm.step(
            &self.conn.btree,
            // Cast the slice from &mut [Option<BTreeCursor<'conn>>] to
            // &mut [Option<BTreeCursor<'_>>] — Rust coerces this through
            // lifetime subtyping because 'conn outlives the borrow.
            &mut self.cursors,
        )?;

        match result {
            sqlite3_vdbe::StepResult::Row => {
                // Cache the current row so column_value() can return values
                // without holding a borrow on `self.vm` simultaneously.
                self.column_cache = if let Some(row) = self.vm.current_result_row() {
                    row.iter().map(mem_to_value).collect()
                } else {
                    Vec::new()
                };
                Ok(StepResult::Row)
            }
            sqlite3_vdbe::StepResult::Done => {
                self.done = true;
                self.column_cache.clear();
                Ok(StepResult::Done)
            }
        }
    }

    /// Reset the statement so it can be re-executed from the beginning.
    pub fn reset(&mut self) -> SqliteResult<()> {
        self.vm.reset()?;
        for slot in &mut self.cursors {
            *slot = None;
        }
        self.column_cache.clear();
        self.done = false;
        Ok(())
    }

    /// Return the value of column `col` (0-indexed) from the last `Row` result.
    pub fn column_value(&self, col: usize) -> SqliteResult<Value> {
        self.column_cache.get(col).cloned()
            .ok_or_else(|| SqliteError::Sql(format!("column index {} out of range", col)))
    }

    /// Return the number of result columns in this statement.
    pub fn column_count(&self) -> usize {
        self.column_cache.len()
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

    #[test]
    fn test_select_from_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE users (id INTEGER, name TEXT)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO users VALUES (1, 'alice')", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO users VALUES (2, 'bob')", [] as [(); 0]).unwrap();

        let rows = conn.query("SELECT id, name FROM users", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 2, "expected 2 rows, got {}", rows.len());
        assert_eq!(rows[0][0], Value::Int(1));
        assert_eq!(rows[0][1], Value::Text(b"alice".to_vec()));
        assert_eq!(rows[1][0], Value::Int(2));
        assert_eq!(rows[1][1], Value::Text(b"bob".to_vec()));
    }

    #[test]
    fn test_select_star_from_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE items (x INTEGER, y INTEGER)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO items VALUES (10, 20)", [] as [(); 0]).unwrap();

        let rows = conn.query("SELECT * FROM items", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], Value::Int(10));
        assert_eq!(rows[0][1], Value::Int(20));
    }

    #[test]
    fn test_select_from_where() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (id INTEGER, val TEXT)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO t VALUES (1, 'a')", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO t VALUES (2, 'b')", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO t VALUES (3, 'c')", [] as [(); 0]).unwrap();

        // WHERE with equality — should return only id=2
        let rows = conn.query("SELECT id, val FROM t WHERE id = 2", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 1, "expected 1 matching row, got {}", rows.len());
        assert_eq!(rows[0][0], Value::Int(2));
        assert_eq!(rows[0][1], Value::Text(b"b".to_vec()));
    }

    #[test]
    fn test_select_from_empty_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE empty (id INTEGER)", [] as [(); 0]).unwrap();
        let rows = conn.query("SELECT id FROM empty", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 0, "expected 0 rows from empty table");
    }

    // ── Phase 9: Statement API ────────────────────────────────────────────────

    #[test]
    fn test_prepare_step_column_value() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE p (x INTEGER, y TEXT)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO p VALUES (1, 'one')", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO p VALUES (2, 'two')", [] as [(); 0]).unwrap();

        let mut stmt = conn.prepare("SELECT x, y FROM p").unwrap();

        // First row
        assert_eq!(stmt.step().unwrap(), StepResult::Row);
        assert_eq!(stmt.column_count(), 2);
        assert_eq!(stmt.column_value(0).unwrap(), Value::Int(1));
        assert_eq!(stmt.column_value(1).unwrap(), Value::Text(b"one".to_vec()));

        // Second row
        assert_eq!(stmt.step().unwrap(), StepResult::Row);
        assert_eq!(stmt.column_value(0).unwrap(), Value::Int(2));
        assert_eq!(stmt.column_value(1).unwrap(), Value::Text(b"two".to_vec()));

        // Done
        assert_eq!(stmt.step().unwrap(), StepResult::Done);
        // Calling step again after Done returns Done
        assert_eq!(stmt.step().unwrap(), StepResult::Done);
    }

    #[test]
    fn test_statement_reset() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE r (n INTEGER)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO r VALUES (42)", [] as [(); 0]).unwrap();

        let mut stmt = conn.prepare("SELECT n FROM r").unwrap();

        // First run
        assert_eq!(stmt.step().unwrap(), StepResult::Row);
        assert_eq!(stmt.column_value(0).unwrap(), Value::Int(42));
        assert_eq!(stmt.step().unwrap(), StepResult::Done);

        // Reset and re-run
        stmt.reset().unwrap();
        assert_eq!(stmt.step().unwrap(), StepResult::Row);
        assert_eq!(stmt.column_value(0).unwrap(), Value::Int(42));
        assert_eq!(stmt.step().unwrap(), StepResult::Done);
    }

    #[test]
    fn test_prepare_literal_select() {
        let conn = Connection::open_in_memory().unwrap();
        let mut stmt = conn.prepare("SELECT 100, 'hello'").unwrap();

        assert_eq!(stmt.step().unwrap(), StepResult::Row);
        assert_eq!(stmt.column_value(0).unwrap(), Value::Int(100));
        assert_eq!(stmt.column_value(1).unwrap(), Value::Text(b"hello".to_vec()));
        assert_eq!(stmt.step().unwrap(), StepResult::Done);
    }

    // ── Phase 10: DELETE, UPDATE, AND/OR, Transactions ────────────────────────

    #[test]
    fn test_delete_all() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE d (x INTEGER)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO d VALUES (1)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO d VALUES (2)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO d VALUES (3)", [] as [(); 0]).unwrap();

        conn.execute("DELETE FROM d", [] as [(); 0]).unwrap();

        let rows = conn.query("SELECT x FROM d", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 0, "all rows should be deleted");
    }

    #[test]
    fn test_delete_where() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE d2 (id INTEGER, val TEXT)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO d2 VALUES (1, 'keep')", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO d2 VALUES (2, 'drop')", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO d2 VALUES (3, 'keep')", [] as [(); 0]).unwrap();

        conn.execute("DELETE FROM d2 WHERE id = 2", [] as [(); 0]).unwrap();

        let rows = conn.query("SELECT id FROM d2", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 2, "only 1 row should be deleted");
        assert_eq!(rows[0][0], Value::Int(1));
        assert_eq!(rows[1][0], Value::Int(3));
    }

    #[test]
    fn test_update_all() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE u (id INTEGER, score INTEGER)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO u VALUES (1, 10)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO u VALUES (2, 20)", [] as [(); 0]).unwrap();

        conn.execute("UPDATE u SET score = 99", [] as [(); 0]).unwrap();

        let rows = conn.query("SELECT score FROM u", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], Value::Int(99));
        assert_eq!(rows[1][0], Value::Int(99));
    }

    #[test]
    fn test_update_where() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE u2 (id INTEGER, val TEXT)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO u2 VALUES (1, 'old')", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO u2 VALUES (2, 'old')", [] as [(); 0]).unwrap();

        conn.execute("UPDATE u2 SET val = 'new' WHERE id = 1", [] as [(); 0]).unwrap();

        let rows = conn.query("SELECT id, val FROM u2", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][1], Value::Text(b"new".to_vec()));
        assert_eq!(rows[1][1], Value::Text(b"old".to_vec()));
    }

    #[test]
    fn test_where_and() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE w (a INTEGER, b INTEGER)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO w VALUES (1, 10)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO w VALUES (2, 20)", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO w VALUES (3, 30)", [] as [(); 0]).unwrap();

        // a > 1 AND b < 30 should match only row (2, 20)
        let rows = conn.query("SELECT a FROM w WHERE a > 1 AND b < 30", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], Value::Int(2));
    }

    #[test]
    fn test_explicit_transaction_commit() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE tx (n INTEGER)", [] as [(); 0]).unwrap();

        conn.execute("BEGIN", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO tx VALUES (42)", [] as [(); 0]).unwrap();
        conn.execute("COMMIT", [] as [(); 0]).unwrap();

        let rows = conn.query("SELECT n FROM tx", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], Value::Int(42));
    }

    #[test]
    fn test_explicit_transaction_rollback() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE tx2 (n INTEGER)", [] as [(); 0]).unwrap();

        conn.execute("BEGIN", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO tx2 VALUES (99)", [] as [(); 0]).unwrap();
        conn.execute("ROLLBACK", [] as [(); 0]).unwrap();

        let rows = conn.query("SELECT n FROM tx2", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 0, "rollback should undo the insert");
    }
}
