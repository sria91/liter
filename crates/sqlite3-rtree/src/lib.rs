//! R*Tree extension for SQLite3-rs.
//!
//! Mirrors `rtree.c`. Provides a virtual table for spatial indexing using an
//! R*-tree backed by auxiliary B-tree pages.
//!
//! ## Status
//! Phase 3 — not yet implemented.

/// An R*Tree Virtual Table.
///
/// In a full implementation, this table manages auxiliary B-Tree pages containing
/// spatial node indices (min/max bounds for multiple dimensions).
pub struct RTreeTable {
    pub name: String,
    pub dimensions: usize,
}

impl RTreeTable {
    pub fn new(name: &str, dimensions: usize) -> Self {
        Self {
            name: name.to_string(),
            dimensions,
        }
    }
}
