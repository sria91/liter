#![allow(clippy::missing_safety_doc)]
#![allow(clippy::manual_c_str_literals)]

//! C ABI compatibility layer for Liter-rs.
//!
//! Exports `#[no_mangle] extern "C"` symbols matching `sqlite3.h`, enabling
//! this library to be used as a drop-in replacement for `libsqlite3.so`.
//!
//! ## Status
//! Phase 4 — skeleton with SQLITE_* constants and `sqlite3_open` / `sqlite3_close`
//! stubs. Full implementation follows once the Rust API in `sqlite3` crate is stable.

use liter::{Connection, SqliteError, Statement, Value};
use std::ffi::{c_char, c_int, c_void, CStr, CString};

// ── SQLITE_* result codes ─────────────────────────────────────────────────────
pub const SQLITE_OK: c_int = 0;
pub const SQLITE_ERROR: c_int = 1;
pub const SQLITE_INTERNAL: c_int = 2;
pub const SQLITE_PERM: c_int = 3;
pub const SQLITE_ABORT: c_int = 4;
pub const SQLITE_BUSY: c_int = 5;
pub const SQLITE_LOCKED: c_int = 6;
pub const SQLITE_NOMEM: c_int = 7;
pub const SQLITE_READONLY: c_int = 8;
pub const SQLITE_INTERRUPT: c_int = 9;
pub const SQLITE_IOERR: c_int = 10;
pub const SQLITE_CORRUPT: c_int = 11;
pub const SQLITE_NOTFOUND: c_int = 12;
pub const SQLITE_FULL: c_int = 13;
pub const SQLITE_CANTOPEN: c_int = 14;
pub const SQLITE_PROTOCOL: c_int = 15;
pub const SQLITE_EMPTY: c_int = 16;
pub const SQLITE_SCHEMA: c_int = 17;
pub const SQLITE_TOOBIG: c_int = 18;
pub const SQLITE_CONSTRAINT: c_int = 19;
pub const SQLITE_MISMATCH: c_int = 20;
pub const SQLITE_MISUSE: c_int = 21;
pub const SQLITE_NOLFS: c_int = 22;
pub const SQLITE_AUTH: c_int = 23;
pub const SQLITE_FORMAT: c_int = 24;
pub const SQLITE_RANGE: c_int = 25;
pub const SQLITE_NOTADB: c_int = 26;
pub const SQLITE_NOTICE: c_int = 27;
pub const SQLITE_WARNING: c_int = 28;
pub const SQLITE_ROW: c_int = 100;
pub const SQLITE_DONE: c_int = 101;

// ── SQLITE_* data types ───────────────────────────────────────────────────────
pub const SQLITE_INTEGER: c_int = 1;
pub const SQLITE_FLOAT: c_int = 2;
pub const SQLITE_TEXT: c_int = 3;
pub const SQLITE_BLOB: c_int = 4;
pub const SQLITE_NULL: c_int = 5;

/// Opaque database connection handle (`sqlite3*` in C).
#[allow(non_camel_case_types)]
pub struct sqlite3(Connection);

/// Opaque prepared statement handle (`sqlite3_stmt*` in C).
#[allow(non_camel_case_types)]
pub struct sqlite3_stmt {
    stmt: Statement<'static>,
    text_cache: std::collections::HashMap<usize, CString>,
}

pub type Sqlite3Callback =
    Option<unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int>;

fn map_err(err: SqliteError) -> c_int {
    match err {
        SqliteError::Sql(_) => SQLITE_ERROR,
        SqliteError::NoMem => SQLITE_NOMEM,
        SqliteError::Io(_) => SQLITE_IOERR,
        SqliteError::Corrupt => SQLITE_CORRUPT,
        SqliteError::Busy => SQLITE_BUSY,
        SqliteError::Constraint(_) => SQLITE_CONSTRAINT,
        SqliteError::Auth => SQLITE_AUTH,
        SqliteError::NotImplemented => SQLITE_ERROR,
        SqliteError::Parse(_) => SQLITE_ERROR,
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_open(filename: *const c_char, pp_db: *mut *mut sqlite3) -> c_int {
    if pp_db.is_null() {
        return SQLITE_MISUSE;
    }
    let path = if filename.is_null() {
        ":memory:".to_owned()
    } else {
        // SAFETY: caller guarantees filename is a valid null-terminated C string.
        CStr::from_ptr(filename).to_string_lossy().into_owned()
    };

    match Connection::open(&path) {
        Ok(conn) => {
            // SAFETY: pp_db was checked non-null above.
            *pp_db = Box::into_raw(Box::new(sqlite3(conn)));
            SQLITE_OK
        }
        Err(_) => SQLITE_CANTOPEN,
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_close(db: *mut sqlite3) -> c_int {
    if !db.is_null() {
        // SAFETY: db must have been obtained from sqlite3_open; we own it.
        drop(Box::from_raw(db));
    }
    SQLITE_OK
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_exec(
    db: *mut sqlite3,
    sql: *const c_char,
    _callback: Sqlite3Callback,
    _cb_arg: *mut c_void,
    _errmsg: *mut *mut c_char,
) -> c_int {
    if db.is_null() || sql.is_null() {
        return SQLITE_MISUSE;
    }

    let conn = &(*db).0;
    let sql_str = CStr::from_ptr(sql).to_string_lossy();

    match conn.execute(&sql_str, [] as [(); 0]) {
        Ok(_) => SQLITE_OK,
        Err(e) => map_err(e),
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_prepare_v2(
    db: *mut sqlite3,
    z_sql: *const c_char,
    _n_byte: c_int,
    pp_stmt: *mut *mut sqlite3_stmt,
    _pz_tail: *mut *const c_char,
) -> c_int {
    if db.is_null() || z_sql.is_null() || pp_stmt.is_null() {
        return SQLITE_MISUSE;
    }

    *pp_stmt = std::ptr::null_mut(); // Start out null
    let conn = &(*db).0;
    let sql_str = CStr::from_ptr(z_sql).to_string_lossy();

    match conn.prepare(&sql_str) {
        Ok(stmt) => {
            // Transmute the lifetime of the statement to 'static.
            // SQLite API documentation dictates that prepared statements must be finalized
            // before the connection is closed, so the Connection outlives the Statement.
            let stmt: Statement<'static> = std::mem::transmute(stmt);
            let s = Box::new(sqlite3_stmt {
                stmt,
                text_cache: std::collections::HashMap::new(),
            });
            *pp_stmt = Box::into_raw(s);
            SQLITE_OK
        }
        Err(e) => map_err(e),
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_step(stmt: *mut sqlite3_stmt) -> c_int {
    if stmt.is_null() {
        return SQLITE_MISUSE;
    }

    let stmt_ref = &mut *stmt;
    stmt_ref.text_cache.clear();

    match stmt_ref.stmt.step() {
        Ok(liter::StepResult::Row) => SQLITE_ROW,
        Ok(liter::StepResult::Done) => SQLITE_DONE,
        Err(e) => map_err(e),
    }
}
#[no_mangle]
pub unsafe extern "C" fn sqlite3_reset(stmt: *mut sqlite3_stmt) -> c_int {
    if stmt.is_null() {
        return SQLITE_MISUSE;
    }

    let stmt_ref = &mut *stmt;
    stmt_ref.text_cache.clear();

    match stmt_ref.stmt.reset() {
        Ok(()) => SQLITE_OK,
        // `Statement::reset` only ever fails via `Vdbe::reset`, which is
        // currently infallible (always returns `Ok`), so this can't
        // actually happen today; kept as a real match arm rather than an
        // unwrap in case that changes.
        Err(e) => map_err(e),
    }
}
#[no_mangle]
pub unsafe extern "C" fn sqlite3_finalize(stmt: *mut sqlite3_stmt) -> c_int {
    if !stmt.is_null() {
        drop(Box::from_raw(stmt));
    }
    SQLITE_OK
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_int64(stmt: *mut sqlite3_stmt, i_col: c_int) -> i64 {
    if stmt.is_null() {
        return 0;
    }
    let stmt_ref = &*stmt;
    match stmt_ref.stmt.column_value(i_col as usize) {
        Ok(Value::Int(i)) => i,
        Ok(Value::Real(f)) => f as i64,
        _ => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_double(stmt: *mut sqlite3_stmt, i_col: c_int) -> f64 {
    if stmt.is_null() {
        return 0.0;
    }
    let stmt_ref = &*stmt;
    match stmt_ref.stmt.column_value(i_col as usize) {
        Ok(Value::Real(f)) => f,
        Ok(Value::Int(i)) => i as f64,
        _ => 0.0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_text(
    stmt: *mut sqlite3_stmt,
    i_col: c_int,
) -> *const c_char {
    if stmt.is_null() {
        return std::ptr::null();
    }
    let stmt_ref = &mut *stmt;
    let idx = i_col as usize;

    if let Some(cstr) = stmt_ref.text_cache.get(&idx) {
        return cstr.as_ptr();
    }

    match stmt_ref.stmt.column_value(idx) {
        Ok(Value::Text(t)) => {
            if let Ok(cstr) = CString::new(t) {
                let ptr = cstr.as_ptr();
                stmt_ref.text_cache.insert(idx, cstr);
                ptr
            } else {
                std::ptr::null()
            }
        }
        Ok(Value::Int(i)) => {
            let s = i.to_string();
            let cstr = CString::new(s).unwrap();
            let ptr = cstr.as_ptr();
            stmt_ref.text_cache.insert(idx, cstr);
            ptr
        }
        Ok(Value::Real(f)) => {
            let s = f.to_string();
            let cstr = CString::new(s).unwrap();
            let ptr = cstr.as_ptr();
            stmt_ref.text_cache.insert(idx, cstr);
            ptr
        }
        _ => std::ptr::null(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_type(stmt: *mut sqlite3_stmt, i_col: c_int) -> c_int {
    if stmt.is_null() {
        return 0;
    }
    let stmt_ref = &*stmt;
    match stmt_ref.stmt.column_value(i_col as usize) {
        Ok(Value::Int(_)) => SQLITE_INTEGER,
        Ok(Value::Real(_)) => SQLITE_FLOAT,
        Ok(Value::Text(_)) => SQLITE_TEXT,
        Ok(Value::Blob(_)) => SQLITE_BLOB,
        Ok(Value::Null) => SQLITE_NULL,
        _ => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_bytes(stmt: *mut sqlite3_stmt, i_col: c_int) -> c_int {
    if stmt.is_null() {
        return 0;
    }
    let stmt_ref = &*stmt;
    match stmt_ref.stmt.column_value(i_col as usize) {
        Ok(Value::Text(t)) => t.len() as c_int,
        Ok(Value::Blob(b)) => b.len() as c_int,
        Ok(Value::Int(i)) => i.to_string().len() as c_int,
        Ok(Value::Real(f)) => f.to_string().len() as c_int,
        _ => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_count(stmt: *mut sqlite3_stmt) -> c_int {
    if stmt.is_null() {
        return 0;
    }
    let stmt_ref = &*stmt;
    stmt_ref.stmt.column_count() as c_int
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_name(_stmt: *mut sqlite3_stmt, _n: c_int) -> *const c_char {
    // Currently, our Statement does not expose column names.
    // Returning an empty string literal.
    b"\0".as_ptr() as *const c_char
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_errmsg(_db: *mut sqlite3) -> *const c_char {
    b"not implemented\0".as_ptr() as *const c_char
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_changes(_db: *mut sqlite3) -> c_int {
    0 // Stub: connection does not track changes yet
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_last_insert_rowid(_db: *mut sqlite3) -> i64 {
    0 // Stub: connection does not track last rowid yet
}

// ── Bind Parameters (Stubs) ───────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_parameter_count(_stmt: *mut sqlite3_stmt) -> c_int {
    0 // Parameter binding not implemented
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_int64(
    _stmt: *mut sqlite3_stmt,
    _i: c_int,
    _val: i64,
) -> c_int {
    SQLITE_ERROR
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_double(
    _stmt: *mut sqlite3_stmt,
    _i: c_int,
    _val: f64,
) -> c_int {
    SQLITE_ERROR
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_null(_stmt: *mut sqlite3_stmt, _i: c_int) -> c_int {
    SQLITE_ERROR
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_text(
    _stmt: *mut sqlite3_stmt,
    _i: c_int,
    _data: *const c_char,
    _n: c_int,
    _destroy: Option<unsafe extern "C" fn(*mut c_void)>,
) -> c_int {
    SQLITE_ERROR
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_blob(
    _stmt: *mut sqlite3_stmt,
    _i: c_int,
    _data: *const c_void,
    _n: c_int,
    _destroy: Option<unsafe extern "C" fn(*mut c_void)>,
) -> c_int {
    SQLITE_ERROR
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_libversion() -> *const c_char {
    b"3.53.0\0".as_ptr() as *const c_char
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_libversion_number() -> c_int {
    3_053_000
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_free(_p: *mut c_void) {
    // liter-ffi does not currently allocate error strings through a C-compatible
    // allocator, so sqlite3_exec() never sets *pzErrMsg. This function is always
    // called with NULL and is intentionally a no-op.
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    /// Open an in-memory database, panicking on failure. Helper for tests
    /// that need a live connection but aren't testing `sqlite3_open` itself.
    unsafe fn open_mem() -> *mut sqlite3 {
        let mut db: *mut sqlite3 = ptr::null_mut();
        let rc = sqlite3_open(b":memory:\0".as_ptr() as *const c_char, &mut db);
        assert_eq!(rc, SQLITE_OK);
        assert!(!db.is_null());
        db
    }

    /// Prepare `sql` against `db`, panicking on failure.
    unsafe fn prepare(db: *mut sqlite3, sql: &str) -> *mut sqlite3_stmt {
        let c_sql = CString::new(sql).unwrap();
        let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
        let rc = sqlite3_prepare_v2(db, c_sql.as_ptr(), -1, &mut stmt, ptr::null_mut());
        assert_eq!(rc, SQLITE_OK, "prepare failed for {sql:?}");
        assert!(!stmt.is_null());
        stmt
    }

    unsafe fn exec_ok(db: *mut sqlite3, sql: &str) {
        let c_sql = CString::new(sql).unwrap();
        let rc = sqlite3_exec(db, c_sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut());
        assert_eq!(rc, SQLITE_OK, "exec failed for {sql:?}");
    }

    // ── map_err ────────────────────────────────────────────────────────────

    #[test]
    fn map_err_covers_every_variant() {
        assert_eq!(map_err(SqliteError::Sql("x".into())), SQLITE_ERROR);
        assert_eq!(map_err(SqliteError::NoMem), SQLITE_NOMEM);
        assert_eq!(
            map_err(SqliteError::Io(std::io::Error::other("boom"))),
            SQLITE_IOERR
        );
        assert_eq!(map_err(SqliteError::Corrupt), SQLITE_CORRUPT);
        assert_eq!(map_err(SqliteError::Busy), SQLITE_BUSY);
        assert_eq!(
            map_err(SqliteError::Constraint("dup".into())),
            SQLITE_CONSTRAINT
        );
        assert_eq!(map_err(SqliteError::Auth), SQLITE_AUTH);
        assert_eq!(map_err(SqliteError::NotImplemented), SQLITE_ERROR);
        assert_eq!(map_err(SqliteError::Parse("bad sql".into())), SQLITE_ERROR);
    }

    // ── sqlite3_open / sqlite3_close ─────────────────────────────────────────

    #[test]
    fn open_null_pp_db_is_misuse() {
        unsafe {
            let rc = sqlite3_open(b":memory:\0".as_ptr() as *const c_char, ptr::null_mut());
            assert_eq!(rc, SQLITE_MISUSE);
        }
    }

    #[test]
    fn open_null_filename_defaults_to_memory() {
        unsafe {
            let mut db: *mut sqlite3 = ptr::null_mut();
            let rc = sqlite3_open(ptr::null(), &mut db);
            assert_eq!(rc, SQLITE_OK);
            assert!(!db.is_null());
            assert_eq!(sqlite3_close(db), SQLITE_OK);
        }
    }

    #[test]
    fn open_invalid_path_returns_cantopen() {
        unsafe {
            let path =
                CString::new("/no/such/directory/liter-ffi-test-db-does-not-exist.sqlite").unwrap();
            let mut db: *mut sqlite3 = ptr::null_mut();
            let rc = sqlite3_open(path.as_ptr(), &mut db);
            assert_eq!(rc, SQLITE_CANTOPEN);
            assert!(db.is_null());
        }
    }

    #[test]
    fn open_real_file_path_succeeds_and_can_reopen() {
        unsafe {
            let mut path = std::env::temp_dir();
            path.push(format!("liter_ffi_test_{}.sqlite", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let c_path = CString::new(path.to_str().unwrap()).unwrap();

            let mut db: *mut sqlite3 = ptr::null_mut();
            let rc = sqlite3_open(c_path.as_ptr(), &mut db);
            assert_eq!(rc, SQLITE_OK);
            assert!(!db.is_null());
            assert_eq!(sqlite3_close(db), SQLITE_OK);

            std::fs::remove_file(&path).ok();
        }
    }

    #[test]
    fn close_null_is_a_noop_ok() {
        unsafe {
            assert_eq!(sqlite3_close(ptr::null_mut()), SQLITE_OK);
        }
    }

    // ── sqlite3_exec ──────────────────────────────────────────────────────────

    #[test]
    fn exec_null_db_is_misuse() {
        unsafe {
            let sql = b"SELECT 1;\0".as_ptr() as *const c_char;
            let rc = sqlite3_exec(ptr::null_mut(), sql, None, ptr::null_mut(), ptr::null_mut());
            assert_eq!(rc, SQLITE_MISUSE);
        }
    }

    #[test]
    fn exec_null_sql_is_misuse() {
        unsafe {
            let db = open_mem();
            let rc = sqlite3_exec(db, ptr::null(), None, ptr::null_mut(), ptr::null_mut());
            assert_eq!(rc, SQLITE_MISUSE);
            sqlite3_close(db);
        }
    }

    #[test]
    fn exec_invalid_sql_maps_error() {
        unsafe {
            let db = open_mem();
            let sql = CString::new("THIS IS NOT VALID SQL").unwrap();
            let rc = sqlite3_exec(db, sql.as_ptr(), None, ptr::null_mut(), ptr::null_mut());
            assert_ne!(rc, SQLITE_OK);
            sqlite3_close(db);
        }
    }

    #[test]
    fn exec_create_and_insert_succeeds() {
        unsafe {
            let db = open_mem();
            exec_ok(db, "CREATE TABLE t (id INTEGER, name TEXT)");
            exec_ok(db, "INSERT INTO t VALUES (1, 'alice')");
            sqlite3_close(db);
        }
    }

    // ── sqlite3_prepare_v2 ────────────────────────────────────────────────────

    #[test]
    fn prepare_null_db_is_misuse() {
        unsafe {
            let sql = CString::new("SELECT 1").unwrap();
            let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
            let rc = sqlite3_prepare_v2(
                ptr::null_mut(),
                sql.as_ptr(),
                -1,
                &mut stmt,
                ptr::null_mut(),
            );
            assert_eq!(rc, SQLITE_MISUSE);
        }
    }

    #[test]
    fn prepare_null_sql_is_misuse() {
        unsafe {
            let db = open_mem();
            let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
            let rc = sqlite3_prepare_v2(db, ptr::null(), -1, &mut stmt, ptr::null_mut());
            assert_eq!(rc, SQLITE_MISUSE);
            sqlite3_close(db);
        }
    }

    #[test]
    fn prepare_null_pp_stmt_is_misuse() {
        unsafe {
            let db = open_mem();
            let sql = CString::new("SELECT 1").unwrap();
            let rc = sqlite3_prepare_v2(db, sql.as_ptr(), -1, ptr::null_mut(), ptr::null_mut());
            assert_eq!(rc, SQLITE_MISUSE);
            sqlite3_close(db);
        }
    }

    #[test]
    fn prepare_invalid_sql_maps_error_and_leaves_stmt_null() {
        unsafe {
            let db = open_mem();
            let sql = CString::new("NOT VALID SQL AT ALL").unwrap();
            let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
            let rc = sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut());
            assert_ne!(rc, SQLITE_OK);
            assert!(stmt.is_null());
            sqlite3_close(db);
        }
    }

    // ── sqlite3_step / sqlite3_reset / sqlite3_finalize ───────────────────────

    #[test]
    fn step_null_stmt_is_misuse() {
        unsafe {
            assert_eq!(sqlite3_step(ptr::null_mut()), SQLITE_MISUSE);
        }
    }

    #[test]
    fn step_runtime_error_is_mapped() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 1/0");
            let rc = sqlite3_step(stmt);
            assert_ne!(rc, SQLITE_ROW);
            assert_ne!(rc, SQLITE_DONE);
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn reset_null_stmt_is_misuse() {
        unsafe {
            assert_eq!(sqlite3_reset(ptr::null_mut()), SQLITE_MISUSE);
        }
    }

    #[test]
    fn reset_allows_statement_reuse() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 42");

            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_int64(stmt, 0), 42);
            assert_eq!(sqlite3_step(stmt), SQLITE_DONE);

            assert_eq!(sqlite3_reset(stmt), SQLITE_OK);

            // After reset, the statement can be stepped again from the start.
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_int64(stmt, 0), 42);
            assert_eq!(sqlite3_step(stmt), SQLITE_DONE);

            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn finalize_null_stmt_is_a_noop_ok() {
        unsafe {
            assert_eq!(sqlite3_finalize(ptr::null_mut()), SQLITE_OK);
        }
    }

    // ── sqlite3_column_* ──────────────────────────────────────────────────────

    #[test]
    fn column_int64_null_stmt_is_zero() {
        unsafe {
            assert_eq!(sqlite3_column_int64(ptr::null_mut(), 0), 0);
        }
    }

    #[test]
    fn column_int64_from_real_truncates() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 3.9");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_int64(stmt, 0), 3);
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_int64_from_text_falls_back_to_zero() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 'hi'");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_int64(stmt, 0), 0);
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_double_null_stmt_is_zero() {
        unsafe {
            assert_eq!(sqlite3_column_double(ptr::null_mut(), 0), 0.0);
        }
    }

    #[test]
    fn column_double_from_real() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 2.5");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_double(stmt, 0), 2.5);
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_double_from_int_converts() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 7");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_double(stmt, 0), 7.0);
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_double_from_text_falls_back_to_zero() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 'hi'");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_double(stmt, 0), 0.0);
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_text_null_stmt_is_null_ptr() {
        unsafe {
            assert!(sqlite3_column_text(ptr::null_mut(), 0).is_null());
        }
    }

    #[test]
    fn column_text_from_text_value() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 'hello'");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            let ptr = sqlite3_column_text(stmt, 0);
            assert!(!ptr.is_null());
            let s = CStr::from_ptr(ptr).to_str().unwrap();
            assert_eq!(s, "hello");
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_text_with_interior_nul_is_null_ptr() {
        // A Rust `&str`/`Value::Text` can contain an embedded NUL byte (the
        // C API's own SQL text can't, since it's read via `CStr::from_ptr`,
        // but the underlying `Connection` takes a plain `&str`). When such
        // a value is fetched through `sqlite3_column_text`, `CString::new`
        // fails and the FFI layer must return a null pointer instead of
        // panicking or truncating silently.
        unsafe {
            let db = open_mem();
            let conn = &(*db).0;
            conn.execute("CREATE TABLE t (x)", [] as [(); 0]).unwrap();
            conn.execute("INSERT INTO t VALUES ('ab\0cd')", [] as [(); 0])
                .unwrap();

            let stmt = prepare(db, "SELECT x FROM t");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert!(sqlite3_column_text(stmt, 0).is_null());
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_text_is_cached_across_calls() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 'hello'");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            let ptr1 = sqlite3_column_text(stmt, 0);
            let ptr2 = sqlite3_column_text(stmt, 0);
            assert_eq!(ptr1, ptr2, "second call should hit the text cache");
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_text_from_int_value() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 123");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            let ptr = sqlite3_column_text(stmt, 0);
            assert!(!ptr.is_null());
            assert_eq!(CStr::from_ptr(ptr).to_str().unwrap(), "123");
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_text_from_real_value() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 4.5");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            let ptr = sqlite3_column_text(stmt, 0);
            assert!(!ptr.is_null());
            assert_eq!(CStr::from_ptr(ptr).to_str().unwrap(), "4.5");
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_text_from_null_value_is_null_ptr() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT NULL");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            let ptr = sqlite3_column_text(stmt, 0);
            assert!(ptr.is_null());
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_type_null_stmt_is_zero() {
        unsafe {
            assert_eq!(sqlite3_column_type(ptr::null_mut(), 0), 0);
        }
    }

    #[test]
    fn column_type_reports_each_kind() {
        unsafe {
            let db = open_mem();

            let stmt = prepare(db, "SELECT 1");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_type(stmt, 0), SQLITE_INTEGER);
            sqlite3_finalize(stmt);

            let stmt = prepare(db, "SELECT 1.5");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_type(stmt, 0), SQLITE_FLOAT);
            sqlite3_finalize(stmt);

            let stmt = prepare(db, "SELECT 'x'");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_type(stmt, 0), SQLITE_TEXT);
            sqlite3_finalize(stmt);

            let stmt = prepare(db, "SELECT zeroblob(3)");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_type(stmt, 0), SQLITE_BLOB);
            assert_eq!(sqlite3_column_bytes(stmt, 0), 3);
            sqlite3_finalize(stmt);

            let stmt = prepare(db, "SELECT NULL");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_type(stmt, 0), SQLITE_NULL);
            // Null has no byte length: sqlite3_column_bytes' fallback arm.
            assert_eq!(sqlite3_column_bytes(stmt, 0), 0);
            sqlite3_finalize(stmt);

            sqlite3_close(db);
        }
    }

    #[test]
    fn column_type_out_of_range_is_zero() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 1");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            // Column index 5 doesn't exist on a single-column result set.
            assert_eq!(sqlite3_column_type(stmt, 5), 0);
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_bytes_null_stmt_is_zero() {
        unsafe {
            assert_eq!(sqlite3_column_bytes(ptr::null_mut(), 0), 0);
        }
    }

    #[test]
    fn column_bytes_of_text() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 'hello'");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_bytes(stmt, 0), 5);
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_bytes_of_numeric_returns_text_len() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 1");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            // After coercion, "1" is 1 byte
            assert_eq!(sqlite3_column_bytes(stmt, 0), 1);
            sqlite3_finalize(stmt);

            let stmt = prepare(db, "SELECT 42");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_bytes(stmt, 0), 2);
            sqlite3_finalize(stmt);

            let stmt = prepare(db, "SELECT 1.5");
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert!(sqlite3_column_bytes(stmt, 0) > 0);
            sqlite3_finalize(stmt);

            sqlite3_close(db);
        }
    }

    #[test]
    fn column_count_null_stmt_is_zero() {
        unsafe {
            assert_eq!(sqlite3_column_count(ptr::null_mut()), 0);
        }
    }

    #[test]
    fn column_count_matches_before_and_after_step() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 1, 2, 3");
            assert_eq!(sqlite3_column_count(stmt), 3);
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_count(stmt), 3);
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    #[test]
    fn column_name_returns_empty_string() {
        unsafe {
            let db = open_mem();
            let stmt = prepare(db, "SELECT 1");
            let name = sqlite3_column_name(stmt, 0);
            assert!(!name.is_null());
            assert_eq!(CStr::from_ptr(name).to_str().unwrap(), "");
            sqlite3_finalize(stmt);
            sqlite3_close(db);
        }
    }

    // ── misc metadata / stub functions ───────────────────────────────────────

    #[test]
    fn errmsg_returns_placeholder() {
        unsafe {
            let msg = sqlite3_errmsg(ptr::null_mut());
            assert_eq!(CStr::from_ptr(msg).to_str().unwrap(), "not implemented");
        }
    }

    #[test]
    fn changes_stub_returns_zero() {
        unsafe {
            assert_eq!(sqlite3_changes(ptr::null_mut()), 0);
        }
    }

    #[test]
    fn last_insert_rowid_stub_returns_zero() {
        unsafe {
            assert_eq!(sqlite3_last_insert_rowid(ptr::null_mut()), 0);
        }
    }

    #[test]
    fn bind_parameter_count_stub_returns_zero() {
        unsafe {
            assert_eq!(sqlite3_bind_parameter_count(ptr::null_mut()), 0);
        }
    }

    #[test]
    fn bind_functions_are_unimplemented_stubs() {
        unsafe {
            assert_eq!(sqlite3_bind_int64(ptr::null_mut(), 1, 42), SQLITE_ERROR);
            assert_eq!(sqlite3_bind_double(ptr::null_mut(), 1, 4.2), SQLITE_ERROR);
            assert_eq!(sqlite3_bind_null(ptr::null_mut(), 1), SQLITE_ERROR);

            let text = CString::new("hi").unwrap();
            assert_eq!(
                sqlite3_bind_text(ptr::null_mut(), 1, text.as_ptr(), 2, None),
                SQLITE_ERROR
            );

            let blob = [1u8, 2, 3];
            assert_eq!(
                sqlite3_bind_blob(ptr::null_mut(), 1, blob.as_ptr() as *const c_void, 3, None),
                SQLITE_ERROR
            );
        }
    }

    #[test]
    fn libversion_matches_expected_string_and_number() {
        unsafe {
            let v = sqlite3_libversion();
            assert_eq!(CStr::from_ptr(v).to_str().unwrap(), "3.53.0");
            assert_eq!(sqlite3_libversion_number(), 3_053_000);
        }
    }

    #[test]
    fn free_on_null_is_a_noop() {
        unsafe {
            sqlite3_free(ptr::null_mut());
        }
    }
}
