//! Session extension for SQLite3-rs.
//!
//! Mirrors `session.c`. Provides the ability to track changes (inserts, updates,
//! deletes) applied to a database connection, and package them into a "changeset".
//!
//! ## Status
//! Phase 4 — Virtual table / tracker stubs.

#[derive(Debug, Clone, PartialEq)]
pub enum ChangeOp {
    Insert,
    Update,
    Delete,
}

#[derive(Debug, Clone)]
pub struct RowChange {
    pub op: ChangeOp,
    pub table_name: String,
    pub rowid: i64,
    // In a real implementation this would hold the old/new column values
}

/// A serialized batch of changes ready to be transmitted and applied elsewhere.
pub struct Changeset {
    pub changes: Vec<RowChange>,
}

impl Changeset {
    /// Applies a changeset to a hypothetical database connection.
    /// In this mock interface, we just return a status of how many changes would be applied.
    pub fn apply(&self) -> Result<usize, String> {
        // Here we would iterate `self.changes` and execute equivalent VDBE ops on the target connection.
        Ok(self.changes.len())
    }
}

/// A Session object that tracks changes on a database connection.
///
/// In a full implementation, this hooks into the VDBE commit lifecycle
/// to record row-level diffs into memory.
pub struct Session {
    pub db_name: String,
    pub active: bool,
    pub pending_changes: Vec<RowChange>,
}

impl Session {
    /// Create a new session tracking the given attached database (e.g. "main").
    pub fn new(db_name: &str) -> Self {
        Self {
            db_name: db_name.to_string(),
            active: true,
            pending_changes: Vec::new(),
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

    /// Hook to record a change to a row.
    pub fn record_change(&mut self, op: ChangeOp, table_name: &str, rowid: i64) {
        if self.active {
            self.pending_changes.push(RowChange {
                op,
                table_name: table_name.to_string(),
                rowid,
            });
        }
    }

    /// Generate a changeset of all tracked changes since the session began, and clear the tracking list.
    pub fn changeset_create(&mut self) -> Changeset {
        let changes = std::mem::take(&mut self.pending_changes);
        Changeset { changes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_tracking() {
        let mut session = Session::new("main");
        session.record_change(ChangeOp::Insert, "users", 1);
        session.record_change(ChangeOp::Update, "users", 2);
        
        let changeset = session.changeset_create();
        assert_eq!(changeset.changes.len(), 2);
        assert_eq!(changeset.changes[0].op, ChangeOp::Insert);
        
        // After create, pending changes are cleared
        let changeset2 = session.changeset_create();
        assert_eq!(changeset2.changes.len(), 0);
        
        // Disable tracking
        session.disable();
        session.record_change(ChangeOp::Delete, "users", 1);
        assert_eq!(session.changeset_create().changes.len(), 0);
        
        // Test apply
        assert_eq!(changeset.apply().unwrap(), 2);
    }
}
