//! Windows VFS implementation for Liter-rs.
//!
//! Mirrors `os_win.c`. Uses `LockFileEx` / `UnlockFile` for byte-range
//! locking semantics compatible with C SQLite on Windows.
//!
//! ## Lock byte layout (matches SQLite exactly)
//! ```text
//! PENDING_BYTE   = 0x40000000  (byte 1073741824)
//! RESERVED_BYTE  = 0x40000001
//! SHARED_FIRST   = 0x40000002
//! SHARED_SIZE    = 510
//! ```

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use liter_vfs::{
    AccessFlags, DeviceCharacteristics, LockLevel, OpenFlags, SyncFlags, Vfs, VfsFile,
};

/// SQLite lock-byte region offsets (must match C SQLite exactly).
#[allow(dead_code)]
const PENDING_BYTE: u64 = 0x40000000;
#[allow(dead_code)]
const RESERVED_BYTE: u64 = PENDING_BYTE + 1;
#[allow(dead_code)]
const SHARED_FIRST: u64 = PENDING_BYTE + 2;
#[allow(dead_code)]
const SHARED_SIZE: u64 = 510;

pub struct WinFile {
    file: File,
    lock: LockLevel,
}

impl VfsFile for WinFile {
    fn read(&mut self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read(buf)
    }

    fn write(&mut self, buf: &[u8], offset: u64) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(buf)
    }

    fn truncate(&mut self, size: u64) -> io::Result<()> {
        self.file.set_len(size)
    }

    fn sync(&mut self, _flags: SyncFlags) -> io::Result<()> {
        self.file.sync_all()
    }

    fn file_size(&self) -> io::Result<u64> {
        self.file.metadata().map(|m| m.len())
    }

    fn lock(&mut self, level: LockLevel) -> io::Result<()> {
        if level <= self.lock {
            return Ok(());
        }
        platform::acquire_lock(&self.file, level, self.lock)?;
        self.lock = level;
        Ok(())
    }

    fn unlock(&mut self, level: LockLevel) -> io::Result<()> {
        if level >= self.lock {
            return Ok(());
        }
        platform::release_lock(&self.file, level, self.lock)?;
        self.lock = level;
        Ok(())
    }

    fn check_reserved_lock(&self) -> io::Result<bool> {
        platform::check_reserved_lock(&self.file)
    }

    fn device_characteristics(&self) -> DeviceCharacteristics {
        DeviceCharacteristics::UNDELETABLE_WHEN_OPEN
    }

    fn sector_size(&self) -> u32 {
        4096
    }
}

// ── Platform-specific locking ─────────────────────────────────────────────────

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        LockFileEx, UnlockFile, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows::Win32::System::IO::OVERLAPPED;

    fn make_overlapped(offset: u64) -> OVERLAPPED {
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.Anonymous.Anonymous.Offset = offset as u32;
        ov.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        ov
    }

    fn shared_lock_range(handle: HANDLE, offset: u64, length: u64) -> io::Result<()> {
        let mut ov = make_overlapped(offset);
        // Shared: no LOCKFILE_EXCLUSIVE_LOCK flag, no LOCKFILE_FAIL_IMMEDIATELY.
        // SAFETY: handle is valid.
        let result = unsafe {
            LockFileEx(
                handle,
                LOCKFILE_FAIL_IMMEDIATELY,
                0,
                length as u32,
                (length >> 32) as u32,
                &mut ov,
            )
        };
        if result.is_ok() {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn exclusive_lock_range(handle: HANDLE, offset: u64, length: u64) -> io::Result<()> {
        let mut ov = make_overlapped(offset);
        // SAFETY: handle is valid.
        let result = unsafe {
            LockFileEx(
                handle,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                length as u32,
                (length >> 32) as u32,
                &mut ov,
            )
        };
        if result.is_ok() {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn unlock_range(handle: HANDLE, offset: u64, length: u64) -> io::Result<()> {
        // SAFETY: handle is valid.
        let result = unsafe {
            UnlockFile(
                handle,
                offset as u32,
                (offset >> 32) as u32,
                length as u32,
                (length >> 32) as u32,
            )
        };
        if result.is_ok() {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn acquire_lock(file: &File, target: LockLevel, current: LockLevel) -> io::Result<()> {
        let handle = HANDLE(file.as_raw_handle() as *mut core::ffi::c_void);
        match target {
            LockLevel::Shared if current < LockLevel::Shared => {
                shared_lock_range(handle, SHARED_FIRST, 1)
            }
            LockLevel::Reserved if current < LockLevel::Reserved => {
                exclusive_lock_range(handle, RESERVED_BYTE, 1)
            }
            LockLevel::Pending if current < LockLevel::Pending => {
                exclusive_lock_range(handle, PENDING_BYTE, 1)
            }
            LockLevel::Exclusive if current < LockLevel::Exclusive => {
                // Acquire the pending byte first (if not already held), then upgrade.
                if current < LockLevel::Pending {
                    exclusive_lock_range(handle, PENDING_BYTE, 1)?;
                }
                // Unlock the one shared byte we held, then lock the full shared range exclusively.
                if current >= LockLevel::Shared {
                    unlock_range(handle, SHARED_FIRST, 1)?;
                }
                exclusive_lock_range(handle, SHARED_FIRST, SHARED_SIZE)
            }
            _ => Ok(()), // already at or above target
        }
    }

    pub fn release_lock(file: &File, target: LockLevel, current: LockLevel) -> io::Result<()> {
        let handle = HANDLE(file.as_raw_handle() as *mut core::ffi::c_void);
        if current >= LockLevel::Exclusive && target < LockLevel::Exclusive {
            unlock_range(handle, SHARED_FIRST, SHARED_SIZE)?;
            if target >= LockLevel::Shared {
                // Re-acquire a single shared byte.
                shared_lock_range(handle, SHARED_FIRST, 1)?;
            }
        }
        if current >= LockLevel::Pending && target < LockLevel::Pending {
            unlock_range(handle, PENDING_BYTE, 1)?;
        }
        if current >= LockLevel::Reserved && target < LockLevel::Reserved {
            unlock_range(handle, RESERVED_BYTE, 1)?;
        }
        if current >= LockLevel::Shared && target < LockLevel::Shared {
            unlock_range(handle, SHARED_FIRST, 1)?;
        }
        Ok(())
    }

    pub fn check_reserved_lock(file: &File) -> io::Result<bool> {
        let handle = HANDLE(file.as_raw_handle() as *mut core::ffi::c_void);
        // Try a non-blocking exclusive lock on the reserved byte.
        // If it succeeds, nobody holds it → release and return false.
        // If it fails, someone holds it → return true.
        match exclusive_lock_range(handle, RESERVED_BYTE, 1) {
            Ok(()) => {
                unlock_range(handle, RESERVED_BYTE, 1)?;
                Ok(false)
            }
            Err(_) => Ok(true),
        }
    }
}

// ── Non-Windows stub ──────────────────────────────────────────────────────────

/// On non-Windows platforms the locking calls are no-ops (the VFS is only
/// intended for Windows runtime use, but the crate must compile everywhere
/// for workspace coherence).
#[cfg(not(windows))]
mod platform {
    use super::*;

    pub fn acquire_lock(_file: &File, _target: LockLevel, _current: LockLevel) -> io::Result<()> {
        Ok(())
    }
    pub fn release_lock(_file: &File, _target: LockLevel, _current: LockLevel) -> io::Result<()> {
        Ok(())
    }
    pub fn check_reserved_lock(_file: &File) -> io::Result<bool> {
        Ok(false)
    }
}

// ── WinVfs ────────────────────────────────────────────────────────────────────

pub struct WinVfs;

impl Vfs for WinVfs {
    type File = WinFile;

    fn open(&self, path: &Path, flags: OpenFlags) -> io::Result<Self::File> {
        let file = OpenOptions::new()
            .read(true)
            .write(!flags.contains(OpenFlags::READ_ONLY))
            .create(flags.contains(OpenFlags::CREATE))
            .open(path)?;
        Ok(WinFile {
            file,
            lock: LockLevel::None,
        })
    }

    fn delete(&self, path: &Path, _sync_dir: bool) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn access(&self, path: &Path, _flags: AccessFlags) -> io::Result<bool> {
        Ok(path.exists())
    }

    fn full_pathname(&self, path: &Path) -> io::Result<PathBuf> {
        path.canonicalize()
    }

    fn randomness(&self, buf: &mut [u8]) {
        // Simple fallback using current time seed.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let seed = t.to_le_bytes();
        for (i, b) in buf.iter_mut().enumerate() {
            *b = seed[i % seed.len()].wrapping_add(i as u8);
        }
    }

    fn sleep(&self, micros: u64) {
        std::thread::sleep(std::time::Duration::from_micros(micros));
    }

    fn current_time(&self) -> f64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        secs / 86400.0 + 2440587.5
    }

    fn name(&self) -> &str {
        "win32"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_constants() {
        assert_eq!(PENDING_BYTE, 0x40000000);
        assert_eq!(RESERVED_BYTE, 0x40000001);
        assert_eq!(SHARED_FIRST, 0x40000002);
        assert_eq!(SHARED_SIZE, 510);
    }

    #[test]
    fn test_vfs_lifecycle_and_file_operations() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_win_vfs.db");
        let vfs = WinVfs;

        assert_eq!(vfs.name(), "win32");
        assert!(!vfs.access(&path, AccessFlags::EXISTS).unwrap());

        let mut file = vfs
            .open(&path, OpenFlags::CREATE | OpenFlags::READ_WRITE)
            .unwrap();

        assert!(vfs.access(&path, AccessFlags::EXISTS).unwrap());
        assert_eq!(
            file.device_characteristics(),
            DeviceCharacteristics::UNDELETABLE_WHEN_OPEN
        );
        assert_eq!(file.sector_size(), 4096);

        // Write and read
        file.write(b"hello world", 0).unwrap();
        assert_eq!(file.file_size().unwrap(), 11);

        let mut buf = [0u8; 5];
        let n = file.read(&mut buf, 0).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, b"hello");

        // Write at offset
        file.write(b"rust", 6).unwrap();
        let mut full_buf = [0u8; 11];
        file.read(&mut full_buf, 0).unwrap();
        assert_eq!(&full_buf, b"hello rustd");

        // Truncate
        file.truncate(5).unwrap();
        assert_eq!(file.file_size().unwrap(), 5);

        // Sync
        file.sync(SyncFlags::NORMAL).unwrap();
        file.sync(SyncFlags::FULL).unwrap();

        // Lock transitions
        assert!(!file.check_reserved_lock().unwrap());

        // Escalating locks
        file.lock(LockLevel::Shared).unwrap();
        file.lock(LockLevel::Shared).unwrap(); // no-op
        file.lock(LockLevel::Reserved).unwrap();
        file.lock(LockLevel::Pending).unwrap();
        file.lock(LockLevel::Exclusive).unwrap();
        file.lock(LockLevel::Exclusive).unwrap(); // no-op

        // Releasing locks
        file.unlock(LockLevel::Exclusive).unwrap(); // no-op
        file.unlock(LockLevel::Pending).unwrap();
        file.unlock(LockLevel::Reserved).unwrap();
        file.unlock(LockLevel::Shared).unwrap();
        file.unlock(LockLevel::None).unwrap();
        file.unlock(LockLevel::None).unwrap(); // no-op

        drop(file);

        // Read-only open
        let mut ro_file = vfs.open(&path, OpenFlags::READ_ONLY).unwrap();
        let mut ro_buf = [0u8; 5];
        assert_eq!(ro_file.read(&mut ro_buf, 0).unwrap(), 5);
        assert_eq!(&ro_buf, b"hello");
        drop(ro_file);

        // Full pathname
        let full = vfs.full_pathname(&path).unwrap();
        assert!(full.is_absolute());

        // Delete
        vfs.delete(&path, false).unwrap();
        assert!(!vfs.access(&path, AccessFlags::EXISTS).unwrap());
    }

    #[test]
    fn test_vfs_randomness_sleep_and_time() {
        let vfs = WinVfs;

        let mut buf = [0u8; 32];
        vfs.randomness(&mut buf);
        assert!(buf.iter().any(|&b| b != 0));

        vfs.sleep(100);

        let t = vfs.current_time();
        assert!(t > 2440587.5);
    }

    #[cfg(not(windows))]
    #[test]
    fn test_non_windows_platform_stubs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("stub_test.db");
        let vfs = WinVfs;
        let file = vfs
            .open(&path, OpenFlags::CREATE | OpenFlags::READ_WRITE)
            .unwrap();

        assert!(platform::acquire_lock(&file.file, LockLevel::Shared, LockLevel::None).is_ok());
        assert!(platform::release_lock(&file.file, LockLevel::None, LockLevel::Shared).is_ok());
        assert!(!platform::check_reserved_lock(&file.file).unwrap());
    }
}
