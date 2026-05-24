use sqlite3_ffi::*;
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
    unsafe {
        let mut db: *mut sqlite3 = ptr::null_mut();
        let path = CString::new(":memory:").unwrap();
        sqlite3_open(path.as_ptr(), &mut db);
        
        let sql = CString::new("SELECT 1;").unwrap();
        let mut errmsg: *mut c_char = ptr::null_mut();
        
        // sqlite3_crate currently returns NotImplemented for execute, which maps to SQLITE_ERROR
        let rc = sqlite3_exec(db, sql.as_ptr(), None, ptr::null_mut(), &mut errmsg);
        assert_eq!(rc, SQLITE_ERROR);
        
        sqlite3_close(db);
    }
}

#[test]
fn test_prepare_stub() {
    unsafe {
        let mut db: *mut sqlite3 = ptr::null_mut();
        let path = CString::new(":memory:").unwrap();
        sqlite3_open(path.as_ptr(), &mut db);
        
        let sql = CString::new("SELECT 1;").unwrap();
        let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
        
        // sqlite3_crate currently returns NotImplemented for prepare, which maps to SQLITE_ERROR
        let rc = sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut());
        assert_eq!(rc, SQLITE_ERROR);
        assert!(stmt.is_null()); // stmt shouldn't be allocated on error
        
        sqlite3_close(db);
    }
}
