//! C ABI compatibility layer for SQLite3-rs.
//!
//! Exports `#[no_mangle] extern "C"` symbols matching `sqlite3.h`, enabling
//! this library to be used as a drop-in replacement for `libsqlite3.so`.
//!
//! ## Status
//! Phase 4 — skeleton with SQLITE_* constants and `sqlite3_open` / `sqlite3_close`
//! stubs. Full implementation follows once the Rust API in `sqlite3` crate is stable.

use std::ffi::{CStr, c_char, c_int, c_void};
use sqlite3_crate::Connection;

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

/// Opaque database connection handle (`sqlite3*` in C).
pub struct sqlite3(Connection);

/// Opaque prepared statement handle (`sqlite3_stmt*` in C).
pub struct sqlite3_stmt; // TODO Phase 4

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
pub unsafe extern "C" fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char {
    // TODO: return the last error message for `db`.
    b"not implemented\0".as_ptr() as *const c_char
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_libversion() -> *const c_char {
    b"3.53.0\0".as_ptr() as *const c_char
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_libversion_number() -> c_int {
    3_053_000
}
