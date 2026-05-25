//! Virtual File System (VFS) trait definitions.
//!
//! Mirrors the `liter_vfs` and `sqlite3_file` interfaces from `sqlite3.h`.
//! Concrete implementations live in `liter-vfs-unix`, `liter-vfs-win`,
//! and `liter-vfs-mem`.

use std::io;
use std::path::{Path, PathBuf};

use bitflags::bitflags;

bitflags! {
    /// File open flags (SQLITE_OPEN_*).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct OpenFlags: u32 {
        const READ_ONLY       = 0x00000001;
        const READ_WRITE      = 0x00000002;
        const CREATE          = 0x00000004;
        const DELETE_ON_CLOSE = 0x00000008;
        const EXCLUSIVE       = 0x00000010;
        const AUTO_PROXY      = 0x00000020;
        const URI             = 0x00000040;
        const MEMORY          = 0x00000080;
        const MAIN_DB         = 0x00000100;
        const TEMP_DB         = 0x00000200;
        const TRANSIENT_DB    = 0x00000400;
        const MAIN_JOURNAL    = 0x00000800;
        const TEMP_JOURNAL    = 0x00001000;
        const SUBJOURNAL      = 0x00002000;
        const SUPER_JOURNAL   = 0x00004000;
        const NO_MUTEX        = 0x00008000;
        const FULL_MUTEX      = 0x00010000;
        const SHARED_CACHE    = 0x00020000;
        const PRIVATE_CACHE   = 0x00040000;
        const WAL             = 0x00080000;
        const NOFOLLOW        = 0x01000000;
    }
}

bitflags! {
    /// File access query flags (SQLITE_ACCESS_*).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct AccessFlags: u32 {
        const EXISTS    = 0;
        const READ_WRITE = 1;
        const READ      = 2;
    }
}

bitflags! {
    /// Sync flags (SQLITE_SYNC_*).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SyncFlags: u32 {
        const NORMAL   = 0x00002;
        const FULL     = 0x00003;
        const DATA_ONLY = 0x00010;
    }
}

bitflags! {
    /// Device characteristics (SQLITE_IOCAP_*).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct DeviceCharacteristics: u32 {
        const ATOMIC                 = 0x00000001;
        const ATOMIC512              = 0x00000002;
        const ATOMIC1K               = 0x00000004;
        const ATOMIC2K               = 0x00000008;
        const ATOMIC4K               = 0x00000010;
        const ATOMIC8K               = 0x00000020;
        const ATOMIC16K              = 0x00000040;
        const ATOMIC32K              = 0x00000080;
        const ATOMIC64K              = 0x00000100;
        const SAFE_APPEND            = 0x00000200;
        const SEQUENTIAL             = 0x00000400;
        const UNDELETABLE_WHEN_OPEN  = 0x00000800;
        const POWERSAFE_OVERWRITE    = 0x00001000;
        const IMMUTABLE              = 0x00002000;
        const BATCH_ATOMIC           = 0x00004000;
    }
}

/// File locking levels (SQLITE_LOCK_*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LockLevel {
    None      = 0,
    Shared    = 1,
    Reserved  = 2,
    Pending   = 3,
    Exclusive = 4,
}

/// Trait representing an open database file.
pub trait VfsFile: Send {
    fn read(&mut self, buf: &mut [u8], offset: u64) -> io::Result<usize>;
    fn write(&mut self, buf: &[u8], offset: u64) -> io::Result<()>;
    fn truncate(&mut self, size: u64) -> io::Result<()>;
    fn sync(&mut self, flags: SyncFlags) -> io::Result<()>;
    fn file_size(&self) -> io::Result<u64>;
    fn lock(&mut self, level: LockLevel) -> io::Result<()>;
    fn unlock(&mut self, level: LockLevel) -> io::Result<()>;
    fn check_reserved_lock(&self) -> io::Result<bool>;
    fn device_characteristics(&self) -> DeviceCharacteristics;
    fn sector_size(&self) -> u32 {
        512
    }
}

/// Trait representing a VFS implementation.
pub trait Vfs: Send + Sync {
    type File: VfsFile;

    fn open(&self, path: &Path, flags: OpenFlags) -> io::Result<Self::File>;
    fn delete(&self, path: &Path, sync_dir: bool) -> io::Result<()>;
    fn access(&self, path: &Path, flags: AccessFlags) -> io::Result<bool>;
    fn full_pathname(&self, path: &Path) -> io::Result<PathBuf>;
    fn randomness(&self, buf: &mut [u8]);
    fn sleep(&self, micros: u64);
    fn current_time(&self) -> f64;
    fn name(&self) -> &str;
    fn max_pathname(&self) -> usize {
        512
    }
}

/// Error type for VFS operations.
#[derive(Debug, thiserror::Error)]
pub enum VfsError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("file not found: {0}")]
    NotFound(PathBuf),
    #[error("lock conflict")]
    LockConflict,
    #[error("read-only file system")]
    ReadOnly,
}
