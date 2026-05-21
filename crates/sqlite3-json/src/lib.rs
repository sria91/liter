//! JSON1 extension for SQLite3-rs.
//!
//! Mirrors `json.c`. Provides `json()`, `json_extract()`, `json_object()`,
//! `json_array()`, `json_patch()`, and related functions.
//!
//! ## Status
//! Phase 3 — not yet implemented.

pub fn json1_enabled() -> bool {
    false
}
