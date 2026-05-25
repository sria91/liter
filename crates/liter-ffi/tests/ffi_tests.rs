#![allow(clippy::manual_c_str_literals)]

use liter_ffi::*;
use std::ffi::{CString, c_char};
use std::ptr;

#[test]
fn test_open_and_close() {
    unsafe {
        let mut db: *mut sqlite3 = ptr::null_mut();
        
        // Open an in-memory database
        let path = CString::new(":memory:").unwrap();
        let rc = sqlite3_open(path.as_ptr(), &mut db);
        
        assert_eq!(rc, SQLITE_OK);
        assert!(!db.is_null());
        
        // Close it
        let rc = sqlite3_close(db);
        assert_eq!(rc, SQLITE_OK);
    }
}

#[test]
fn test_exec_stub() {
    let mut db: *mut sqlite3 = ptr::null_mut();
    let rc = unsafe { sqlite3_open(b":memory:\0".as_ptr() as *const c_char, &mut db) };
    assert_eq!(rc, 0);

    let err_msg: *mut *mut c_char = ptr::null_mut();
    // Since sqlite3 execute() is now wired, SELECT 1 should succeed and return SQLITE_OK (0).
    let rc = unsafe {
        sqlite3_exec(
            db,
            b"SELECT 1;\0".as_ptr() as *const c_char,
            None,
            ptr::null_mut(),
            err_msg,
        )
    };
    assert_eq!(rc, 0);

    unsafe { sqlite3_close(db) };
}

#[test]
fn test_prepare_and_step() {
    unsafe {
        let mut db: *mut sqlite3 = ptr::null_mut();
        let path = CString::new(":memory:").unwrap();
        sqlite3_open(path.as_ptr(), &mut db);
        
        let sql = CString::new("SELECT 42;").unwrap();
        let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
        
        let rc = sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut());
        assert_eq!(rc, SQLITE_OK);
        assert!(!stmt.is_null());
        
        let rc = sqlite3_step(stmt);
        assert_eq!(rc, SQLITE_ROW);
        
        let val = sqlite3_column_int64(stmt, 0);
        assert_eq!(val, 42);
        
        let rc = sqlite3_step(stmt);
        assert_eq!(rc, SQLITE_DONE);
        
        sqlite3_finalize(stmt);
        sqlite3_close(db);
    }
}
