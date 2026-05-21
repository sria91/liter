//! File format compatibility tests.
//!
//! Opens `.db` files created by C SQLite and verifies that our implementation
//! can read all tables correctly. Covers legacy page sizes, freelist pages,
//! overflow chains, and WAL-mode files.

#[cfg(test)]
mod tests {
    #[test]
    fn placeholder_format_compat() {
        // TODO: add .db fixtures and validate once pager + btree are implemented.
    }
}
