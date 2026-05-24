//! Full-Text Search (FTS5) extension for SQLite3-rs.
//!
//! Mirrors the `fts5.c` family. Provides virtual table-based full-text search
//! using a trigram / BM25 index stored in auxiliary B-tree pages.
//!
//! ## Status
//! Phase 3 — not yet implemented.

/// A token extracted from an FTS5 document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fts5Token {
    pub text: String,
    pub start_offset: usize,
    pub end_offset: usize,
}

/// Trait defining how text is broken down into tokens for the inverted index.
pub trait Fts5Tokenizer {
    fn tokenize(&self, text: &str) -> Vec<Fts5Token>;
}

/// A basic ASCII tokenizer splitting on non-alphanumeric boundaries.
pub struct AsciiTokenizer;

impl Fts5Tokenizer for AsciiTokenizer {
    fn tokenize(&self, text: &str) -> Vec<Fts5Token> {
        let mut tokens = Vec::new();
        let mut start = None;
        for (i, c) in text.char_indices() {
            if c.is_ascii_alphanumeric() {
                if start.is_none() {
                    start = Some(i);
                }
            } else if let Some(s) = start {
                tokens.push(Fts5Token {
                    text: text[s..i].to_lowercase(),
                    start_offset: s,
                    end_offset: i,
                });
                start = None;
            }
        }
        if let Some(s) = start {
            tokens.push(Fts5Token {
                text: text[s..].to_lowercase(),
                start_offset: s,
                end_offset: text.len(),
            });
        }
        tokens
    }
}

/// A specialized Virtual Table representing an FTS5 index.
/// 
/// In a full implementation, this table manages auxiliary B-Tree pages containing
/// the inverted term indices and BM25 document rankings.
pub struct Fts5Table {
    pub name: String,
    pub tokenizer: Box<dyn Fts5Tokenizer>,
}

impl Fts5Table {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            tokenizer: Box::new(AsciiTokenizer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ascii_tokenizer() {
        let t = AsciiTokenizer;
        let tokens = t.tokenize("Hello, World!");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].text, "hello");
        assert_eq!(tokens[1].text, "world");
    }
}
