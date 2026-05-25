//! Unix VFS implementation for Liter-rs.
//!
//! Mirrors `os_unix.c`. Uses POSIX advisory locks (`fcntl F_SETLK/F_GETLK`)
//! for file locking semantics compatible with the C SQLite implementation.
//!
//! ## Lock byte layout (matches SQLite)
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
const PENDING_BYTE: i64 = 0x40000000;
const RESERVED_BYTE: i64 = PENDING_BYTE + 1;
const SHARED_FIRST: i64 = PENDING_BYTE + 2;
const SHARED_SIZE: i64 = 510;

pub struct UnixFile {
    file: File,
    lock: LockLevel,
}

impl VfsFile for UnixFile {
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

    fn sync(&mut self, flags: SyncFlags) -> io::Result<()> {
        if flags.contains(SyncFlags::DATA_ONLY) {
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::io::AsRawFd;
                // SAFETY: fdatasync is a valid syscall on a valid fd.
                let rc = unsafe { libc::fdatasync(self.file.as_raw_fd()) };
                if rc != 0 {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
        }
        self.file.sync_all()
    }

    fn file_size(&self) -> io::Result<u64> {
        self.file.metadata().map(|m| m.len())
    }

    fn lock(&mut self, level: LockLevel) -> io::Result<()> {
        if level <= self.lock {
            return Ok(()); // Already have equal or stronger lock.
        }
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = self.file.as_raw_fd();
            match level {
                LockLevel::Shared => {
                    // Read lock on a random byte in the shared range.
                    fcntl_lock(fd, libc::F_RDLCK, SHARED_FIRST, 1)?;
                }
                LockLevel::Reserved => {
                    // Write lock on the reserved byte.
                    fcntl_lock(fd, libc::F_WRLCK, RESERVED_BYTE, 1)?;
                }
                LockLevel::Pending => {
                    // Write lock on the pending byte.
                    fcntl_lock(fd, libc::F_WRLCK, PENDING_BYTE, 1)?;
                }
                LockLevel::Exclusive => {
                    // Write lock over the entire shared range.
                    fcntl_lock(fd, libc::F_WRLCK, SHARED_FIRST, SHARED_SIZE)?;
                }
                LockLevel::None => {}
            }
        }
        self.lock = level;
        Ok(())
    }

    fn unlock(&mut self, level: LockLevel) -> io::Result<()> {
        if level >= self.lock {
            return Ok(());
        }
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = self.file.as_raw_fd();
            if self.lock >= LockLevel::Exclusive && level < LockLevel::Exclusive {
                fcntl_lock(fd, libc::F_UNLCK, SHARED_FIRST, SHARED_SIZE)?;
                if level >= LockLevel::Shared {
                    // Re-acquire shared lock after dropping exclusive.
                    fcntl_lock(fd, libc::F_RDLCK, SHARED_FIRST, 1)?;
                }
            }
            if self.lock >= LockLevel::Reserved && level < LockLevel::Reserved {
                fcntl_lock(fd, libc::F_UNLCK, RESERVED_BYTE, 1)?;
            }
            if self.lock >= LockLevel::Pending && level < LockLevel::Pending {
                fcntl_lock(fd, libc::F_UNLCK, PENDING_BYTE, 1)?;
            }
            if self.lock >= LockLevel::Shared && level < LockLevel::Shared {
                fcntl_lock(fd, libc::F_UNLCK, SHARED_FIRST, 1)?;
            }
        }
        self.lock = level;
        Ok(())
    }

    fn check_reserved_lock(&self) -> io::Result<bool> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = self.file.as_raw_fd();
            return fcntl_has_lock(fd, libc::F_WRLCK, RESERVED_BYTE, 1);
        }
        #[allow(unreachable_code)]
        Ok(false)
    }

    fn device_characteristics(&self) -> DeviceCharacteristics {
        DeviceCharacteristics::SAFE_APPEND
    }

    fn sector_size(&self) -> u32 {
        4096
    }
}

/// Apply an `fcntl` lock/unlock to a byte range.
#[cfg(unix)]
fn fcntl_lock(
    fd: std::os::unix::io::RawFd,
    lock_type: libc::c_int,
    start: i64,
    len: i64,
) -> io::Result<()> {
    let flock = libc::flock {
        l_type: lock_type as _,
        l_whence: libc::SEEK_SET as libc::c_short,
        l_start: start as libc::off_t,
        l_len: len as libc::off_t,
        l_pid: 0,
    };
    // SAFETY: fd is a valid file descriptor; flock is initialised above.
    let rc = unsafe { libc::fcntl(fd, libc::F_SETLK, &flock) };
    if rc == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Check whether another process holds a conflicting lock on a byte range.
#[cfg(unix)]
fn fcntl_has_lock(
    fd: std::os::unix::io::RawFd,
    lock_type: libc::c_int,
    start: i64,
    len: i64,
) -> io::Result<bool> {
    let mut flock = libc::flock {
        l_type: lock_type as _,
        l_whence: libc::SEEK_SET as libc::c_short,
        l_start: start as libc::off_t,
        l_len: len as libc::off_t,
        l_pid: 0,
    };
    // SAFETY: fd and flock are valid.
    let rc = unsafe { libc::fcntl(fd, libc::F_GETLK, &mut flock) };
    if rc == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(flock.l_type != libc::F_UNLCK as libc::c_short)
    }
}

/// The Unix VFS.
pub struct UnixVfs;

impl Vfs for UnixVfs {
    type File = UnixFile;

    fn open(&self, path: &Path, flags: OpenFlags) -> io::Result<Self::File> {
        let file = OpenOptions::new()
            .read(true)
            .write(!flags.contains(OpenFlags::READ_ONLY))
            .create(flags.contains(OpenFlags::CREATE))
            .open(path)?;
        Ok(UnixFile {
            file,
            lock: LockLevel::None,
        })
    }

    fn delete(&self, path: &Path, sync_dir: bool) -> io::Result<()> {
        std::fs::remove_file(path)?;
        if sync_dir {
            if let Some(parent) = path.parent() {
                let dir = File::open(parent)?;
                dir.sync_all()?;
            }
        }
        Ok(())
    }

    fn access(&self, path: &Path, _flags: AccessFlags) -> io::Result<bool> {
        Ok(path.exists())
    }

    fn full_pathname(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }

    fn randomness(&self, buf: &mut [u8]) {
        // SAFETY: getrandom is always safe to call.
        getrandom::getrandom(buf).expect("getrandom failed");
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
        // Return Julian Day Number.
        secs / 86400.0 + 2440587.5
    }

    fn name(&self) -> &str {
        "unix"
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use liter_vfs::{LockLevel, OpenFlags, VfsFile};
    use tempfile::NamedTempFile;

    fn open_file() -> (NamedTempFile, UnixFile) {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"\x00").unwrap();
        let vfs = UnixVfs;
        let f = vfs.open(tmp.path(), OpenFlags::READ_WRITE).unwrap();
        (tmp, f)
    }

    #[test]
    fn lock_upgrade_downgrade() {
        let (_tmp, mut f) = open_file();
        assert_eq!(f.lock, LockLevel::None);

        f.lock(LockLevel::Shared).unwrap();
        assert_eq!(f.lock, LockLevel::Shared);

        f.lock(LockLevel::Reserved).unwrap();
        assert_eq!(f.lock, LockLevel::Reserved);

        f.lock(LockLevel::Exclusive).unwrap();
        assert_eq!(f.lock, LockLevel::Exclusive);

        f.unlock(LockLevel::Shared).unwrap();
        assert_eq!(f.lock, LockLevel::Shared);

        f.unlock(LockLevel::None).unwrap();
        assert_eq!(f.lock, LockLevel::None);
    }

    #[test]
    fn lock_idempotent() {
        let (_tmp, mut f) = open_file();
        f.lock(LockLevel::Shared).unwrap();
        f.lock(LockLevel::Shared).unwrap(); // no-op
        assert_eq!(f.lock, LockLevel::Shared);
    }

    #[test]
    fn check_reserved_lock_false_when_no_locker() {
        let (_tmp, f) = open_file();
        assert!(!f.check_reserved_lock().unwrap());
    }

    #[test]
    fn check_reserved_lock_own_lock_posix_semantics() {
        // POSIX F_GETLK: our own locks are never reported as conflicting.
        let (_tmp, mut f) = open_file();
        f.lock(LockLevel::Shared).unwrap();
        f.lock(LockLevel::Reserved).unwrap();
        assert!(!f.check_reserved_lock().unwrap());
    }

    #[test]
    fn unlock_same_level_no_op() {
        let (_tmp, mut f) = open_file();
        f.lock(LockLevel::Shared).unwrap();
        f.unlock(LockLevel::Shared).unwrap(); // already at Shared → no-op
        assert_eq!(f.lock, LockLevel::Shared);
    }

    #[test]
    fn basic_rw_via_vfs_file() {
        let (_tmp, mut f) = open_file();
        f.write(b"hello", 0).unwrap();
        let mut buf = [0u8; 5];
        f.read(&mut buf, 0).unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn sector_size_and_characteristics() {
        let (_tmp, f) = open_file();
        assert_eq!(f.sector_size(), 4096);
        assert!(f
            .device_characteristics()
            .contains(DeviceCharacteristics::SAFE_APPEND));
    }
}
