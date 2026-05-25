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
///
/// Each allocation is prefixed by an 8-byte header containing the usable size,
/// which lets `free`, `realloc`, and `size` recover the `Layout` without any
/// external bookkeeping.
///
/// Memory layout of each allocation:
/// ```text
/// [ usize (8 bytes) | ... n bytes of usable data ... ]
///  ^raw              ^returned pointer
/// ```
pub struct SystemAlloc;

/// Alignment used for all `SystemAlloc` allocations.
const ALIGN: usize = 8;
/// Size of the bookkeeping header prepended to every allocation.
const HEADER: usize = std::mem::size_of::<usize>(); // 8 bytes on 64-bit

unsafe impl SqliteAlloc for SystemAlloc {
    fn malloc(&self, n: usize) -> *mut u8 {
        if n == 0 {
            return core::ptr::null_mut();
        }
        let total = n + HEADER;
        // SAFETY: total > 0 and ALIGN is a valid power-of-two alignment.
        let layout = match std::alloc::Layout::from_size_align(total, ALIGN) {
            Ok(l) => l,
            Err(_) => return core::ptr::null_mut(),
        };
        unsafe {
            let raw = std::alloc::alloc(layout);
            if raw.is_null() {
                return raw;
            }
            // Store usable size in header.
            (raw as *mut usize).write(n);
            raw.add(HEADER)
        }
    }

    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn free(&self, ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        unsafe {
            // SAFETY: ptr was obtained from SystemAlloc::malloc; the header
            // immediately precedes it and contains the original usable size.
            let raw = ptr.sub(HEADER);
            let n = (raw as *const usize).read();
            let layout = std::alloc::Layout::from_size_align_unchecked(n + HEADER, ALIGN);
            std::alloc::dealloc(raw, layout);
        }
    }

    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn realloc(&self, ptr: *mut u8, n: usize) -> *mut u8 {
        if ptr.is_null() {
            return self.malloc(n);
        }
        if n == 0 {
            self.free(ptr);
            return core::ptr::null_mut();
        }
        unsafe {
            // SAFETY: same invariant as free.
            let raw = ptr.sub(HEADER);
            let old_n = (raw as *const usize).read();
            let old_layout = std::alloc::Layout::from_size_align_unchecked(old_n + HEADER, ALIGN);
            let new_total = n + HEADER;
            let new_raw = std::alloc::realloc(raw, old_layout, new_total);
            if new_raw.is_null() {
                return new_raw;
            }
            (new_raw as *mut usize).write(n);
            new_raw.add(HEADER)
        }
    }

    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    fn size(&self, ptr: *mut u8) -> usize {
        if ptr.is_null() {
            return 0;
        }
        // SAFETY: header immediately precedes ptr.
        unsafe { (ptr.sub(HEADER) as *const usize).read() }
    }

    fn roundup(&self, n: usize) -> usize {
        (n + ALIGN - 1) & !(ALIGN - 1)
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

impl<const N: usize> Default for StaticAlloc<N> {
    fn default() -> Self {
        Self::new()
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

/// A wrapper allocator that tracks allocation counts and bytes.
/// Also supports simulating OOM failures on the Nth allocation.
pub struct CountingAlloc<A> {
    inner: A,
    pub count: std::sync::atomic::AtomicUsize,
    pub bytes: std::sync::atomic::AtomicUsize,
    pub oom_inject_at: std::sync::atomic::AtomicUsize,
}

impl<A> CountingAlloc<A> {
    pub const fn new(inner: A) -> Self {
        Self {
            inner,
            count: std::sync::atomic::AtomicUsize::new(0),
            bytes: std::sync::atomic::AtomicUsize::new(0),
            oom_inject_at: std::sync::atomic::AtomicUsize::new(usize::MAX),
        }
    }

    pub fn reset(&self) {
        self.count.store(0, std::sync::atomic::Ordering::SeqCst);
        self.bytes.store(0, std::sync::atomic::Ordering::SeqCst);
        self.oom_inject_at
            .store(usize::MAX, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn set_oom_inject(&self, at: usize) {
        self.oom_inject_at
            .store(at, std::sync::atomic::Ordering::SeqCst);
    }
}

unsafe impl<A: SqliteAlloc> SqliteAlloc for CountingAlloc<A> {
    fn malloc(&self, n: usize) -> *mut u8 {
        let current = self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if current >= self.oom_inject_at.load(std::sync::atomic::Ordering::SeqCst) {
            return core::ptr::null_mut();
        }
        let ptr = self.inner.malloc(n);
        if !ptr.is_null() {
            self.bytes
                .fetch_add(self.inner.size(ptr), std::sync::atomic::Ordering::SeqCst);
        }
        ptr
    }

    fn free(&self, ptr: *mut u8) {
        if !ptr.is_null() {
            let size = self.inner.size(ptr);
            self.bytes
                .fetch_sub(size, std::sync::atomic::Ordering::SeqCst);
            self.inner.free(ptr);
        }
    }

    fn realloc(&self, ptr: *mut u8, n: usize) -> *mut u8 {
        if ptr.is_null() {
            return self.malloc(n);
        }
        if n == 0 {
            self.free(ptr);
            return core::ptr::null_mut();
        }
        let current = self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if current >= self.oom_inject_at.load(std::sync::atomic::Ordering::SeqCst) {
            return core::ptr::null_mut();
        }

        let old_size = self.inner.size(ptr);
        let new_ptr = self.inner.realloc(ptr, n);
        if !new_ptr.is_null() {
            let new_size = self.inner.size(new_ptr);
            self.bytes
                .fetch_add(new_size, std::sync::atomic::Ordering::SeqCst);
            self.bytes
                .fetch_sub(old_size, std::sync::atomic::Ordering::SeqCst);
        }
        new_ptr
    }

    fn size(&self, ptr: *mut u8) -> usize {
        self.inner.size(ptr)
    }

    fn roundup(&self, n: usize) -> usize {
        self.inner.roundup(n)
    }

    fn init(&self) -> Result<(), AllocError> {
        self.inner.init()
    }

    fn shutdown(&self) {
        self.inner.shutdown()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_alloc_malloc_free() {
        let a = SystemAlloc;
        let p = a.malloc(64);
        assert!(!p.is_null());
        assert_eq!(a.size(p), 64);
        a.free(p);
    }

    #[test]
    fn system_alloc_realloc() {
        let a = SystemAlloc;
        let p = a.malloc(32);
        assert!(!p.is_null());
        let p2 = a.realloc(p, 128);
        assert!(!p2.is_null());
        assert_eq!(a.size(p2), 128);
        a.free(p2);
    }

    #[test]
    fn system_alloc_roundup() {
        let a = SystemAlloc;
        assert_eq!(a.roundup(1), 8);
        assert_eq!(a.roundup(8), 8);
        assert_eq!(a.roundup(9), 16);
    }

    #[test]
    fn system_alloc_null_cases() {
        let a = SystemAlloc;
        assert!(a.malloc(0).is_null());
        a.free(core::ptr::null_mut()); // must not panic
        assert_eq!(a.size(core::ptr::null_mut()), 0);
        // realloc(null, n) == malloc(n)
        let p = a.realloc(core::ptr::null_mut(), 16);
        assert!(!p.is_null());
        a.free(p);
    }

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

    #[test]
    fn counting_alloc_oom_inject() {
        let alloc = CountingAlloc::new(SystemAlloc);
        alloc.set_oom_inject(1); // 0th succeeds, 1st fails

        let p1 = alloc.malloc(16);
        assert!(!p1.is_null());
        assert_eq!(alloc.count.load(std::sync::atomic::Ordering::SeqCst), 1);

        let p2 = alloc.malloc(16);
        assert!(p2.is_null());
        assert_eq!(alloc.count.load(std::sync::atomic::Ordering::SeqCst), 2);

        alloc.free(p1);
    }
}
