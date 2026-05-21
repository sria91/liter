//! Full-Text Search (FTS5) extension for SQLite3-rs.
//!
//! Mirrors the `fts5.c` family. Provides virtual table-based full-text search
//! using a trigram / BM25 index stored in auxiliary B-tree pages.
//!
//! ## Status
//! Phase 3 — not yet implemented.

/// Placeholder to ensure the crate compiles.
pub fn fts5_enabled() -> bool {
    false
}
