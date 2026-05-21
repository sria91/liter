//! R*Tree extension for SQLite3-rs.
//!
//! Mirrors `rtree.c`. Provides a virtual table for spatial indexing using an
//! R*-tree backed by auxiliary B-tree pages.
//!
//! ## Status
//! Phase 3 — not yet implemented.

pub fn rtree_enabled() -> bool {
    false
}
