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

/// Calculates the BM25 relevance score for a document.
///
/// Parameters:
/// - `term_freq`: Frequency of the term in the current document.
/// - `doc_length`: Total number of tokens in the current document.
/// - `avg_doc_length`: Average number of tokens across all documents in the index.
/// - `total_docs`: Total number of documents in the index.
/// - `doc_freq`: Number of documents that contain the term.
pub fn bm25_score(
    term_freq: f64,
    doc_length: f64,
    avg_doc_length: f64,
    total_docs: f64,
    doc_freq: f64,
) -> f64 {
    let k1 = 1.2;
    let b = 0.75;

    // IDF (Inverse Document Frequency) formula typically used by SQLite FTS5 / Lucene
    let idf = ((total_docs - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0).ln();

    // TF (Term Frequency) weighted
    let tf_denom = term_freq + k1 * (1.0 - b + b * (doc_length / avg_doc_length));
    let tf_weighted = (term_freq * (k1 + 1.0)) / tf_denom;

    idf * tf_weighted
}

/// Defines the names of the shadow tables created by an FTS5 virtual table.
/// 
/// If the vtab is named `xyz`, the shadow tables are:
/// - `xyz_data`: Stores the actual inverted index B-Tree nodes.
/// - `xyz_idx`: Stores the mapping from terms to `xyz_data` segment blocks.
/// - `xyz_docsize`: Stores the size (in tokens) of each inserted document (for BM25).
/// - `xyz_config`: Configuration and global statistics (e.g. total document count).
pub struct Fts5ShadowTables {
    pub data_table: String,
    pub idx_table: String,
    pub docsize_table: String,
    pub config_table: String,
}

impl Fts5ShadowTables {
    pub fn new(table_name: &str) -> Self {
        Self {
            data_table: format!("{}_data", table_name),
            idx_table: format!("{}_idx", table_name),
            docsize_table: format!("{}_docsize", table_name),
            config_table: format!("{}_config", table_name),
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

    #[test]
    fn test_bm25_score() {
        let score = bm25_score(3.0, 100.0, 150.0, 1000.0, 50.0);
        assert!(score > 0.0); // should be a positive score
    }

    #[test]
    fn test_shadow_tables() {
        let tables = Fts5ShadowTables::new("docs");
        assert_eq!(tables.data_table, "docs_data");
        assert_eq!(tables.idx_table, "docs_idx");
        assert_eq!(tables.docsize_table, "docs_docsize");
        assert_eq!(tables.config_table, "docs_config");
    }
}
