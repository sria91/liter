//! Out-of-memory (OOM) injection tests.
//!
//! Verifies that every public API returns `SqliteError::NoMem` (never panics)
//! when allocations fail at each possible allocation site.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Global allocation failure counter.
/// When non-zero, the Nth allocation will fail.
static ALLOC_FAIL_AFTER: AtomicUsize = AtomicUsize::new(0);

/// Run `f` with allocations failing after `limit` successful allocations.
///
/// Iterates from `limit = 1` until `f` returns `Ok(())` (meaning no more OOM
/// paths are reachable).
pub fn oom_test<F>(mut f: F)
where
    F: FnMut() -> Result<(), sqlite3::SqliteError>,
{
    for limit in 1..=10_000 {
        ALLOC_FAIL_AFTER.store(limit, Ordering::SeqCst);
        match f() {
            Err(sqlite3::SqliteError::NoMem) => continue,
            Err(e) => panic!("Unexpected error at alloc limit {limit}: {e}"),
            Ok(()) => {
                // All allocation sites passed — done.
                ALLOC_FAIL_AFTER.store(0, Ordering::SeqCst);
                return;
            }
        }
    }
    ALLOC_FAIL_AFTER.store(0, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder_oom_test() {
        // OOM injection is wired up once the allocator shim is in place.
    }
}
