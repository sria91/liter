//! VDBE bytecode generator for SQLite3-rs.
//!
//! Mirrors `select.c`, `insert.c`, `delete.c`, `update.c`, `trigger.c`.
//! Transforms a resolved + planned AST into a `Vdbe` program.
//!
//! ## Status
//! Phase 3 — stub.

use sqlite3_ast::Stmt;
use sqlite3_vdbe::Vdbe;

#[derive(Debug, thiserror::Error)]
pub enum CodegenError {
    #[error("not yet implemented")]
    NotImplemented,
    #[error("internal codegen error: {0}")]
    Internal(String),
}

pub type CodegenResult<T> = Result<T, CodegenError>;

/// Compile a resolved AST statement into a VDBE program.
pub fn compile(stmt: &Stmt) -> CodegenResult<Vdbe> {
    let _ = stmt;
    Err(CodegenError::NotImplemented)
}
