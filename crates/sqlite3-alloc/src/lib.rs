//! Memory allocation traits and implementations.
//!
//! Mirrors `sqlite3_mem_methods` from the C library. Provides a pluggable
//! allocator interface with a default implementation that delegates to the
//! global Rust allocator.

use std::cell::UnsafeCell;

/// Errors that can occur during allocation.
#[derive(Debug, thiserror::Error)]
pub enum AllocError {
    #[error("out of memory")]
    OutOfMemory,
    #[error("initialization failed")]
    InitFailed,
}

/// Pluggable allocator trait mirroring `sqlite3_mem_methods`.
///
/// # Safety
/// Implementors must ensure allocations are valid for the declared sizes and
/// that freed pointers are never used after `free()` is called.
pub unsafe trait SqliteAlloc: Send + Sync {
    fn malloc(&self, n: usize) -> *mut u8;
    fn free(&self, ptr: *mut u8);
    fn realloc(&self, ptr: *mut u8, n: usize) -> *mut u8;
    /// Returns the usable size of the allocation at `ptr`.
    fn size(&self, ptr: *mut u8) -> usize;
    /// Round `n` up to the next allocation granularity.
    fn roundup(&self, n: usize) -> usize;
    fn init(&self) -> Result<(), AllocError>;
    fn shutdown(&self);
}

/// Default allocator that delegates to the global Rust allocator.
pub struct SystemAlloc;

unsafe impl SqliteAlloc for SystemAlloc {
    fn malloc(&self, n: usize) -> *mut u8 {
        if n == 0 {
            return core::ptr::null_mut();
        }
        // SAFETY: layout is valid because n > 0 and align is 1.
        let layout = std::alloc::Layout::from_size_align(n, 8).expect("invalid layout");
        unsafe { std::alloc::alloc(layout) }
    }

    fn free(&self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        // SAFETY: caller must have obtained ptr from malloc/realloc with the
        // same allocator. We store the size in the 8-byte header — but for the
        // system allocator we cannot recover layout without storing it.
        // This stub panics to flag the unimplemented path.
        unimplemented!("SystemAlloc::free requires size tracking — use the tracking wrapper")
    }

    fn realloc(&self, _ptr: *mut u8, _n: usize) -> *mut u8 {
        unimplemented!("SystemAlloc::realloc requires size tracking — use the tracking wrapper")
    }

    fn size(&self, _ptr: *mut u8) -> usize {
        unimplemented!("SystemAlloc::size requires size tracking")
    }

    fn roundup(&self, n: usize) -> usize {
        // Round up to 8-byte boundary.
        (n + 7) & !7
    }

    fn init(&self) -> Result<(), AllocError> {
        Ok(())
    }

    fn shutdown(&self) {}
}

/// Static-buffer allocator for embedded / no-std targets.
///
/// Uses a simple bump-pointer strategy backed by a fixed array.
pub struct StaticAlloc<const N: usize> {
    buf: UnsafeCell<[u8; N]>,
    top: std::sync::atomic::AtomicUsize,
}

// SAFETY: Internal mutation is guarded by the atomic `top` pointer.
unsafe impl<const N: usize> Sync for StaticAlloc<N> {}

impl<const N: usize> StaticAlloc<N> {
    pub const fn new() -> Self {
        Self {
            buf: UnsafeCell::new([0u8; N]),
            top: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

unsafe impl<const N: usize> SqliteAlloc for StaticAlloc<N> {
    fn malloc(&self, n: usize) -> *mut u8 {
        use std::sync::atomic::Ordering;
        let n = self.roundup(n);
        let old = self.top.fetch_add(n, Ordering::SeqCst);
        if old + n > N {
            self.top.fetch_sub(n, Ordering::SeqCst);
            return core::ptr::null_mut();
        }
        // SAFETY: `old` is within the buffer bounds as verified above.
        unsafe { (self.buf.get() as *mut u8).add(old) }
    }

    fn free(&self, _ptr: *mut u8) {
        // Bump allocators do not support individual frees.
    }

    fn realloc(&self, _ptr: *mut u8, n: usize) -> *mut u8 {
        // Simplified: allocate new, caller must copy.
        self.malloc(n)
    }

    fn size(&self, _ptr: *mut u8) -> usize {
        0
    }

    fn roundup(&self, n: usize) -> usize {
        (n + 7) & !7
    }

    fn init(&self) -> Result<(), AllocError> {
        Ok(())
    }

    fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_alloc_basic() {
        let alloc = StaticAlloc::<1024>::new();
        let p = alloc.malloc(16);
        assert!(!p.is_null());
    }

    #[test]
    fn static_alloc_oom() {
        let alloc = StaticAlloc::<8>::new();
        let p = alloc.malloc(16);
        assert!(p.is_null());
    }
}
