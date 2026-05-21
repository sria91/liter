//! Name resolution and type affinity for SQLite3-rs.
//!
//! Mirrors `resolve.c`. Walks the AST and binds each column reference to its
//! source table/expression; computes type affinity for expressions.
//!
//! ## Status
//! Phase 3 — stub.

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("no such table: {0}")]
    NoSuchTable(String),
    #[error("no such column: {0}")]
    NoSuchColumn(String),
    #[error("ambiguous column name: {0}")]
    AmbiguousColumn(String),
    #[error("not yet implemented")]
    NotImplemented,
}

pub type ResolveResult<T> = Result<T, ResolveError>;

/// Type affinity, as defined by the SQLite spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affinity {
    Text,
    Numeric,
    Integer,
    Real,
    Blob,
}

impl Affinity {
    /// Determine affinity from a declared type string (§3.1 of the file format spec).
    pub fn from_type_name(name: &str) -> Self {
        let n = name.to_ascii_uppercase();
        if n.contains("INT") {
            Affinity::Integer
        } else if n.contains("CHAR") || n.contains("CLOB") || n.contains("TEXT") {
            Affinity::Text
        } else if n.contains("BLOB") || n.is_empty() {
            Affinity::Blob
        } else if n.contains("REAL") || n.contains("FLOA") || n.contains("DOUB") {
            Affinity::Real
        } else {
            Affinity::Numeric
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn affinity_integer() {
        assert_eq!(Affinity::from_type_name("INTEGER"), Affinity::Integer);
        assert_eq!(Affinity::from_type_name("INT"), Affinity::Integer);
        assert_eq!(Affinity::from_type_name("TINYINT"), Affinity::Integer);
    }

    #[test]
    fn affinity_text() {
        assert_eq!(Affinity::from_type_name("VARCHAR(255)"), Affinity::Text);
        assert_eq!(Affinity::from_type_name("TEXT"), Affinity::Text);
        assert_eq!(Affinity::from_type_name("CLOB"), Affinity::Text);
    }

    #[test]
    fn affinity_blob() {
        assert_eq!(Affinity::from_type_name("BLOB"), Affinity::Blob);
        assert_eq!(Affinity::from_type_name(""), Affinity::Blob);
    }

    #[test]
    fn affinity_real() {
        assert_eq!(Affinity::from_type_name("REAL"), Affinity::Real);
        assert_eq!(Affinity::from_type_name("DOUBLE"), Affinity::Real);
        assert_eq!(Affinity::from_type_name("FLOAT"), Affinity::Real);
    }

    #[test]
    fn affinity_numeric() {
        assert_eq!(Affinity::from_type_name("NUMERIC"), Affinity::Numeric);
        assert_eq!(Affinity::from_type_name("DECIMAL"), Affinity::Numeric);
    }
}
