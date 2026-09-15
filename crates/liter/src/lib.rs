//! Liter-rs — unified Rust API.
//!
//! The public interface mirrors the ergonomics of the C `sqlite3.h` API while
//! being idiomatic Rust: ownership-based, `Result`-returning, and panic-free.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use liter::{Connection, Value};
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

use log::{debug, trace, warn};

pub use liter_record::Value;

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
    #[cfg_attr(
        all(feature = "wasm", target_arch = "wasm32"),
        wasm_bindgen(constructor)
    )]
    pub fn new() -> Result<WasmConnection, String> {
        match Connection::open_in_memory() {
            Ok(conn) => Ok(Self { conn }),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Top-level error type for liter-rs.
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

impl From<liter_parser::ParseError> for SqliteError {
    fn from(e: liter_parser::ParseError) -> Self {
        SqliteError::Parse(e.to_string())
    }
}

impl From<liter_codegen::CodegenError> for SqliteError {
    fn from(e: liter_codegen::CodegenError) -> Self {
        SqliteError::Sql(format!("codegen error: {}", e))
    }
}

impl From<liter_vdbe::VdbeError> for SqliteError {
    fn from(e: liter_vdbe::VdbeError) -> Self {
        SqliteError::Sql(format!("execution error: {}", e))
    }
}

impl From<liter_btree::BTreeError> for SqliteError {
    fn from(e: liter_btree::BTreeError) -> Self {
        SqliteError::Sql(format!("btree error: {}", e))
    }
}

pub type SqliteResult<T> = Result<T, SqliteError>;

/// An open database connection.
///
/// `Connection` is `Send` but not `Sync` (mirrors C SQLite `SQLITE_THREADSAFE=1`).
pub struct Connection {
    path: String,
    pub btree: liter_btree::BTree,
    pub schema: liter_schema::Schema,
    /// True when the user has issued an explicit `BEGIN` and we must not
    /// auto-commit individual DML statements.
    in_txn: Cell<bool>,
}

impl Connection {
    /// Open a database at the given path.
    /// Use `":memory:"` for an in-memory database.
    pub fn open(path: &str) -> SqliteResult<Self> {
        let btree = if path == ":memory:" {
            liter_btree::BTree::new_in_memory()
        } else {
            liter_btree::BTree::open(Path::new(path), false)?
        };
        let schema = liter_schema::Schema::new();

        // Try to load schema from sqlite_schema (root page 1)
        if let Ok(mut cursor) = btree.cursor(1, false) {
            if cursor.move_to_first().unwrap_or(false) {
                while cursor.is_valid() {
                    if let Ok(payload) = cursor.data() {
                        if let Ok(record) = liter_record::decode_record(payload) {
                            // sqlite_schema: (type, name, tbl_name, rootpage, sql)
                            if let (
                                Some(Value::Text(type_val)),
                                Some(Value::Int(rootpage)),
                                Some(Value::Text(sql)),
                            ) = (record.first(), record.get(3), record.get(4))
                            {
                                let type_str = String::from_utf8_lossy(type_val);
                                debug!(
                                    "Loaded schema object: type='{}', sql={:?}",
                                    type_str,
                                    String::from_utf8_lossy(sql)
                                );
                                if type_str == "table" {
                                    let sql_str = String::from_utf8_lossy(sql);
                                    match liter_parser::parse_stmt(&sql_str) {
                                        Ok(liter_ast::Stmt::Create(create_stmt)) => {
                                            if let liter_ast::CreateStmt::Table(create_table) =
                                                *create_stmt
                                            {
                                                let columns = match create_table.body {
                                                    liter_ast::CreateTableBody::Columns {
                                                        columns,
                                                        ..
                                                    } => columns,
                                                    _ => Vec::new(),
                                                };
                                                schema.insert(liter_schema::SchemaObject {
                                                    kind: liter_schema::ObjectKind::Table,
                                                    name: create_table.name.clone(),
                                                    tbl_name: create_table.name.clone(),
                                                    root_page: *rootpage as u32,
                                                    sql: Some(sql_str.into_owned()),
                                                    columns,
                                                });
                                            }
                                        }
                                        Err(e) => {
                                            warn!("Parse error for schema object: {}", e);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    let _ = cursor.next();
                }
            }
        }

        Ok(Self {
            path: path.to_owned(),
            btree,
            schema,
            in_txn: Cell::new(false),
        })
    }

    /// Open an in-memory database.
    pub fn open_in_memory() -> SqliteResult<Self> {
        Self::open(":memory:")
    }

    /// Execute a SQL statement, discarding any result rows.
    pub fn execute(&self, sql: &str, _params: impl IntoParams) -> SqliteResult<u64> {
        let ast = liter_parser::parse_stmt(sql)?;

        // ── Transaction control (no bytecode needed) ──────────────────────────
        match &ast {
            liter_ast::Stmt::Begin(_) => {
                self.btree.begin_write()?;
                self.in_txn.set(true);
                return Ok(0);
            }
            liter_ast::Stmt::Commit => {
                self.btree.commit()?;
                self.in_txn.set(false);
                return Ok(0);
            }
            liter_ast::Stmt::Rollback { .. } => {
                self.btree.rollback()?;
                self.in_txn.set(false);
                return Ok(0);
            }
            // Savepoint/Release: forward to btree if supported, else no-op for now.
            liter_ast::Stmt::Savepoint(_) | liter_ast::Stmt::Release(_) => {
                return Ok(0);
            }
            _ => {}
        }

        let is_create = matches!(&ast, liter_ast::Stmt::Create(_));
        let mut vm = liter_codegen::compile_with_schema(&ast, &self.schema)?;
        vm.func_dispatcher = Some(|name, args| {
            if name.to_ascii_lowercase().starts_with("json") || name == "->" || name == "->>" {
                liter_json::dispatch_function(name, args)
            } else {
                liter_functions::dispatch_function(name, args).map_err(|e| e.to_string())
            }
        });
        vm.agg_dispatcher = Some(liter_functions::dispatch_aggregate);

        // Only auto-begin/commit when NOT inside a user transaction.
        let auto_txn = !self.in_txn.get();
        if auto_txn {
            self.btree.begin_write()?;
        }

        let mut cursors: Vec<Option<liter_vdbe::VdbeCursor>> = Vec::with_capacity(vm.n_cursors);
        for _ in 0..vm.n_cursors {
            cursors.push(None);
        }

        let mut root_page = None;

        let res = (|| -> SqliteResult<()> {
            while let liter_vdbe::StepResult::Row = vm.step(&self.btree, &mut cursors)? {
                if is_create {
                    if let Some(row) = vm.current_result_row() {
                        if let Some(pgno) = row[0].to_int() {
                            root_page = Some(pgno as u32);
                        }
                    }
                }
            }
            Ok(())
        })();

        if let Err(e) = res {
            if auto_txn {
                let _ = self.btree.rollback();
            }
            return Err(e);
        }

        if auto_txn {
            self.btree.commit()?;
        }

        // If it was a CREATE TABLE statement, insert into the schema catalog.
        if is_create {
            if let Some(rp) = root_page {
                if let liter_ast::Stmt::Create(ref create_stmt) = ast {
                    if let liter_ast::CreateStmt::Table(ref create_table) = **create_stmt {
                        let columns = match &create_table.body {
                            liter_ast::CreateTableBody::Columns { columns, .. } => columns.clone(),
                            _ => Vec::new(),
                        };

                        self.schema.insert(liter_schema::SchemaObject {
                            kind: liter_schema::ObjectKind::Table,
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
        let ast = liter_parser::parse_stmt(sql)?;

        let mut results = Vec::new();

        let mut vm = liter_codegen::compile_with_schema(&ast, &self.schema)?;
        if log::log_enabled!(log::Level::Trace) {
            trace!("VDBE ops for sql '{}':", sql);
            for (i, op) in vm.ops.iter().enumerate() {
                trace!("{:04} {:?}", i, op);
            }
        }
        vm.func_dispatcher = Some(|name, args| {
            if name.to_ascii_lowercase().starts_with("json") || name == "->" || name == "->>" {
                liter_json::dispatch_function(name, args)
            } else {
                liter_functions::dispatch_function(name, args).map_err(|e| e.to_string())
            }
        });
        vm.agg_dispatcher = Some(liter_functions::dispatch_aggregate);

        let mut cursors: Vec<Option<liter_vdbe::VdbeCursor>> = Vec::with_capacity(vm.n_cursors);
        for _ in 0..vm.n_cursors {
            cursors.push(None);
        }

        while let liter_vdbe::StepResult::Row = vm.step(&self.btree, &mut cursors)? {
            if let Some(row) = vm.current_result_row() {
                let mut out_row = Vec::with_capacity(row.len());
                for mem in row {
                    out_row.push(mem_to_value(mem));
                }
                results.push(out_row);
            }
        }

        Ok(results)
    }

    /// Execute a SQL query expected to return a single value.
    pub fn query_one<T: FromValue>(&self, sql: &str, params: impl IntoParams) -> SqliteResult<T> {
        let rows = self.query(sql, params)?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| SqliteError::Sql("no rows returned".into()))?;
        let val = row
            .into_iter()
            .next()
            .ok_or_else(|| SqliteError::Sql("no columns returned".into()))?;
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
        let ast = liter_parser::parse_stmt(sql)?;
        let mut vm = liter_codegen::compile_with_schema(&ast, &self.schema)?;
        if log::log_enabled!(log::Level::Trace) {
            trace!("VDBE ops for sql '{}':", sql);
            for (i, op) in vm.ops.iter().enumerate() {
                trace!("{:04} {:?}", i, op);
            }
        }
        vm.func_dispatcher = Some(|name, args| {
            if name.to_ascii_lowercase().starts_with("json") || name == "->" || name == "->>" {
                liter_json::dispatch_function(name, args)
            } else {
                liter_functions::dispatch_function(name, args).map_err(|e| e.to_string())
            }
        });
        vm.agg_dispatcher = Some(liter_functions::dispatch_aggregate);
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

fn mem_to_value(mem: &liter_vdbe::Mem) -> Value {
    match mem {
        liter_vdbe::Mem::Null => Value::Null,
        liter_vdbe::Mem::Int(i) => Value::Int(*i),
        liter_vdbe::Mem::Real(f) => Value::Real(*f),
        liter_vdbe::Mem::Text(t) => Value::Text(t.as_bytes().to_vec()),
        liter_vdbe::Mem::Blob(b) => Value::Blob(b.to_vec()),
        liter_vdbe::Mem::ZeroBlob(n) => Value::Blob(vec![0; *n as usize]),
        liter_vdbe::Mem::Agg(_) => Value::Null, // Should not leak out to results, but just in case
    }
}

/// A prepared SQL statement tied to its originating `Connection`.
///
/// Call `step()` repeatedly: it returns `Ok(StepResult::Row)` for each
/// result row (retrieve values via `column_value`) and `Ok(StepResult::Done)`
/// when execution is complete. Call `reset()` to rewind for re-execution.
pub struct Statement<'conn> {
    conn: &'conn Connection,
    vm: liter_vdbe::Vdbe,
    /// One slot per cursor allocated by the compiled program.
    /// Cursor lifetime is tied to `conn.btree` via `'conn`.
    cursors: Vec<Option<liter_vdbe::VdbeCursor<'conn>>>,
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
            liter_vdbe::StepResult::Row => {
                // Cache the current row so column_value() can return values
                // without holding a borrow on `self.vm` simultaneously.
                self.column_cache = if let Some(row) = self.vm.current_result_row() {
                    row.iter().map(mem_to_value).collect()
                } else {
                    Vec::new()
                };
                Ok(StepResult::Row)
            }
            liter_vdbe::StepResult::Done => {
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
        self.column_cache
            .get(col)
            .cloned()
            .ok_or_else(|| SqliteError::Sql(format!("column index {} out of range", col)))
    }

    /// Return the number of result columns in this statement.
    pub fn column_count(&self) -> usize {
        if !self.column_cache.is_empty() {
            self.column_cache.len()
        } else {
            self.vm.num_result_cols()
        }
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
        let rows = conn
            .query("SELECT 42, 'hello', 3.5;", [] as [(); 0])
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 3);
        assert_eq!(rows[0][0], Value::Int(42));
        assert_eq!(rows[0][1], Value::Text(b"hello".to_vec()));
        assert_eq!(rows[0][2], Value::Real(3.5));
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
        conn.execute("CREATE TABLE users (id INTEGER, name TEXT)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO users VALUES (1, 'alice')", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO users VALUES (2, 'bob')", [] as [(); 0])
            .unwrap();

        let rows = conn
            .query("SELECT id, name FROM users", [] as [(); 0])
            .unwrap();
        assert_eq!(rows.len(), 2, "expected 2 rows, got {}", rows.len());
        assert_eq!(rows[0][0], Value::Int(1));
        assert_eq!(rows[0][1], Value::Text(b"alice".to_vec()));
        assert_eq!(rows[1][0], Value::Int(2));
        assert_eq!(rows[1][1], Value::Text(b"bob".to_vec()));
    }

    #[test]
    fn test_select_star_from_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE items (x INTEGER, y INTEGER)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO items VALUES (10, 20)", [] as [(); 0])
            .unwrap();

        let rows = conn.query("SELECT * FROM items", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], Value::Int(10));
        assert_eq!(rows[0][1], Value::Int(20));
    }

    #[test]
    fn test_select_from_where() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (id INTEGER, val TEXT)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO t VALUES (1, 'a')", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO t VALUES (2, 'b')", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO t VALUES (3, 'c')", [] as [(); 0])
            .unwrap();

        // WHERE with equality — should return only id=2
        let rows = conn
            .query("SELECT id, val FROM t WHERE id = 2", [] as [(); 0])
            .unwrap();
        assert_eq!(rows.len(), 1, "expected 1 matching row, got {}", rows.len());
        assert_eq!(rows[0][0], Value::Int(2));
        assert_eq!(rows[0][1], Value::Text(b"b".to_vec()));
    }

    #[test]
    fn test_select_from_empty_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE empty (id INTEGER)", [] as [(); 0])
            .unwrap();
        let rows = conn.query("SELECT id FROM empty", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 0, "expected 0 rows from empty table");
    }

    // ── Phase 9: Statement API ────────────────────────────────────────────────

    #[test]
    fn test_prepare_step_column_value() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE p (x INTEGER, y TEXT)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO p VALUES (1, 'one')", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO p VALUES (2, 'two')", [] as [(); 0])
            .unwrap();

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
        conn.execute("CREATE TABLE r (n INTEGER)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO r VALUES (42)", [] as [(); 0])
            .unwrap();

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
        assert_eq!(
            stmt.column_value(1).unwrap(),
            Value::Text(b"hello".to_vec())
        );
        assert_eq!(stmt.step().unwrap(), StepResult::Done);
    }

    // ── Phase 10: DELETE, UPDATE, AND/OR, Transactions ────────────────────────

    #[test]
    fn test_delete_all() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE d (x INTEGER)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO d VALUES (1)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO d VALUES (2)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO d VALUES (3)", [] as [(); 0])
            .unwrap();

        conn.execute("DELETE FROM d", [] as [(); 0]).unwrap();

        let rows = conn.query("SELECT x FROM d", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 0, "all rows should be deleted");
    }

    #[test]
    fn test_delete_where() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE d2 (id INTEGER, val TEXT)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO d2 VALUES (1, 'keep')", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO d2 VALUES (2, 'drop')", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO d2 VALUES (3, 'keep')", [] as [(); 0])
            .unwrap();

        conn.execute("DELETE FROM d2 WHERE id = 2", [] as [(); 0])
            .unwrap();

        let rows = conn.query("SELECT id FROM d2", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 2, "only 1 row should be deleted");
        assert_eq!(rows[0][0], Value::Int(1));
        assert_eq!(rows[1][0], Value::Int(3));
    }

    #[test]
    fn test_update_all() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE u (id INTEGER, score INTEGER)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO u VALUES (1, 10)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO u VALUES (2, 20)", [] as [(); 0])
            .unwrap();

        conn.execute("UPDATE u SET score = 99", [] as [(); 0])
            .unwrap();

        let rows = conn.query("SELECT score FROM u", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], Value::Int(99));
        assert_eq!(rows[1][0], Value::Int(99));
    }

    #[test]
    fn test_update_where() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE u2 (id INTEGER, val TEXT)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO u2 VALUES (1, 'old')", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO u2 VALUES (2, 'old')", [] as [(); 0])
            .unwrap();

        conn.execute("UPDATE u2 SET val = 'new' WHERE id = 1", [] as [(); 0])
            .unwrap();

        let rows = conn.query("SELECT id, val FROM u2", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][1], Value::Text(b"new".to_vec()));
        assert_eq!(rows[1][1], Value::Text(b"old".to_vec()));
    }

    #[test]
    fn test_where_and() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE w (a INTEGER, b INTEGER)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO w VALUES (1, 10)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO w VALUES (2, 20)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO w VALUES (3, 30)", [] as [(); 0])
            .unwrap();

        // a > 1 AND b < 30 should match only row (2, 20)
        let rows = conn
            .query("SELECT a FROM w WHERE a > 1 AND b < 30", [] as [(); 0])
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], Value::Int(2));
    }

    #[test]
    fn test_explicit_transaction_commit() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE tx (n INTEGER)", [] as [(); 0])
            .unwrap();

        conn.execute("BEGIN", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO tx VALUES (42)", [] as [(); 0])
            .unwrap();
        conn.execute("COMMIT", [] as [(); 0]).unwrap();

        let rows = conn.query("SELECT n FROM tx", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], Value::Int(42));
    }

    #[test]
    fn test_explicit_transaction_rollback() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE tx2 (n INTEGER)", [] as [(); 0])
            .unwrap();

        conn.execute("BEGIN", [] as [(); 0]).unwrap();
        conn.execute("INSERT INTO tx2 VALUES (99)", [] as [(); 0])
            .unwrap();
        conn.execute("ROLLBACK", [] as [(); 0]).unwrap();

        let rows = conn.query("SELECT n FROM tx2", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 0, "rollback should undo the insert");
    }

    #[test]
    fn test_sqlite_error_variants_and_conversions() {
        let err_sql = SqliteError::Sql("syntax error".into());
        assert_eq!(err_sql.to_string(), "SQL error: syntax error");

        let err_nomem = SqliteError::NoMem;
        assert_eq!(err_nomem.to_string(), "out of memory");

        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err_io: SqliteError = io_err.into();
        assert!(err_io.to_string().contains("I/O error"));

        let err_corrupt = SqliteError::Corrupt;
        assert_eq!(err_corrupt.to_string(), "database is corrupt");

        let err_busy = SqliteError::Busy;
        assert_eq!(err_busy.to_string(), "database is busy");

        let err_constraint = SqliteError::Constraint("unique".into());
        assert_eq!(err_constraint.to_string(), "constraint violation: unique");

        let err_auth = SqliteError::Auth;
        assert_eq!(err_auth.to_string(), "authorization denied");

        let err_not_impl = SqliteError::NotImplemented;
        assert_eq!(err_not_impl.to_string(), "not yet implemented");

        let err_parse = SqliteError::Parse("unexpected token".into());
        assert_eq!(err_parse.to_string(), "parse error: unexpected token");

        // ParseError From conversion
        let parse_err = liter_parser::ParseError::SyntaxError("unexpected".into());
        let err_from_parse: SqliteError = parse_err.into();
        assert!(matches!(err_from_parse, SqliteError::Parse(_)));

        // CodegenError From conversion
        let codegen_err = liter_codegen::CodegenError::Internal("test".into());
        let err_from_codegen: SqliteError = codegen_err.into();
        assert!(matches!(err_from_codegen, SqliteError::Sql(_)));

        // VdbeError From conversion
        let vdbe_err = liter_vdbe::VdbeError::Exec("halt".into());
        let err_from_vdbe: SqliteError = vdbe_err.into();
        assert!(matches!(err_from_vdbe, SqliteError::Sql(_)));

        // BTreeError From conversion
        let btree_err = liter_btree::BTreeError::Corrupt;
        let err_from_btree: SqliteError = btree_err.into();
        assert!(matches!(err_from_btree, SqliteError::Sql(_)));
    }

    #[test]
    fn test_savepoint_and_release_noop() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(conn.execute("SAVEPOINT sp1", [] as [(); 0]).is_ok());
        assert!(conn.execute("RELEASE sp1", [] as [(); 0]).is_ok());
    }

    #[test]
    fn test_wasm_connection() {
        let wasm_conn = WasmConnection::new();
        assert!(wasm_conn.is_ok());
    }

    #[test]
    fn test_query_one_and_from_value() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE q1 (i INT, r REAL, t TEXT)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO q1 VALUES (123, 45.67, 'hello')", [] as [(); 0])
            .unwrap();

        // Int conversions
        let int_val: i64 = conn.query_one("SELECT i FROM q1", [] as [(); 0]).unwrap();
        assert_eq!(int_val, 123);
        let int_from_real: i64 = conn.query_one("SELECT 10.5", [] as [(); 0]).unwrap();
        assert_eq!(int_from_real, 10);
        let err_int: SqliteResult<i64> = conn.query_one("SELECT t FROM q1", [] as [(); 0]);
        assert!(err_int.is_err());

        // Real conversions
        let real_val: f64 = conn.query_one("SELECT r FROM q1", [] as [(); 0]).unwrap();
        assert_eq!(real_val, 45.67);
        let real_from_int: f64 = conn.query_one("SELECT 5", [] as [(); 0]).unwrap();
        assert_eq!(real_from_int, 5.0);
        let err_real: SqliteResult<f64> = conn.query_one("SELECT t FROM q1", [] as [(); 0]);
        assert!(err_real.is_err());

        // String conversions
        let text_val: String = conn.query_one("SELECT t FROM q1", [] as [(); 0]).unwrap();
        assert_eq!(text_val, "hello");
        let err_text: SqliteResult<String> = conn.query_one("SELECT i FROM q1", [] as [(); 0]);
        assert!(err_text.is_err());

        // Invalid UTF-8 String conversion
        let bad_utf8_val = Value::Text(vec![0xff, 0xfe, 0xfd]);
        assert!(String::from_value(bad_utf8_val).is_err());

        // No rows returned
        let err_no_rows: SqliteResult<i64> =
            conn.query_one("SELECT i FROM q1 WHERE i = 999", [] as [(); 0]);
        assert!(
            matches!(err_no_rows, Err(SqliteError::Sql(msg)) if msg.contains("no rows returned"))
        );
    }

    #[test]
    fn test_query_row() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE qr (id INT, val TEXT)", [] as [(); 0])
            .unwrap();
        conn.execute("INSERT INTO qr VALUES (1, 'one')", [] as [(); 0])
            .unwrap();

        let row = conn
            .query_row("SELECT id, val FROM qr WHERE id = 1", [] as [(); 0])
            .unwrap();
        assert_eq!(row.len(), 2);
        assert_eq!(row[0], Value::Int(1));
        assert_eq!(row[1], Value::Text(b"one".to_vec()));

        let err = conn.query_row("SELECT id, val FROM qr WHERE id = 999", [] as [(); 0]);
        assert!(err.is_err());
    }

    #[test]
    fn test_disk_database_and_schema_loading() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("disk_test.db");
        let db_path_str = db_path.to_str().unwrap();

        {
            let conn = Connection::open(db_path_str).unwrap();
            assert_eq!(conn.path(), db_path_str);
            conn.btree.begin_write().unwrap();
            let tbl_pgno = conn
                .btree
                .allocate_page(liter_btree::PageKind::TableLeaf)
                .unwrap();
            let schema_record = liter_record::encode_record(&[
                Value::Text(b"table".to_vec()),
                Value::Text(b"disk_tbl".to_vec()),
                Value::Text(b"disk_tbl".to_vec()),
                Value::Int(tbl_pgno as i64),
                Value::Text(b"CREATE TABLE disk_tbl (x INT, y TEXT)".to_vec()),
            ])
            .unwrap();
            let mut cur = conn.btree.cursor(1, true).unwrap();
            cur.insert(&1u64.to_be_bytes(), &schema_record, false)
                .unwrap();
            conn.btree.commit().unwrap();
        }

        // Reopen database from disk and ensure schema is recovered from page 1
        {
            let conn = Connection::open(db_path_str).unwrap();
            let tables = conn.schema.tables();
            assert_eq!(tables.len(), 1);
            assert_eq!(tables[0].name, "disk_tbl");
        }
    }

    #[test]
    fn test_statement_column_value_out_of_range() {
        let conn = Connection::open_in_memory().unwrap();
        let mut stmt = conn.prepare("SELECT 10, 20").unwrap();
        assert_eq!(stmt.step().unwrap(), StepResult::Row);
        assert!(stmt.column_value(5).is_err());
    }

    #[test]
    fn test_mem_to_value_all_variants() {
        assert_eq!(mem_to_value(&liter_vdbe::Mem::Null), Value::Null);
        assert_eq!(mem_to_value(&liter_vdbe::Mem::Int(42)), Value::Int(42));
        assert_eq!(mem_to_value(&liter_vdbe::Mem::Real(3.5)), Value::Real(3.5));
        assert_eq!(
            mem_to_value(&liter_vdbe::Mem::Text("abc".into())),
            Value::Text(b"abc".to_vec())
        );
        assert_eq!(
            mem_to_value(&liter_vdbe::Mem::Blob(std::sync::Arc::from(
                vec![1, 2, 3].into_boxed_slice()
            ))),
            Value::Blob(vec![1, 2, 3])
        );
        assert_eq!(
            mem_to_value(&liter_vdbe::Mem::ZeroBlob(3)),
            Value::Blob(vec![0, 0, 0])
        );
        assert_eq!(mem_to_value(&liter_vdbe::Mem::Agg(42)), Value::Null);
    }

    #[test]
    fn test_query_json_and_scalar_functions() {
        let conn = Connection::open_in_memory().unwrap();
        let rows = conn
            .query("SELECT json_extract('{\"a\": 10}', '$.a')", [] as [(); 0])
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], Value::Int(10));

        let rows_len = conn.query("SELECT length('hello')", [] as [(); 0]).unwrap();
        assert_eq!(rows_len.len(), 1);
        assert_eq!(rows_len[0][0], Value::Int(5));
    }

    #[test]
    fn test_into_params_vec_and_slice() {
        let conn = Connection::open_in_memory().unwrap();
        let slice: &[Value] = &[Value::Int(1)];
        assert!(conn.query("SELECT 1", slice).is_ok());

        let vec: Vec<Value> = vec![Value::Int(2)];
        assert!(conn.query("SELECT 2", vec).is_ok());
    }

    #[test]
    fn test_trace_logging_and_schema_edge_cases() {
        struct TestLogger;
        impl log::Log for TestLogger {
            fn enabled(&self, _metadata: &log::Metadata) -> bool {
                true
            }
            fn log(&self, _record: &log::Record) {}
            fn flush(&self) {}
        }
        static LOGGER: TestLogger = TestLogger;
        let _ = log::set_logger(&LOGGER);
        log::set_max_level(log::LevelFilter::Trace);
        log::logger().flush();

        let conn = Connection::open_in_memory().unwrap();
        let _ = conn.execute("CREATE TABLE t_as AS SELECT 1", [] as [(); 0]);
        let _ = conn.query("SELECT 1", [] as [(); 0]);
        let mut stmt = conn.prepare("SELECT 2").unwrap();
        let _ = stmt.step();

        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("schema_malformed.db");
        let db_path_str = db_path.to_str().unwrap();

        {
            let conn = Connection::open(db_path_str).unwrap();
            conn.btree.begin_write().unwrap();
            let _ = conn
                .btree
                .allocate_page(liter_btree::PageKind::TableLeaf)
                .unwrap();
            let mut cur = conn.btree.cursor(1, true).unwrap();

            // Record with non-matching column types (Int for type, Text for rootpage, Int for sql)
            let rec_wrong_types = liter_record::encode_record(&[
                Value::Int(123),
                Value::Text(b"tbl_wrong".to_vec()),
                Value::Text(b"tbl_wrong".to_vec()),
                Value::Text(b"not_int_page".to_vec()),
                Value::Int(456),
            ])
            .unwrap();
            cur.insert(&10u64.to_be_bytes(), &rec_wrong_types, false)
                .unwrap();

            // Record with fewer than 3 elements
            let rec_short = liter_record::encode_record(&[Value::Text(b"table".to_vec())]).unwrap();
            cur.insert(&11u64.to_be_bytes(), &rec_short, false).unwrap();

            // Record with CREATE TABLE AS SELECT
            let rec_as_select = liter_record::encode_record(&[
                Value::Text(b"table".to_vec()),
                Value::Text(b"tbl_as".to_vec()),
                Value::Text(b"tbl_as".to_vec()),
                Value::Int(6),
                Value::Text(b"CREATE TABLE tbl_as AS SELECT 1".to_vec()),
            ])
            .unwrap();
            cur.insert(&12u64.to_be_bytes(), &rec_as_select, false)
                .unwrap();

            // Invalid raw bytes that fail decode_record
            cur.insert(&13u64.to_be_bytes(), &[0xFF, 0xFF, 0xFF], false)
                .unwrap();

            conn.btree.commit().unwrap();
        }

        let _conn_reopened = Connection::open(db_path_str).unwrap();
    }

    #[test]
    fn test_schema_loading_edge_cases() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("schema_edge.db");
        let db_path_str = db_path.to_str().unwrap();

        {
            let conn = Connection::open(db_path_str).unwrap();
            conn.btree.begin_write().unwrap();
            let _ = conn
                .btree
                .allocate_page(liter_btree::PageKind::TableLeaf)
                .unwrap();
            let mut cur = conn.btree.cursor(1, true).unwrap();

            // 1. Valid Table
            let rec_valid = liter_record::encode_record(&[
                Value::Text(b"table".to_vec()),
                Value::Text(b"tbl_valid".to_vec()),
                Value::Text(b"tbl_valid".to_vec()),
                Value::Int(2),
                Value::Text(b"CREATE TABLE tbl_valid (x INT)".to_vec()),
            ])
            .unwrap();
            cur.insert(&1u64.to_be_bytes(), &rec_valid, false).unwrap();

            // 2. Table with invalid SQL (parse error warning)
            let rec_bad_sql = liter_record::encode_record(&[
                Value::Text(b"table".to_vec()),
                Value::Text(b"tbl_bad".to_vec()),
                Value::Text(b"tbl_bad".to_vec()),
                Value::Int(3),
                Value::Text(b"INVALID SQL STATEMENT".to_vec()),
            ])
            .unwrap();
            cur.insert(&2u64.to_be_bytes(), &rec_bad_sql, false)
                .unwrap();

            // 3. Table with non-create SQL
            let rec_non_create = liter_record::encode_record(&[
                Value::Text(b"table".to_vec()),
                Value::Text(b"tbl_select".to_vec()),
                Value::Text(b"tbl_select".to_vec()),
                Value::Int(4),
                Value::Text(b"SELECT 1".to_vec()),
            ])
            .unwrap();
            cur.insert(&3u64.to_be_bytes(), &rec_non_create, false)
                .unwrap();

            // 4. Non-table schema objects (index, view, trigger)
            let rec_index = liter_record::encode_record(&[
                Value::Text(b"index".to_vec()),
                Value::Text(b"idx_test".to_vec()),
                Value::Text(b"tbl_valid".to_vec()),
                Value::Int(5),
                Value::Text(b"CREATE INDEX idx_test ON tbl_valid(x)".to_vec()),
            ])
            .unwrap();
            cur.insert(&4u64.to_be_bytes(), &rec_index, false).unwrap();

            conn.btree.commit().unwrap();
        }

        // Reopen database and verify schema loaded as expected
        let conn2 = Connection::open(db_path_str).unwrap();
        assert!(conn2.schema.get("tbl_valid").is_some());
    }

    #[test]
    fn test_execute_with_scalar_func_and_json_arrow() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t_funcs (a INT, b TEXT)", [] as [(); 0])
            .unwrap();
        conn.execute(
            "INSERT INTO t_funcs VALUES (abs(10), 'hello')",
            [] as [(); 0],
        )
        .unwrap();

        let rows = conn.query("SELECT a FROM t_funcs", [] as [(); 0]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], Value::Int(10));

        // Test arrow JSON operators
        let rows_arrow = conn
            .query("SELECT json_extract('{\"x\": 42}', '$.x')", [] as [(); 0])
            .unwrap();
        assert_eq!(rows_arrow[0][0], Value::Int(42));
    }

    #[test]
    fn test_execute_error_rollback_and_func_errors() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t_err (a INT)", [] as [(); 0])
            .unwrap();

        // Unknown function in execute triggers step error and auto-txn rollback
        let res_exec = conn.execute("INSERT INTO t_err VALUES (unknown_fn(1))", [] as [(); 0]);
        assert!(res_exec.is_err());

        // Unknown function in query triggers step error
        let res_query = conn.query("SELECT unknown_fn(1)", [] as [(); 0]);
        assert!(res_query.is_err());
    }

    #[test]
    fn test_current_datetime_literals() {
        let conn = Connection::open_in_memory().unwrap();
        let rows = conn
            .query(
                "SELECT CURRENT_DATE, CURRENT_TIME, CURRENT_TIMESTAMP",
                [] as [(); 0],
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.len(), 3);

        // CURRENT_DATE should be "YYYY-MM-DD"
        match &row[0] {
            Value::Text(t) => {
                let s = std::str::from_utf8(t).unwrap();
                assert_eq!(s.len(), 10);
                assert_eq!(s.chars().filter(|c| *c == '-').count(), 2);
            }
            other => panic!("expected text for CURRENT_DATE, got {:?}", other),
        }

        // CURRENT_TIME should be "HH:MM:SS"
        match &row[1] {
            Value::Text(t) => {
                let s = std::str::from_utf8(t).unwrap();
                assert_eq!(s.len(), 8);
                assert_eq!(s.chars().filter(|c| *c == ':').count(), 2);
            }
            other => panic!("expected text for CURRENT_TIME, got {:?}", other),
        }

        // CURRENT_TIMESTAMP should be "YYYY-MM-DD HH:MM:SS"
        match &row[2] {
            Value::Text(t) => {
                let s = std::str::from_utf8(t).unwrap();
                assert_eq!(s.len(), 19);
                assert_eq!(s.chars().filter(|c| *c == '-').count(), 2);
                assert_eq!(s.chars().filter(|c| *c == ':').count(), 2);
            }
            other => panic!("expected text for CURRENT_TIMESTAMP, got {:?}", other),
        }
    }
}
