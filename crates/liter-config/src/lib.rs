//! Global configuration for Liter-rs.
//!
//! Mirrors `global.c` and `config.c`. Provides a thread-safe singleton that
//! holds library-wide settings, mirroring `liter_config()`.

use std::sync::OnceLock;
use parking_lot::RwLock;

/// Thread-safety mode, mirroring SQLITE_THREADSAFE values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThreadsafeMode {
    /// No mutexes are used (single-threaded).
    SingleThread = 0,
    /// Serialized: all API calls are protected by a global mutex.
    Serialized = 1,
    /// Multi-thread: separate connections can be used concurrently.
    #[default]
    MultiThread = 2,
}

/// Library-wide configuration, set once before the first database is opened.
#[derive(Debug, Clone)]
pub struct GlobalConfig {
    pub threadsafe: ThreadsafeMode,
    /// Maximum number of pages in the lookaside freelist.
    pub lookaside_size: usize,
    /// Number of lookaside slots per connection.
    pub lookaside_count: usize,
    /// Heap memory limit (0 = unlimited).
    pub heap_limit: usize,
    /// Enable URI filename interpretation.
    pub uri_enabled: bool,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            threadsafe: ThreadsafeMode::MultiThread,
            lookaside_size: 128,
            lookaside_count: 500,
            heap_limit: 0,
            uri_enabled: false,
        }
    }
}

static CONFIG: OnceLock<RwLock<GlobalConfig>> = OnceLock::new();

fn config_lock() -> &'static RwLock<GlobalConfig> {
    CONFIG.get_or_init(|| RwLock::new(GlobalConfig::default()))
}

/// Read the current global configuration.
pub fn get() -> GlobalConfig {
    config_lock().read().clone()
}

/// Update the global configuration.
///
/// # Errors
/// Returns `Err` if any connections are already open (not tracked in this
/// stub; callers should call this before opening any database).
pub fn set(config: GlobalConfig) -> Result<(), ConfigError> {
    *config_lock().write() = config;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot change config while databases are open")]
    DatabasesOpen,
    #[error("invalid configuration value")]
    InvalidValue,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_multithread() {
        let cfg = get();
        assert_eq!(cfg.threadsafe, ThreadsafeMode::MultiThread);
    }

    #[test]
    fn set_and_get() {
        let mut cfg = get();
        cfg.uri_enabled = true;
        set(cfg).unwrap();
        assert!(get().uri_enabled);
        // Reset.
        let mut cfg = get();
        cfg.uri_enabled = false;
        set(cfg).unwrap();
    }
}
