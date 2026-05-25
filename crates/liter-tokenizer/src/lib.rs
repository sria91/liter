//! SQL tokenizer for Liter-rs.
//!
//! Mirrors `tokenize.c`. Uses `logos` for fast lexing.
//! Produces a stream of `Token` values that the parser consumes.

use logos::Logos;

/// Error type for tokenizer failures.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Default)]
pub enum TokenError {
    #[default]
    #[error("unexpected character")]
    Unexpected,
    #[error("unterminated string literal")]
    UnterminatedString,
    #[error("unterminated block comment")]
    UnterminatedComment,
}

/// SQL tokens produced by the lexer.
#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(error = TokenError)]
#[logos(skip r"[ \t\r\n\f]+")]
// Block comments
#[logos(skip r"/\*([^*]|\*[^/])*\*/")]
// Line comments
#[logos(skip r"--[^\n]*")]
pub enum Token<'src> {
    // ── Keywords (case-insensitive) ──────────────────────────────────────────
    #[token("ABORT",    ignore(ascii_case))] Abort,
    #[token("ACTION",   ignore(ascii_case))] Action,
    #[token("ADD",      ignore(ascii_case))] Add,
    #[token("ALL",      ignore(ascii_case))] All,
    #[token("ALTER",    ignore(ascii_case))] Alter,
    #[token("ANALYZE",  ignore(ascii_case))] Analyze,
    #[token("AND",      ignore(ascii_case))] And,
    #[token("AS",       ignore(ascii_case))] As,
    #[token("ASC",      ignore(ascii_case))] Asc,
    #[token("ATTACH",   ignore(ascii_case))] Attach,
    #[token("AUTOINCREMENT", ignore(ascii_case))] Autoincrement,
    #[token("BEFORE",   ignore(ascii_case))] Before,
    #[token("BEGIN",    ignore(ascii_case))] Begin,
    #[token("BETWEEN",  ignore(ascii_case))] Between,
    #[token("BY",       ignore(ascii_case))] By,
    #[token("CASCADE",  ignore(ascii_case))] Cascade,
    #[token("CASE",     ignore(ascii_case))] Case,
    #[token("CAST",     ignore(ascii_case))] Cast,
    #[token("CHECK",    ignore(ascii_case))] Check,
    #[token("COLLATE",  ignore(ascii_case))] Collate,
    #[token("COLUMN",   ignore(ascii_case))] Column,
    #[token("COMMIT",   ignore(ascii_case))] Commit,
    #[token("CONFLICT", ignore(ascii_case))] Conflict,
    #[token("CONSTRAINT", ignore(ascii_case))] Constraint,
    #[token("CREATE",   ignore(ascii_case))] Create,
    #[token("CROSS",    ignore(ascii_case))] Cross,
    #[token("CURRENT_DATE",      ignore(ascii_case))] CurrentDate,
    #[token("CURRENT_TIME",      ignore(ascii_case))] CurrentTime,
    #[token("CURRENT_TIMESTAMP", ignore(ascii_case))] CurrentTimestamp,
    #[token("DATABASE", ignore(ascii_case))] Database,
    #[token("DEFAULT",  ignore(ascii_case))] Default,
    #[token("DEFERRABLE", ignore(ascii_case))] Deferrable,
    #[token("DEFERRED", ignore(ascii_case))] Deferred,
    #[token("DELETE",   ignore(ascii_case))] Delete,
    #[token("DESC",     ignore(ascii_case))] Desc,
    #[token("DETACH",   ignore(ascii_case))] Detach,
    #[token("DISTINCT", ignore(ascii_case))] Distinct,
    #[token("DO",       ignore(ascii_case))] Do,
    #[token("DROP",     ignore(ascii_case))] Drop,
    #[token("EACH",     ignore(ascii_case))] Each,
    #[token("ELSE",     ignore(ascii_case))] Else,
    #[token("END",      ignore(ascii_case))] End,
    #[token("ESCAPE",   ignore(ascii_case))] Escape,
    #[token("EXCEPT",   ignore(ascii_case))] Except,
    #[token("EXCLUSIVE", ignore(ascii_case))] Exclusive,
    #[token("EXISTS",   ignore(ascii_case))] Exists,
    #[token("EXPLAIN",  ignore(ascii_case))] Explain,
    #[token("FAIL",     ignore(ascii_case))] Fail,
    #[token("FILTER",   ignore(ascii_case))] Filter,
    #[token("FIRST",    ignore(ascii_case))] First,
    #[token("FOLLOWING", ignore(ascii_case))] Following,
    #[token("FOR",      ignore(ascii_case))] For,
    #[token("FOREIGN",  ignore(ascii_case))] Foreign,
    #[token("FROM",     ignore(ascii_case))] From,
    #[token("FULL",     ignore(ascii_case))] Full,
    #[token("GLOB",     ignore(ascii_case))] Glob,
    #[token("GROUP",    ignore(ascii_case))] Group,
    #[token("GROUPS",   ignore(ascii_case))] Groups,
    #[token("HAVING",   ignore(ascii_case))] Having,
    #[token("IF",       ignore(ascii_case))] If,
    #[token("IGNORE",   ignore(ascii_case))] Ignore,
    #[token("IMMEDIATE", ignore(ascii_case))] Immediate,
    #[token("IN",       ignore(ascii_case))] In,
    #[token("INDEX",    ignore(ascii_case))] Index,
    #[token("INDEXED",  ignore(ascii_case))] Indexed,
    #[token("INITIALLY", ignore(ascii_case))] Initially,
    #[token("INNER",    ignore(ascii_case))] Inner,
    #[token("INSERT",   ignore(ascii_case))] Insert,
    #[token("INSTEAD",  ignore(ascii_case))] Instead,
    #[token("INTERSECT", ignore(ascii_case))] Intersect,
    #[token("INTO",     ignore(ascii_case))] Into,
    #[token("IS",       ignore(ascii_case))] Is,
    #[token("ISNULL",   ignore(ascii_case))] IsNull,
    #[token("JOIN",     ignore(ascii_case))] Join,
    #[token("KEY",      ignore(ascii_case))] Key,
    #[token("LAST",     ignore(ascii_case))] Last,
    #[token("LEFT",     ignore(ascii_case))] Left,
    #[token("LIKE",     ignore(ascii_case))] Like,
    #[token("LIMIT",    ignore(ascii_case))] Limit,
    #[token("MATCH",    ignore(ascii_case))] Match,
    #[token("MATERIALIZED", ignore(ascii_case))] Materialized,
    #[token("NATURAL",  ignore(ascii_case))] Natural,
    #[token("NO",       ignore(ascii_case))] No,
    #[token("NOT",      ignore(ascii_case))] Not,
    #[token("NOTHING",  ignore(ascii_case))] Nothing,
    #[token("NOTNULL",  ignore(ascii_case))] NotNull,
    #[token("NULL",     ignore(ascii_case))] Null,
    #[token("NULLS",    ignore(ascii_case))] Nulls,
    #[token("OF",       ignore(ascii_case))] Of,
    #[token("OFFSET",   ignore(ascii_case))] Offset,
    #[token("ON",       ignore(ascii_case))] On,
    #[token("OR",       ignore(ascii_case))] Or,
    #[token("ORDER",    ignore(ascii_case))] Order,
    #[token("OTHERS",   ignore(ascii_case))] Others,
    #[token("OUTER",    ignore(ascii_case))] Outer,
    #[token("OVER",     ignore(ascii_case))] Over,
    #[token("PARTITION", ignore(ascii_case))] Partition,
    #[token("PLAN",     ignore(ascii_case))] Plan,
    #[token("PRAGMA",   ignore(ascii_case))] Pragma,
    #[token("PRECEDING", ignore(ascii_case))] Preceding,
    #[token("PRIMARY",  ignore(ascii_case))] Primary,
    #[token("QUERY",    ignore(ascii_case))] Query,
    #[token("RAISE",    ignore(ascii_case))] Raise,
    #[token("RANGE",    ignore(ascii_case))] Range,
    #[token("RECURSIVE", ignore(ascii_case))] Recursive,
    #[token("REFERENCES", ignore(ascii_case))] References,
    #[token("REGEXP",   ignore(ascii_case))] Regexp,
    #[token("REINDEX",  ignore(ascii_case))] Reindex,
    #[token("RELEASE",  ignore(ascii_case))] Release,
    #[token("RENAME",   ignore(ascii_case))] Rename,
    #[token("REPLACE",  ignore(ascii_case))] Replace,
    #[token("RESTRICT", ignore(ascii_case))] Restrict,
    #[token("RETURNING", ignore(ascii_case))] Returning,
    #[token("RIGHT",    ignore(ascii_case))] Right,
    #[token("ROLLBACK", ignore(ascii_case))] Rollback,
    #[token("ROW",      ignore(ascii_case))] Row,
    #[token("ROWS",     ignore(ascii_case))] Rows,
    #[token("SAVEPOINT", ignore(ascii_case))] Savepoint,
    #[token("SELECT",   ignore(ascii_case))] Select,
    #[token("SET",      ignore(ascii_case))] Set,
    #[token("TABLE",    ignore(ascii_case))] Table,
    #[token("TEMP",     ignore(ascii_case))] Temp,
    #[token("TEMPORARY", ignore(ascii_case))] Temporary,
    #[token("THEN",     ignore(ascii_case))] Then,
    #[token("TIES",     ignore(ascii_case))] Ties,
    #[token("TO",       ignore(ascii_case))] To,
    #[token("TRANSACTION", ignore(ascii_case))] Transaction,
    #[token("TRIGGER",  ignore(ascii_case))] Trigger,
    #[token("UNBOUNDED", ignore(ascii_case))] Unbounded,
    #[token("UNION",    ignore(ascii_case))] Union,
    #[token("UNIQUE",   ignore(ascii_case))] Unique,
    #[token("UPDATE",   ignore(ascii_case))] Update,
    #[token("USING",    ignore(ascii_case))] Using,
    #[token("VACUUM",   ignore(ascii_case))] Vacuum,
    #[token("VALUES",   ignore(ascii_case))] Values,
    #[token("VIEW",     ignore(ascii_case))] View,
    #[token("VIRTUAL",  ignore(ascii_case))] Virtual,
    #[token("WHEN",     ignore(ascii_case))] When,
    #[token("WHERE",    ignore(ascii_case))] Where,
    #[token("WINDOW",   ignore(ascii_case))] Window,
    #[token("WITH",     ignore(ascii_case))] With,
    #[token("WITHOUT",  ignore(ascii_case))] Without,

    // ── Literals ─────────────────────────────────────────────────────────────
    /// Integer literal: `42`, `0x1F`
    #[regex(r"0[xX][0-9a-fA-F]+|[0-9]+")]
    Integer(&'src str),

    /// Floating-point literal: `3.14`, `1e10`, `2.5E-3`
    #[regex(r"[0-9]+\.[0-9]*([eE][+\-]?[0-9]+)?|[0-9]+[eE][+\-]?[0-9]+|\.[0-9]+([eE][+\-]?[0-9]+)?")]
    Float(&'src str),

    /// Single-quoted string literal: `'hello'` (doubled `''` is an escaped quote)
    #[regex(r"'([^'\\]|''|\\.)*'")]
    StringLit(&'src str),

    /// Identifier or quoted identifier: `foo`, `"bar"`, `` `baz` ``, `[qux]`
    #[regex(r#"[a-zA-Z_][a-zA-Z0-9_]*|"([^"\\]|\\.)*"|`([^`])*`|\[([^\]]*)\]"#)]
    Ident(&'src str),

    /// Blob literal: `X'deadbeef'`
    #[regex(r"[xX]'[0-9a-fA-F]*'")]
    Blob(&'src str),

    /// Bind parameter: `?`, `?1`, `:name`, `@name`, `$name`
    #[regex(r"\?[0-9]*|:[a-zA-Z_][a-zA-Z0-9_]*|@[a-zA-Z_][a-zA-Z0-9_]*|\$[a-zA-Z_][a-zA-Z0-9_]*")]
    Param(&'src str),

    // ── Punctuation ───────────────────────────────────────────────────────────
    #[token("(")] LParen,
    #[token(")")] RParen,
    #[token(",")] Comma,
    #[token(";")] Semi,
    #[token(".")] Dot,
    #[token("*")] Star,
    #[token("/")] Slash,
    #[token("%")] Percent,
    #[token("+")] Plus,
    #[token("-")] Minus,
    #[token("&")] Amp,
    #[token("|")] Pipe,
    #[token("~")] Tilde,
    #[token("<<")] LShift,
    #[token(">>")] RShift,
    #[token("||")] Concat,
    #[token("=")] Eq,
    #[token("==")] EqEq,
    #[token("<>")] Ne,
    #[token("!=")] BangEq,
    #[token("<")] Lt,
    #[token("<=")] Le,
    #[token(">")] Gt,
    #[token(">=")] Ge,
    #[token("->")] Arrow,
    #[token("->>")] ArrowArrow,
}

/// Tokenize the input SQL, returning an iterator of `(Token, span)` pairs.
/// Errors are embedded as `Err(TokenError)` items.
pub fn tokenize(input: &str) -> impl Iterator<Item = Result<(Token<'_>, std::ops::Range<usize>), TokenError>> + '_ {
    Token::lexer(input).spanned().map(|(tok, span)| {
        tok.map(|t| (t, span))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_simple_select() {
        let sql = "SELECT * FROM users WHERE id = 1;";
        let tokens: Vec<_> = tokenize(sql).collect();
        assert!(tokens.iter().all(|r| r.is_ok()));
        let kinds: Vec<_> = tokens.into_iter().map(|r| r.unwrap().0).collect();
        assert!(matches!(kinds[0], Token::Select));
        assert!(matches!(kinds[1], Token::Star));
        assert!(matches!(kinds[2], Token::From));
    }

    #[test]
    fn tokenize_string_literal() {
        let sql = "'hello world'";
        let mut lex = tokenize(sql);
        let tok = lex.next().unwrap().unwrap().0;
        assert!(matches!(tok, Token::StringLit("'hello world'")));
    }

    #[test]
    fn tokenize_blob() {
        let sql = "X'DEADBEEF'";
        let mut lex = tokenize(sql);
        let tok = lex.next().unwrap().unwrap().0;
        assert!(matches!(tok, Token::Blob(_)));
    }

    #[test]
    fn skip_comments() {
        let sql = "-- this is a comment\nSELECT 1";
        let tokens: Vec<_> = tokenize(sql).collect();
        assert_eq!(tokens.len(), 2); // SELECT + 1
    }
}
