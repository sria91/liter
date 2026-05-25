//! Mutex, condvar, and atomic wrappers for Liter-rs.
//!
//! Mirrors the `sqlite3_mutex` interface. Provides static and recursive mutex
//! types backed by `parking_lot`.

pub use parking_lot::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// A recursive mutex compatible with SQLite's mutex semantics.
/// SQLite frequently takes the same mutex from the same thread.
pub type RecursiveMutex<T> = parking_lot::ReentrantMutex<T>;

/// Unique mutex IDs mirroring SQLITE_MUTEX_* constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum MutexKind {
    Fast = 0,
    Recursive = 1,
    Static0 = 2,
    Static1 = 3,
    Static2 = 4,
    Static3 = 5,
    Static4 = 6,
    Static5 = 7,
    Static6 = 8,
    Static7 = 9,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutex_lock_unlock() {
        let m = Mutex::new(0u32);
        {
            let mut g = m.lock();
            *g = 42;
        }
        assert_eq!(*m.lock(), 42);
    }

    #[test]
    fn rwlock_concurrent_reads() {
        let rw = RwLock::new(99u32);
        let r1 = rw.read();
        let r2 = rw.read();
        assert_eq!(*r1, *r2);
    }
}
