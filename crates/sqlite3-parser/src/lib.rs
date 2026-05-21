//! SQL parser for SQLite3-rs.
//!
//! Mirrors the Lemon-generated LALR(1) parser from `parse.y`. Implemented as
//! a hand-written recursive-descent parser using `sqlite3-tokenizer` for
//! lexing.
//!
//! ## Status
//! Phase 3 — stub skeleton only. Returns `ParseError::NotImplemented` for all
//! inputs until the full grammar is ported.

use sqlite3_ast::Stmt;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("syntax error near '{0}'")]
    SyntaxError(String),
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("not yet implemented")]
    NotImplemented,
    #[error("tokenizer error: {0}")]
    TokenError(#[from] sqlite3_tokenizer::TokenError),
}

pub type ParseResult<T> = Result<T, ParseError>;

/// Parse a single SQL statement from `input`.
///
/// Returns `Ok(stmt)` on success or a `ParseError` on failure.
/// If `input` contains multiple statements, only the first is parsed.
pub fn parse_stmt(input: &str) -> ParseResult<Stmt> {
    let _ = input;
    Err(ParseError::NotImplemented)
}

/// Parse all SQL statements from `input`, separated by `;`.
pub fn parse_all(input: &str) -> ParseResult<Vec<Stmt>> {
    let _ = input;
    Err(ParseError::NotImplemented)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_returns_not_implemented() {
        assert!(matches!(parse_stmt("SELECT 1"), Err(ParseError::NotImplemented)));
    }
}
