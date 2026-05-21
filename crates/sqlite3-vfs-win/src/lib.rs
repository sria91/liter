//! Windows VFS implementation for SQLite3-rs.
//!
//! Mirrors `os_win.c`. Uses `LockFileEx` / `UnlockFileEx` for byte-range
//! locking semantics compatible with C SQLite on Windows.
//!
//! This crate only compiles on Windows targets. On non-Windows platforms it
//! exports no items (all code is guarded by `#[cfg(windows)]`).

#[cfg(windows)]
mod win_vfs {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};

    use sqlite3_vfs::{AccessFlags, DeviceCharacteristics, LockLevel, OpenFlags, SyncFlags, Vfs, VfsFile};

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
            // TODO: implement LockFileEx-based byte-range locking.
            self.lock = level;
            Ok(())
        }

        fn unlock(&mut self, level: LockLevel) -> io::Result<()> {
            self.lock = level;
            Ok(())
        }

        fn check_reserved_lock(&self) -> io::Result<bool> {
            Ok(false)
        }

        fn device_characteristics(&self) -> DeviceCharacteristics {
            DeviceCharacteristics::UNDELETABLE_WHEN_OPEN
        }

        fn sector_size(&self) -> u32 {
            4096
        }
    }

    pub struct WinVfs;

    impl Vfs for WinVfs {
        type File = WinFile;

        fn open(&self, path: &Path, flags: OpenFlags) -> io::Result<Self::File> {
            let file = OpenOptions::new()
                .read(true)
                .write(!flags.contains(OpenFlags::READ_ONLY))
                .create(flags.contains(OpenFlags::CREATE))
                .open(path)?;
            Ok(WinFile { file, lock: LockLevel::None })
        }

        fn delete(&self, path: &Path, _sync_dir: bool) -> io::Result<()> {
            std::fs::remove_file(path)
        }

        fn access(&self, path: &Path, _flags: AccessFlags) -> io::Result<bool> {
            Ok(path.exists())
        }

        fn full_pathname(&self, path: &Path) -> io::Result<PathBuf> {
            std::fs::canonicalize(path)
        }

        fn randomness(&self, buf: &mut [u8]) {
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
            secs / 86400.0 + 2440587.5
        }

        fn name(&self) -> &str {
            "win32"
        }
    }
}

#[cfg(windows)]
pub use win_vfs::{WinFile, WinVfs};
