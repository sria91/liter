//! In-memory VFS for SQLite3-rs.
//!
//! Used for `:memory:` databases and in unit tests. All data is stored in a
//! `Vec<u8>` protected by a `Mutex`. Simulates a single-file in-memory device.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use sqlite3_vfs::{AccessFlags, DeviceCharacteristics, LockLevel, OpenFlags, SyncFlags, Vfs, VfsFile};

/// Shared backing store for a single in-memory file.
#[derive(Debug, Default, Clone)]
pub struct MemStore(Arc<Mutex<Vec<u8>>>);

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.0.lock().len()
    }
}

/// An open handle to an in-memory file.
pub struct MemFile {
    store: MemStore,
    lock: LockLevel,
}

impl VfsFile for MemFile {
    fn read(&mut self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        let data = self.store.0.lock();
        let start = offset as usize;
        if start >= data.len() {
            return Ok(0);
        }
        let end = (start + buf.len()).min(data.len());
        let n = end - start;
        buf[..n].copy_from_slice(&data[start..end]);
        Ok(n)
    }

    fn write(&mut self, buf: &[u8], offset: u64) -> io::Result<()> {
        let mut data = self.store.0.lock();
        let start = offset as usize;
        let end = start + buf.len();
        if end > data.len() {
            data.resize(end, 0);
        }
        data[start..end].copy_from_slice(buf);
        Ok(())
    }

    fn truncate(&mut self, size: u64) -> io::Result<()> {
        self.store.0.lock().resize(size as usize, 0);
        Ok(())
    }

    fn sync(&mut self, _flags: SyncFlags) -> io::Result<()> {
        // No-op for in-memory storage.
        Ok(())
    }

    fn file_size(&self) -> io::Result<u64> {
        Ok(self.store.0.lock().len() as u64)
    }

    fn lock(&mut self, level: LockLevel) -> io::Result<()> {
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
        DeviceCharacteristics::ATOMIC
            | DeviceCharacteristics::SAFE_APPEND
            | DeviceCharacteristics::SEQUENTIAL
    }

    fn sector_size(&self) -> u32 {
        512
    }
}

/// In-memory VFS. Each path maps to a distinct `MemStore`.
pub struct MemVfs {
    files: Mutex<std::collections::HashMap<PathBuf, MemStore>>,
}

impl MemVfs {
    pub fn new() -> Self {
        Self {
            files: Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for MemVfs {
    fn default() -> Self {
        Self::new()
    }
}

impl Vfs for MemVfs {
    type File = MemFile;

    fn open(&self, path: &Path, _flags: OpenFlags) -> io::Result<Self::File> {
        let mut files = self.files.lock();
        let store = files.entry(path.to_path_buf()).or_default().clone();
        Ok(MemFile { store, lock: LockLevel::None })
    }

    fn delete(&self, path: &Path, _sync_dir: bool) -> io::Result<()> {
        self.files.lock().remove(path);
        Ok(())
    }

    fn access(&self, path: &Path, _flags: AccessFlags) -> io::Result<bool> {
        Ok(self.files.lock().contains_key(path))
    }

    fn full_pathname(&self, path: &Path) -> io::Result<PathBuf> {
        Ok(path.to_path_buf())
    }

    fn randomness(&self, buf: &mut [u8]) {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(0x9E).wrapping_add(0x3F);
        }
    }

    fn sleep(&self, _micros: u64) {}

    fn current_time(&self) -> f64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        secs / 86400.0 + 2440587.5
    }

    fn name(&self) -> &str {
        "memvfs"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_read_back() {
        let vfs = MemVfs::new();
        let mut f = vfs.open(Path::new(":memory:"), OpenFlags::READ_WRITE | OpenFlags::CREATE).unwrap();
        f.write(b"hello", 0).unwrap();
        let mut buf = [0u8; 5];
        let n = f.read(&mut buf, 0).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn write_at_offset() {
        let vfs = MemVfs::new();
        let mut f = vfs.open(Path::new(":memory:"), OpenFlags::READ_WRITE | OpenFlags::CREATE).unwrap();
        f.write(b"world", 5).unwrap();
        assert_eq!(f.file_size().unwrap(), 10);
        let mut buf = [0u8; 5];
        f.read(&mut buf, 5).unwrap();
        assert_eq!(&buf, b"world");
    }

    #[test]
    fn truncate() {
        let vfs = MemVfs::new();
        let mut f = vfs.open(Path::new(":memory:"), OpenFlags::READ_WRITE | OpenFlags::CREATE).unwrap();
        f.write(b"hello world", 0).unwrap();
        f.truncate(5).unwrap();
        assert_eq!(f.file_size().unwrap(), 5);
    }
}
