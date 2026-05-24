//! Session extension for SQLite3-rs.
//!
//! Mirrors `session.c`. Provides the ability to track changes (inserts, updates,
//! deletes) applied to a database connection, and package them into a "changeset".
//!
//! ## Status
//! Phase 4 — Virtual table / tracker stubs.

/// A Session object that tracks changes on a database connection.
///
/// In a full implementation, this hooks into the VDBE commit lifecycle
/// to record row-level diffs into memory.
pub struct Session {
    pub db_name: String,
    pub active: bool,
}

impl Session {
    /// Create a new session tracking the given attached database (e.g. "main").
    pub fn new(db_name: &str) -> Self {
        Self {
            db_name: db_name.to_string(),
            active: true,
        }
    }

    /// Pause the session from tracking changes.
    pub fn disable(&mut self) {
        self.active = false;
    }

    /// Resume session tracking.
    pub fn enable(&mut self) {
        self.active = true;
    }
}
