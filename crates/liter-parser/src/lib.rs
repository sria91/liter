//! SQL parser for Liter-rs.
//!
//! Mirrors the Lemon-generated LALR(1) parser from `parse.y`. Implemented as
//! a hand-written recursive-descent parser using `liter-tokenizer` for
//! lexing.

use liter_ast::*;
use liter_tokenizer::{tokenize, Token, TokenError};
use std::iter::Peekable;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("syntax error near '{0}'")]
    SyntaxError(String),
    #[error("unexpected end of input")]
    UnexpectedEof,
    #[error("not yet implemented")]
    NotImplemented,
    #[error("tokenizer error: {0}")]
    TokenError(#[from] TokenError),
}

pub type ParseResult<T> = Result<T, ParseError>;

type TokenIter<'a> =
    Box<dyn Iterator<Item = Result<(Token<'a>, std::ops::Range<usize>), TokenError>> + 'a>;

/// Recursive-descent SQL Parser
pub struct Parser<'a> {
    iter: Peekable<TokenIter<'a>>,
}

impl<'a> Parser<'a> {
    pub fn new(input: &'a str) -> Self {
        let iter: TokenIter<'a> = Box::new(tokenize(input));
        Self {
            iter: iter.peekable(),
        }
    }

    pub fn parse_all(&mut self) -> ParseResult<Vec<Stmt>> {
        let mut stmts = Vec::new();
        while self.peek()?.is_some() {
            stmts.push(self.parse_stmt()?);
            if let Some(Token::Semi) = self.peek()? {
                self.consume()?;
            }
        }
        Ok(stmts)
    }

    pub fn parse_stmt(&mut self) -> ParseResult<Stmt> {
        let tok = self.peek()?.cloned();
        match tok {
            Some(Token::Select) => Ok(Stmt::Select(Box::new(self.parse_select_stmt()?))),
            Some(Token::Create) => self.parse_create_stmt(),
            Some(Token::Insert) => self.parse_insert_stmt(),
            Some(Token::Update) => self.parse_update_stmt(),
            Some(Token::Delete) => self.parse_delete_stmt(),
            Some(Token::Begin) => self.parse_transaction_stmt(),
            Some(Token::Commit) => {
                self.consume()?;
                self.consume_optional(Token::Transaction);
                Ok(Stmt::Commit)
            }
            Some(Token::Rollback) => {
                self.consume()?;
                self.consume_optional(Token::Transaction);
                let savepoint = if let Some(Token::To) = self.peek()? {
                    self.consume()?;
                    self.consume_optional(Token::Savepoint);
                    Some(self.expect_ident()?)
                } else {
                    None
                };
                Ok(Stmt::Rollback { savepoint })
            }
            Some(Token::Savepoint) => {
                self.consume()?;
                Ok(Stmt::Savepoint(self.expect_ident()?))
            }
            Some(Token::Release) => {
                self.consume()?;
                self.consume_optional(Token::Savepoint);
                Ok(Stmt::Release(self.expect_ident()?))
            }
            Some(tok) => Err(ParseError::SyntaxError(format!(
                "Unexpected token starting statement: {:?}",
                tok
            ))),
            None => Err(ParseError::UnexpectedEof),
        }
    }

    fn parse_transaction_stmt(&mut self) -> ParseResult<Stmt> {
        self.expect(Token::Begin)?;
        let kind = match self.peek()? {
            Some(Token::Deferred) => {
                self.consume()?;
                TransactionKind::Deferred
            }
            Some(Token::Immediate) => {
                self.consume()?;
                TransactionKind::Immediate
            }
            Some(Token::Exclusive) => {
                self.consume()?;
                TransactionKind::Exclusive
            }
            _ => TransactionKind::Deferred,
        };
        self.consume_optional(Token::Transaction);
        Ok(Stmt::Begin(kind))
    }

    /// Consume the next token only if it matches `expected` (best-effort, no error).
    fn consume_optional(&mut self, expected: Token<'a>) {
        let _ = self.consume_if(expected);
    }

    /// Consume the next token if it matches `expected` and return true if consumed.
    fn consume_if(&mut self, expected: Token<'a>) -> Result<bool, ParseError> {
        if let Some(t) = self.peek()? {
            if *t == expected {
                self.consume()?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn parse_select_stmt(&mut self) -> ParseResult<SelectStmt> {
        self.expect(Token::Select)?;
        let distinct = match self.peek()? {
            Some(Token::Distinct) => {
                self.consume()?;
                DistinctKind::Distinct
            }
            Some(Token::All) => {
                self.consume()?;
                DistinctKind::All
            }
            _ => DistinctKind::All,
        };

        let mut result_columns = Vec::new();
        loop {
            if let Some(Token::Star) = self.peek()? {
                self.consume()?;
                result_columns.push(ResultColumn::Star);
            } else {
                let expr = self.parse_expr()?;
                let mut alias = None;
                if let Some(Token::As) = self.peek()? {
                    self.consume()?;
                    alias = Some(self.expect_ident()?);
                } else if let Some(Token::Ident(id)) = self.peek()? {
                    alias = Some(id.to_string());
                    self.consume()?;
                }
                result_columns.push(ResultColumn::Expr { expr, alias });
            }

            if let Some(Token::Comma) = self.peek()? {
                self.consume()?;
            } else {
                break;
            }
        }

        let mut from = None;
        if let Some(Token::From) = self.peek()? {
            self.consume()?;
            let mut tables = Vec::new();
            loop {
                let name = self.expect_ident()?;
                let mut alias = None;
                if let Some(Token::As) = self.peek()? {
                    self.consume()?;
                    alias = Some(self.expect_ident()?);
                } else if let Some(Token::Ident(id)) = self.peek()? {
                    alias = Some(id.to_string());
                    self.consume()?;
                }
                tables.push(TableOrSubquery::Table {
                    schema: None,
                    name,
                    alias,
                    indexed: IndexedKind::None,
                });

                if let Some(Token::Comma) = self.peek()? {
                    self.consume()?;
                } else {
                    break;
                }
            }
            from = Some(FromClause {
                tables,
                joins: vec![],
            });
        }

        let mut where_ = None;
        if let Some(Token::Where) = self.peek()? {
            self.consume()?;
            where_ = Some(self.parse_expr()?);
        }

        let mut group_by = vec![];
        if self.consume_if(Token::Group)? {
            self.expect(Token::By)?;
            loop {
                group_by.push(self.parse_expr()?);
                if !self.consume_if(Token::Comma)? {
                    break;
                }
            }
        }

        let mut having = None;
        if self.consume_if(Token::Having)? {
            having = Some(self.parse_expr()?);
        }

        let mut order_by = vec![];
        if self.consume_if(Token::Order)? {
            self.expect(Token::By)?;
            loop {
                let expr = self.parse_expr()?;
                let mut direction = SortDirection::Asc;
                if self.consume_if(Token::Asc)? {
                    direction = SortDirection::Asc;
                } else if self.consume_if(Token::Desc)? {
                    direction = SortDirection::Desc;
                }

                let mut nulls = NullsOrder::Default;
                if self.consume_if(Token::Nulls)? {
                    if self.consume_if(Token::First)? {
                        nulls = NullsOrder::First;
                    } else if self.consume_if(Token::Last)? {
                        nulls = NullsOrder::Last;
                    } else {
                        return Err(ParseError::SyntaxError(
                            "expected FIRST or LAST after NULLS".to_string(),
                        ));
                    }
                }

                order_by.push(OrderingTerm {
                    expr,
                    direction,
                    nulls,
                });

                if !self.consume_if(Token::Comma)? {
                    break;
                }
            }
        }

        let mut limit = None;
        if self.consume_if(Token::Limit)? {
            let limit_expr = self.parse_expr()?;
            if self.consume_if(Token::Offset)? {
                limit = Some(LimitClause {
                    limit: limit_expr,
                    offset: Some(self.parse_expr()?),
                });
            } else if self.consume_if(Token::Comma)? {
                // LIMIT offset, limit
                limit = Some(LimitClause {
                    limit: self.parse_expr()?,
                    offset: Some(limit_expr),
                });
            } else {
                limit = Some(LimitClause {
                    limit: limit_expr,
                    offset: None,
                });
            }
        }

        Ok(SelectStmt {
            with: None,
            body: SelectBody::Simple(SimpleSelect {
                distinct,
                result_columns,
                from,
                where_,
                group_by,
                having,
                window: vec![],
            }),
            order_by,
            limit,
        })
    }

    fn parse_create_stmt(&mut self) -> ParseResult<Stmt> {
        self.expect(Token::Create)?;
        let tok = self.peek()?.cloned();
        match tok {
            Some(Token::Table) => self.parse_create_table(),
            _ => Err(ParseError::NotImplemented),
        }
    }

    fn parse_create_table(&mut self) -> ParseResult<Stmt> {
        self.expect(Token::Table)?;
        let mut if_not_exists = false;
        if let Some(Token::If) = self.peek()? {
            self.consume()?;
            self.expect(Token::Not)?;
            self.expect(Token::Exists)?;
            if_not_exists = true;
        }

        let name = self.expect_ident()?;
        self.expect(Token::LParen)?;

        let mut columns = Vec::new();
        loop {
            let col_name = self.expect_ident()?;
            let mut type_name = None;

            // Check if the next token is an identifier for the type
            if let Some(Token::Ident(t)) = self.peek()? {
                type_name = Some(TypeName {
                    name: t.to_string(),
                    args: vec![],
                });
                self.consume()?;
            } else if let Some(Token::Integer(t)) = self.peek()? {
                // Hack for types like INT
                type_name = Some(TypeName {
                    name: t.to_string(),
                    args: vec![],
                });
                self.consume()?;
            }

            let mut constraints = Vec::new();
            loop {
                if let Some(Token::Primary) = self.peek()? {
                    self.consume()?;
                    self.expect(Token::Key)?;
                    let mut direction = None;
                    if self.consume_if(Token::Asc)? {
                        direction = Some(SortDirection::Asc);
                    } else if self.consume_if(Token::Desc)? {
                        direction = Some(SortDirection::Desc);
                    }

                    let conflict = None; // TODO: conflict clause
                    let autoincrement = self.consume_if(Token::Autoincrement)?;

                    constraints.push(ColumnConstraint::PrimaryKey {
                        direction,
                        conflict,
                        autoincrement,
                    });
                } else if let Some(Token::Not) = self.peek()? {
                    self.consume()?;
                    self.expect(Token::Null)?;
                    constraints.push(ColumnConstraint::NotNull { conflict: None });
                } else {
                    break;
                }
            }

            columns.push(ColumnDef {
                name: col_name,
                type_name,
                constraints,
            });

            if let Some(Token::Comma) = self.peek()? {
                self.consume()?;
            } else {
                break;
            }
        }
        self.expect(Token::RParen)?;

        Ok(Stmt::Create(Box::new(CreateStmt::Table(CreateTable {
            temp: false,
            if_not_exists,
            schema: None,
            name,
            body: CreateTableBody::Columns {
                columns,
                constraints: vec![],
            },
            options: TableOptions::default(),
        }))))
    }

    fn parse_insert_stmt(&mut self) -> ParseResult<Stmt> {
        self.expect(Token::Insert)?;
        self.expect(Token::Into)?;
        let table = self.expect_ident()?;

        let mut columns = Vec::new();
        if let Some(Token::LParen) = self.peek()? {
            self.consume()?;
            loop {
                columns.push(self.expect_ident()?);
                if let Some(Token::Comma) = self.peek()? {
                    self.consume()?;
                } else {
                    break;
                }
            }
            self.expect(Token::RParen)?;
        }

        self.expect(Token::Values)?;
        let mut values = Vec::new();
        loop {
            self.expect(Token::LParen)?;
            let mut row = Vec::new();
            loop {
                row.push(self.parse_expr()?);
                if let Some(Token::Comma) = self.peek()? {
                    self.consume()?;
                } else {
                    break;
                }
            }
            self.expect(Token::RParen)?;
            values.push(row);

            if let Some(Token::Comma) = self.peek()? {
                self.consume()?;
            } else {
                break;
            }
        }

        Ok(Stmt::Insert(Box::new(InsertStmt {
            with: None,
            or: None,
            schema: None,
            table,
            alias: None,
            columns,
            source: InsertSource::Values(values),
            returning: vec![],
        })))
    }

    fn parse_update_stmt(&mut self) -> ParseResult<Stmt> {
        self.expect(Token::Update)?;
        let table_name = self.expect_ident()?;
        let table = QualifiedTable {
            schema: None,
            name: table_name,
            alias: None,
            indexed: IndexedKind::None,
        };

        self.expect(Token::Set)?;
        let mut assignments = Vec::new();
        loop {
            let col = self.expect_ident()?;
            self.expect(Token::Eq)?;
            let value = self.parse_expr()?;
            assignments.push(Assignment {
                columns: vec![col],
                value,
            });

            if let Some(Token::Comma) = self.peek()? {
                self.consume()?;
            } else {
                break;
            }
        }

        let mut where_ = None;
        if let Some(Token::Where) = self.peek()? {
            self.consume()?;
            where_ = Some(self.parse_expr()?);
        }

        Ok(Stmt::Update(Box::new(UpdateStmt {
            with: None,
            or: None,
            table,
            assignments,
            from: None,
            where_,
            returning: vec![],
        })))
    }

    fn parse_delete_stmt(&mut self) -> ParseResult<Stmt> {
        self.expect(Token::Delete)?;
        self.expect(Token::From)?;
        let table_name = self.expect_ident()?;
        let table = QualifiedTable {
            schema: None,
            name: table_name,
            alias: None,
            indexed: IndexedKind::None,
        };

        let mut where_ = None;
        if let Some(Token::Where) = self.peek()? {
            self.consume()?;
            where_ = Some(self.parse_expr()?);
        }

        Ok(Stmt::Delete(Box::new(DeleteStmt {
            with: None,
            table,
            where_,
            returning: vec![],
        })))
    }

    fn parse_expr(&mut self) -> ParseResult<Expr> {
        self.parse_expr_bp(0)
    }

    fn parse_expr_bp(&mut self, min_bp: u8) -> ParseResult<Expr> {
        let mut lhs = self.parse_primary()?;

        while let Some(tok) = self.peek()?.cloned() {
            let (l_bp, r_bp) = self.infix_binding_power(&tok);
            if l_bp == 0 || l_bp < min_bp {
                break;
            }
            self.consume()?;
            let op = self.token_to_binary_op(&tok).unwrap();
            let rhs = self.parse_expr_bp(r_bp)?;
            lhs = Expr::Binary {
                op,
                left: Box::new(lhs),
                right: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_primary(&mut self) -> ParseResult<Expr> {
        let tok = self.peek()?.cloned();
        match tok {
            Some(Token::Null) => {
                self.consume()?;
                Ok(Expr::Literal(LiteralValue::Null))
            }
            Some(Token::Integer(s)) => {
                self.consume()?;
                Ok(Expr::Literal(LiteralValue::Integer(s.parse().unwrap_or(0))))
            }
            Some(Token::Float(s)) => {
                self.consume()?;
                Ok(Expr::Literal(LiteralValue::Float(s.parse().unwrap_or(0.0))))
            }
            Some(Token::StringLit(s)) => {
                self.consume()?;
                let val = &s[1..s.len() - 1];
                Ok(Expr::Literal(LiteralValue::Text(val.to_string())))
            }
            Some(Token::Ident(id)) => {
                self.consume()?;
                if self.consume_if(Token::LParen)? {
                    let args;
                    if self.consume_if(Token::RParen)? {
                        args = FunctionArgs::None;
                    } else if self.consume_if(Token::Star)? {
                        self.expect(Token::RParen)?;
                        args = FunctionArgs::Star;
                    } else {
                        let mut exprs = vec![];
                        loop {
                            exprs.push(self.parse_expr()?);
                            if !self.consume_if(Token::Comma)? {
                                break;
                            }
                        }
                        self.expect(Token::RParen)?;
                        args = FunctionArgs::List(exprs);
                    }
                    Ok(Expr::Function {
                        schema: None,
                        name: id.to_string(),
                        args,
                        filter: None,
                        over: None,
                    })
                } else {
                    Ok(Expr::Column {
                        schema: None,
                        table: None,
                        name: id.to_string(),
                    })
                }
            }
            Some(Token::LParen) => {
                self.consume()?;
                let expr = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            Some(tok) => Err(ParseError::SyntaxError(format!("{:?}", tok))),
            None => Err(ParseError::UnexpectedEof),
        }
    }

    fn infix_binding_power(&self, tok: &Token) -> (u8, u8) {
        match tok {
            Token::Or => (1, 2),
            Token::And => (3, 4),
            Token::Eq
            | Token::EqEq
            | Token::Ne
            | Token::BangEq
            | Token::Lt
            | Token::Le
            | Token::Gt
            | Token::Ge => (5, 6),
            Token::Plus | Token::Minus => (9, 10),
            Token::Star | Token::Slash | Token::Percent => (11, 12),
            _ => (0, 0),
        }
    }

    fn token_to_binary_op(&self, tok: &Token) -> Option<BinaryOp> {
        match tok {
            Token::Eq | Token::EqEq => Some(BinaryOp::Eq),
            Token::Ne | Token::BangEq => Some(BinaryOp::Ne),
            Token::Lt => Some(BinaryOp::Lt),
            Token::Le => Some(BinaryOp::Le),
            Token::Gt => Some(BinaryOp::Gt),
            Token::Ge => Some(BinaryOp::Ge),
            Token::Plus => Some(BinaryOp::Add),
            Token::Minus => Some(BinaryOp::Sub),
            Token::Star => Some(BinaryOp::Mul),
            Token::Slash => Some(BinaryOp::Div),
            Token::Percent => Some(BinaryOp::Mod),
            Token::And => Some(BinaryOp::And),
            Token::Or => Some(BinaryOp::Or),
            _ => None,
        }
    }

    fn peek(&mut self) -> ParseResult<Option<&Token<'a>>> {
        match self.iter.peek() {
            Some(Ok((tok, _))) => Ok(Some(tok)),
            Some(Err(e)) => Err(ParseError::TokenError(e.clone())),
            None => Ok(None),
        }
    }

    fn consume(&mut self) -> ParseResult<Option<Token<'a>>> {
        match self.iter.next() {
            Some(Ok((tok, _))) => Ok(Some(tok)),
            Some(Err(e)) => Err(ParseError::TokenError(e)),
            None => Ok(None),
        }
    }

    fn expect(&mut self, expected: Token<'a>) -> ParseResult<()> {
        match self.consume()? {
            Some(tok) if tok == expected => Ok(()),
            Some(tok) => Err(ParseError::SyntaxError(format!("{:?}", tok))),
            None => Err(ParseError::UnexpectedEof),
        }
    }

    fn expect_ident(&mut self) -> ParseResult<String> {
        match self.consume()? {
            Some(Token::Ident(id)) => {
                if id.starts_with('"') || id.starts_with('`') || id.starts_with('[') {
                    Ok(id[1..id.len() - 1].to_string())
                } else {
                    Ok(id.to_string())
                }
            }
            Some(tok) => Err(ParseError::SyntaxError(format!("{:?}", tok))),
            None => Err(ParseError::UnexpectedEof),
        }
    }
}

pub fn parse_stmt(input: &str) -> ParseResult<Stmt> {
    let mut parser = Parser::new(input);
    parser.parse_stmt()
}

pub fn parse_all(input: &str) -> ParseResult<Vec<Stmt>> {
    let mut parser = Parser::new(input);
    parser.parse_all()
}
