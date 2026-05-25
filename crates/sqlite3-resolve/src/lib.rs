//! Name resolution and type affinity for SQLite3-rs.
//!
//! Mirrors `resolve.c`. Walks the AST and binds each column reference to its
//! source table/expression; computes type affinity for expressions.

use sqlite3_ast::*;
use sqlite3_schema::Schema;

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

pub struct Resolver<'a> {
    schema: &'a Schema,
}

impl<'a> Resolver<'a> {
    pub fn new(schema: &'a Schema) -> Self {
        Self { schema }
    }

    pub fn resolve_stmt(&self, stmt: &mut Stmt) -> ResolveResult<()> {
        match stmt {
            Stmt::Select(select) => self.resolve_select(select),
            _ => Ok(()), // TODO support other statements
        }
    }

    fn resolve_select(&self, select: &mut SelectStmt) -> ResolveResult<()> {
        let body = match &mut select.body {
            SelectBody::Simple(simple) => simple,
            _ => return Err(ResolveError::NotImplemented),
        };

        // 1. Resolve FROM clause and verify tables exist
        let mut available_tables = Vec::new();
        if let Some(from) = &body.from {
            for table_or_subquery in &from.tables {
                match table_or_subquery {
                    TableOrSubquery::Table { name, alias, .. } => {
                        let obj = self.schema.get(name).ok_or_else(|| ResolveError::NoSuchTable(name.clone()))?;
                        available_tables.push((alias.clone().unwrap_or_else(|| name.clone()), obj));
                    }
                    _ => return Err(ResolveError::NotImplemented),
                }
            }
            // TODO handle joins
        }

        // 2. Expand SELECT * and resolve column references
        let mut new_columns = Vec::new();
        for col in &mut body.result_columns {
            match col {
                ResultColumn::Star => {
                    for (table_alias, obj) in &available_tables {
                        for c in &obj.columns {
                            new_columns.push(ResultColumn::Expr {
                                expr: Expr::Column {
                                    schema: None,
                                    table: Some(table_alias.clone()),
                                    name: c.name.clone(),
                                },
                                alias: None,
                            });
                        }
                    }
                }
                ResultColumn::TableStar(table_name) => {
                    let mut found = false;
                    for (table_alias, obj) in &available_tables {
                        if table_alias == table_name {
                            found = true;
                            for c in &obj.columns {
                                new_columns.push(ResultColumn::Expr {
                                    expr: Expr::Column {
                                        schema: None,
                                        table: Some(table_alias.clone()),
                                        name: c.name.clone(),
                                    },
                                    alias: None,
                                });
                            }
                        }
                    }
                    if !found {
                        return Err(ResolveError::NoSuchTable(table_name.clone()));
                    }
                }
                ResultColumn::Expr { expr, .. } => {
                    self.resolve_expr(expr, &available_tables)?;
                    new_columns.push(col.clone());
                }
            }
        }
        body.result_columns = new_columns;

        // 3. Resolve WHERE clause
        if let Some(where_) = &mut body.where_ {
            self.resolve_expr(where_, &available_tables)?;
        }

        Ok(())
    }

    fn resolve_expr(&self, expr: &mut Expr, available_tables: &[(String, sqlite3_schema::SchemaObject)]) -> ResolveResult<()> {
        match expr {
            Expr::Column { table, name, .. } => {
                let mut matches = 0;
                let mut resolved_table = None;

                if let Some(t_name) = table {
                    for (alias, obj) in available_tables {
                        if alias == t_name && obj.columns.iter().any(|c| &c.name == name) {
                            matches += 1;
                            resolved_table = Some(alias.clone());
                        }
                    }
                } else {
                    for (alias, obj) in available_tables {
                        if obj.columns.iter().any(|c| &c.name == name) {
                            matches += 1;
                            resolved_table = Some(alias.clone());
                        }
                    }
                }

                if matches == 0 {
                    return Err(ResolveError::NoSuchColumn(name.clone()));
                } else if matches > 1 {
                    return Err(ResolveError::AmbiguousColumn(name.clone()));
                }

                *table = resolved_table;
                Ok(())
            }
            Expr::Binary { left, right, .. } => {
                self.resolve_expr(left, available_tables)?;
                self.resolve_expr(right, available_tables)?;
                Ok(())
            }
            Expr::Literal(_) => Ok(()),
            _ => Err(ResolveError::NotImplemented),
        }
    }
}
