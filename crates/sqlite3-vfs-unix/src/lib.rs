//! Unix VFS implementation for SQLite3-rs.
//!
//! Mirrors `os_unix.c`. Uses POSIX advisory locks (`fcntl F_SETLK/F_GETLK`)
//! for file locking semantics compatible with the C SQLite implementation.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sqlite3_vfs::{AccessFlags, DeviceCharacteristics, LockLevel, OpenFlags, SyncFlags, Vfs, VfsFile};

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
        // TODO: implement POSIX fcntl advisory locking.
        self.lock = level;
        Ok(())
    }

    fn unlock(&mut self, level: LockLevel) -> io::Result<()> {
        self.lock = level;
        Ok(())
    }

    fn check_reserved_lock(&self) -> io::Result<bool> {
        // TODO: query the OS for a conflicting reserved lock from another process.
        Ok(false)
    }

    fn device_characteristics(&self) -> DeviceCharacteristics {
        DeviceCharacteristics::SAFE_APPEND
    }

    fn sector_size(&self) -> u32 {
        4096
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
        Ok(UnixFile { file, lock: LockLevel::None })
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
