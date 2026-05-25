use liter_ast::*;
use liter_parser::{parse_all, parse_stmt, ParseError};

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
