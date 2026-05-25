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

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use liter::{Connection, SqliteError, Statement, Value};

// ── SQLITE_* result codes ─────────────────────────────────────────────────────
pub const SQLITE_OK:         c_int = 0;
pub const SQLITE_ERROR:      c_int = 1;
pub const SQLITE_INTERNAL:   c_int = 2;
pub const SQLITE_PERM:       c_int = 3;
pub const SQLITE_ABORT:      c_int = 4;
pub const SQLITE_BUSY:       c_int = 5;
pub const SQLITE_LOCKED:     c_int = 6;
pub const SQLITE_NOMEM:      c_int = 7;
pub const SQLITE_READONLY:   c_int = 8;
pub const SQLITE_INTERRUPT:  c_int = 9;
pub const SQLITE_IOERR:      c_int = 10;
pub const SQLITE_CORRUPT:    c_int = 11;
pub const SQLITE_NOTFOUND:   c_int = 12;
pub const SQLITE_FULL:       c_int = 13;
pub const SQLITE_CANTOPEN:   c_int = 14;
pub const SQLITE_PROTOCOL:   c_int = 15;
pub const SQLITE_EMPTY:      c_int = 16;
pub const SQLITE_SCHEMA:     c_int = 17;
pub const SQLITE_TOOBIG:     c_int = 18;
pub const SQLITE_CONSTRAINT: c_int = 19;
pub const SQLITE_MISMATCH:   c_int = 20;
pub const SQLITE_MISUSE:     c_int = 21;
pub const SQLITE_NOLFS:      c_int = 22;
pub const SQLITE_AUTH:       c_int = 23;
pub const SQLITE_FORMAT:     c_int = 24;
pub const SQLITE_RANGE:      c_int = 25;
pub const SQLITE_NOTADB:     c_int = 26;
pub const SQLITE_NOTICE:     c_int = 27;
pub const SQLITE_WARNING:    c_int = 28;
pub const SQLITE_ROW:        c_int = 100;
pub const SQLITE_DONE:       c_int = 101;

// ── SQLITE_* data types ───────────────────────────────────────────────────────
pub const SQLITE_INTEGER:    c_int = 1;
pub const SQLITE_FLOAT:      c_int = 2;
pub const SQLITE_TEXT:       c_int = 3;
pub const SQLITE_BLOB:       c_int = 4;
pub const SQLITE_NULL:       c_int = 5;

/// Opaque database connection handle (`sqlite3*` in C).
#[allow(non_camel_case_types)]
pub struct sqlite3(Connection);

/// Opaque prepared statement handle (`sqlite3_stmt*` in C).
#[allow(non_camel_case_types)]
pub struct sqlite3_stmt {
    stmt: Statement<'static>,
    text_cache: std::collections::HashMap<usize, CString>,
}

pub type Sqlite3Callback = Option<
    unsafe extern "C" fn(
        *mut c_void,
        c_int,
        *mut *mut c_char,
        *mut *mut c_char,
    ) -> c_int,
>;

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
pub unsafe extern "C" fn sqlite3_open(
    filename: *const c_char,
    pp_db: *mut *mut sqlite3,
) -> c_int {
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
    if stmt.is_null() { return 0; }
    let stmt_ref = &*stmt;
    match stmt_ref.stmt.column_value(i_col as usize) {
        Ok(Value::Int(i)) => i,
        Ok(Value::Real(f)) => f as i64,
        _ => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_double(stmt: *mut sqlite3_stmt, i_col: c_int) -> f64 {
    if stmt.is_null() { return 0.0; }
    let stmt_ref = &*stmt;
    match stmt_ref.stmt.column_value(i_col as usize) {
        Ok(Value::Real(f)) => f,
        Ok(Value::Int(i)) => i as f64,
        _ => 0.0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_text(stmt: *mut sqlite3_stmt, i_col: c_int) -> *const c_char {
    if stmt.is_null() { return std::ptr::null(); }
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
    if stmt.is_null() { return 0; }
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
    if stmt.is_null() { return 0; }
    let stmt_ref = &*stmt;
    match stmt_ref.stmt.column_value(i_col as usize) {
        Ok(Value::Text(t)) => t.len() as c_int,
        Ok(Value::Blob(b)) => b.len() as c_int,
        _ => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_column_count(stmt: *mut sqlite3_stmt) -> c_int {
    if stmt.is_null() { return 0; }
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
pub unsafe extern "C" fn sqlite3_bind_int64(_stmt: *mut sqlite3_stmt, _i: c_int, _val: i64) -> c_int {
    SQLITE_ERROR
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_double(_stmt: *mut sqlite3_stmt, _i: c_int, _val: f64) -> c_int {
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
    _destroy: Option<unsafe extern "C" fn(*mut c_void)>
) -> c_int {
    SQLITE_ERROR
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_bind_blob(
    _stmt: *mut sqlite3_stmt,
    _i: c_int,
    _data: *const c_void,
    _n: c_int,
    _destroy: Option<unsafe extern "C" fn(*mut c_void)>
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
