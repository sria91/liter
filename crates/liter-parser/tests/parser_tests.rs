use liter_ast::*;
use liter_parser::{parse_all, parse_stmt, ParseError, Parser};
use liter_tokenizer::Token;

#[test]
fn parse_select_basic() {
    let sql = "SELECT * FROM users;";
    let stmts = parse_all(sql).unwrap();
    assert_eq!(stmts.len(), 1);

    if let Stmt::Select(select) = &stmts[0] {
        if let SelectBody::Simple(simple) = &select.body {
            assert_eq!(simple.result_columns.len(), 1);
            assert!(matches!(simple.result_columns[0], ResultColumn::Star));

            let from = simple.from.as_ref().unwrap();
            assert_eq!(from.tables.len(), 1);
            if let TableOrSubquery::Table { name, .. } = &from.tables[0] {
                assert_eq!(name, "users");
            } else {
                panic!("Expected Table");
            }
        } else {
            panic!("Expected Simple select");
        }
    } else {
        panic!("Expected Select statement");
    }
}

#[test]
fn parse_select_with_where_and_alias() {
    let sql = "SELECT id as user_id, name FROM users WHERE id = 1";
    let stmt = parse_stmt(sql).unwrap();

    if let Stmt::Select(select) = &stmt {
        if let SelectBody::Simple(simple) = &select.body {
            assert_eq!(simple.result_columns.len(), 2);
            assert!(matches!(simple.where_, Some(Expr::Binary { .. })));
        }
    } else {
        panic!("Expected Select statement");
    }
}

#[test]
fn parse_create_table() {
    let sql = "CREATE TABLE IF NOT EXISTS users (id INT, name TEXT);";
    let stmts = parse_all(sql).unwrap();
    assert_eq!(stmts.len(), 1);

    if let Stmt::Create(create) = &stmts[0] {
        if let CreateStmt::Table(table) = create.as_ref() {
            assert_eq!(table.name, "users");
            assert!(table.if_not_exists);

            if let CreateTableBody::Columns { columns, .. } = &table.body {
                assert_eq!(columns.len(), 2);
                assert_eq!(columns[0].name, "id");
                assert_eq!(columns[1].name, "name");
            }
        } else {
            panic!("Expected CreateStmt::Table");
        }
    } else {
        panic!("Expected Create statement");
    }
}

#[test]
fn parse_insert() {
    let sql = "INSERT INTO users (id, name) VALUES (1, 'Alice'), (2, 'Bob');";
    let stmts = parse_all(sql).unwrap();
    assert_eq!(stmts.len(), 1);

    if let Stmt::Insert(insert) = &stmts[0] {
        assert_eq!(insert.table, "users");
        assert_eq!(insert.columns.len(), 2);

        if let InsertSource::Values(values) = &insert.source {
            assert_eq!(values.len(), 2);
            assert_eq!(values[0].len(), 2); // 1, 'Alice'
            assert_eq!(values[1].len(), 2); // 2, 'Bob'
        } else {
            panic!("Expected InsertSource::Values");
        }
    } else {
        panic!("Expected Insert statement");
    }
}

#[test]
fn parse_update() {
    let sql = "UPDATE users SET name = 'Charlie', age = 30 WHERE id = 3;";
    let stmts = parse_all(sql).unwrap();
    assert_eq!(stmts.len(), 1);

    if let Stmt::Update(update) = &stmts[0] {
        assert_eq!(update.table.name, "users");
        assert_eq!(update.assignments.len(), 2);
        assert_eq!(update.assignments[0].columns[0], "name");
        assert_eq!(update.assignments[1].columns[0], "age");
        assert!(matches!(update.where_, Some(Expr::Binary { .. })));
    } else {
        panic!("Expected Update statement");
    }
}

#[test]
fn parse_delete() {
    let sql = "DELETE FROM users WHERE id = 3;";
    let stmts = parse_all(sql).unwrap();
    assert_eq!(stmts.len(), 1);

    if let Stmt::Delete(delete) = &stmts[0] {
        assert_eq!(delete.table.name, "users");
        assert!(matches!(delete.where_, Some(Expr::Binary { .. })));
    } else {
        panic!("Expected Delete statement");
    }
}

#[test]
fn parse_transactions_and_savepoints() {
    let cases = vec![
        ("BEGIN;", Stmt::Begin(TransactionKind::Deferred)),
        (
            "BEGIN DEFERRED TRANSACTION;",
            Stmt::Begin(TransactionKind::Deferred),
        ),
        ("BEGIN IMMEDIATE;", Stmt::Begin(TransactionKind::Immediate)),
        (
            "BEGIN EXCLUSIVE TRANSACTION;",
            Stmt::Begin(TransactionKind::Exclusive),
        ),
        ("COMMIT;", Stmt::Commit),
        ("COMMIT TRANSACTION;", Stmt::Commit),
        ("ROLLBACK;", Stmt::Rollback { savepoint: None }),
        ("ROLLBACK TRANSACTION;", Stmt::Rollback { savepoint: None }),
        (
            "ROLLBACK TO sp1;",
            Stmt::Rollback {
                savepoint: Some("sp1".to_string()),
            },
        ),
        (
            "ROLLBACK TRANSACTION TO SAVEPOINT sp2;",
            Stmt::Rollback {
                savepoint: Some("sp2".to_string()),
            },
        ),
        ("SAVEPOINT sp3;", Stmt::Savepoint("sp3".to_string())),
        ("RELEASE sp4;", Stmt::Release("sp4".to_string())),
        ("RELEASE SAVEPOINT sp5;", Stmt::Release("sp5".to_string())),
    ];

    for (sql, expected) in cases {
        let stmt = parse_stmt(sql).unwrap();
        assert_eq!(stmt, expected, "Failed for SQL: {sql}");
    }
}

#[test]
fn parse_select_comprehensive() {
    // SELECT DISTINCT / ALL, aliases without AS, multiple FROM tables with aliases
    let sql = "SELECT DISTINCT a as col1, b col2, NULL, 3.14, * FROM t1 as table1, `t2` table2 WHERE a > 10 AND b < 5 AND c >= 2 GROUP BY a, b HAVING count(*) > 1 ORDER BY a ASC NULLS FIRST, b DESC NULLS LAST LIMIT 10 OFFSET 5;";
    let stmts = parse_all(sql).unwrap();
    assert_eq!(stmts.len(), 1);

    let sql2 = "SELECT ALL 1 + 2 * 3 / 4 % 5, 10 - 2, (a == b) AND (c != d) OR (e <= f) FROM [t3] LIMIT 5, 10";
    let stmts2 = parse_all(sql2).unwrap();
    assert_eq!(stmts2.len(), 1);

    let sql3 = "SELECT foo(), bar(x, y), count(*) FROM \"t4\" LIMIT 5";
    let stmts3 = parse_all(sql3).unwrap();
    assert_eq!(stmts3.len(), 1);
}

#[test]
fn parse_select_orderby_default() {
    let sql = "SELECT a FROM t1 ORDER BY a;";
    let stmts = parse_all(sql).unwrap();
    assert_eq!(stmts.len(), 1);

    if let Stmt::Select(select) = &stmts[0] {
        assert_eq!(select.order_by.len(), 1);
        assert_eq!(select.order_by[0].nulls, NullsOrder::Default);
    } else {
        panic!("Expected Select statement");
    }
}

#[test]
fn parse_create_table_extended() {
    let sql = "CREATE TABLE users (id INTEGER PRIMARY KEY ASC AUTOINCREMENT, age 123 PRIMARY KEY DESC, name TEXT NOT NULL, score FLOAT);";
    let stmts = parse_all(sql).unwrap();
    assert_eq!(stmts.len(), 1);
}

#[test]
fn parse_insert_update_delete_variants() {
    let sql1 = "INSERT INTO users VALUES (1, 'Alice');";
    let stmts1 = parse_all(sql1).unwrap();
    assert_eq!(stmts1.len(), 1);

    let sql2 = "UPDATE users SET name = 'Bob';";
    let stmts2 = parse_all(sql2).unwrap();
    assert_eq!(stmts2.len(), 1);

    let sql3 = "DELETE FROM users;";
    let stmts3 = parse_all(sql3).unwrap();
    assert_eq!(stmts3.len(), 1);
}

#[test]
fn parse_errors() {
    // EOF error
    assert!(matches!(parse_stmt(""), Err(ParseError::UnexpectedEof)));
    assert!(matches!(
        parse_stmt("SELECT"),
        Err(ParseError::UnexpectedEof)
    ));
    assert!(matches!(
        parse_stmt("BEGIN DEFERRED"),
        Ok(Stmt::Begin(TransactionKind::Deferred))
    ));

    // Syntax errors
    assert!(matches!(
        parse_stmt("SELECT 1 FROM t ORDER BY a NULLS FOO"),
        Err(ParseError::SyntaxError(_))
    ));
    assert!(matches!(
        parse_stmt("CREATE INDEX idx ON t(a)"),
        Err(ParseError::NotImplemented)
    ));
    assert!(matches!(
        parse_stmt("FOOBAR"),
        Err(ParseError::SyntaxError(_))
    ));
    assert!(matches!(
        parse_stmt("SELECT (1 + 2"),
        Err(ParseError::UnexpectedEof)
    ));
    assert!(matches!(
        parse_stmt("SELECT (1 + 2 +)"),
        Err(ParseError::SyntaxError(_))
    ));
    assert!(matches!(
        parse_stmt("INSERT INTO"),
        Err(ParseError::UnexpectedEof)
    ));
    assert!(matches!(
        parse_stmt("INSERT INTO 123"),
        Err(ParseError::SyntaxError(_))
    ));
    assert!(matches!(
        parse_stmt("CREATE TABLE"),
        Err(ParseError::UnexpectedEof)
    ));
    assert!(matches!(
        parse_stmt("CREATE TABLE t (id INT;"),
        Err(ParseError::SyntaxError(_))
    ));
    assert!(matches!(
        parse_stmt("UPDATE 123"),
        Err(ParseError::SyntaxError(_))
    ));
    assert!(matches!(
        parse_stmt("DELETE FROM 123"),
        Err(ParseError::SyntaxError(_))
    ));

    // Tokenizer / Lexer errors
    assert!(matches!(
        parse_stmt("SELECT 'unclosed string"),
        Err(ParseError::TokenError(_))
    ));
}

#[test]
fn syntax_error_propagation() {
    let sql = "SELECT * FORM users"; // intentional typo 'FORM'
    let result = parse_all(sql);
    assert!(result.is_err());

    if let Err(ParseError::SyntaxError(msg)) = result {
        assert!(msg.contains("Ident(\"FORM\")"));
    } else {
        panic!("Expected SyntaxError");
    }
}

#[test]
fn test_internal_parser_methods() {
    let parser = Parser::new("SELECT");
    assert_eq!(parser.token_to_binary_op(&Token::Select), None);
    assert_eq!(parser.token_to_binary_op(&Token::Plus), Some(BinaryOp::Add));

    // Test consume on tokenizer error
    let mut err_parser = Parser::new("'unclosed");
    assert!(matches!(
        err_parser.consume(),
        Err(ParseError::TokenError(_))
    ));

    // Test expect with wrong token and EOF
    let mut p = Parser::new("123");
    assert!(matches!(
        p.expect(Token::Select),
        Err(ParseError::SyntaxError(_))
    ));
    let mut p_eof = Parser::new("");
    assert!(matches!(
        p_eof.expect(Token::Select),
        Err(ParseError::UnexpectedEof)
    ));

    // Test expect_ident
    let mut p_id = Parser::new("123");
    assert!(matches!(
        p_id.expect_ident(),
        Err(ParseError::SyntaxError(_))
    ));
    let mut p_id_eof = Parser::new("");
    assert!(matches!(
        p_id_eof.expect_ident(),
        Err(ParseError::UnexpectedEof)
    ));
}

#[test]
fn test_create_table_column_types() {
    // Test column with no type, ident type, and integer type
    let sql = "CREATE TABLE t (col1, col2 TEXT, col3 100);";
    let stmt = parse_stmt(sql).unwrap();
    let Stmt::Create(c) = stmt else {
        panic!("expected Create")
    };
    let CreateStmt::Table(t) = *c else {
        panic!("expected Table")
    };
    let CreateTableBody::Columns { columns, .. } = t.body else {
        panic!("expected Columns")
    };
    assert_eq!(columns.len(), 3);
    assert!(columns[0].type_name.is_none());
    assert_eq!(columns[1].type_name.as_ref().unwrap().name, "TEXT");
    assert_eq!(columns[2].type_name.as_ref().unwrap().name, "100");
}
