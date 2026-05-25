use liter_parser::parse_all;
use liter_schema::{Schema, SchemaObject, ObjectKind};
use liter_ast::{ColumnDef, Stmt, ResultColumn};
use liter_resolve::{Resolver, ResolveError};

fn make_schema() -> Schema {
    let schema = Schema::new();
    schema.insert(SchemaObject {
        kind: ObjectKind::Table,
        name: "users".to_string(),
        tbl_name: "users".to_string(),
        root_page: 2,
        sql: None,
        columns: vec![
            ColumnDef { name: "id".to_string(), type_name: None, constraints: vec![] },
            ColumnDef { name: "name".to_string(), type_name: None, constraints: vec![] },
            ColumnDef { name: "age".to_string(), type_name: None, constraints: vec![] },
        ],
    });
    schema.insert(SchemaObject {
        kind: ObjectKind::Table,
        name: "posts".to_string(),
        tbl_name: "posts".to_string(),
        root_page: 3,
        sql: None,
        columns: vec![
            ColumnDef { name: "id".to_string(), type_name: None, constraints: vec![] },
            ColumnDef { name: "author_id".to_string(), type_name: None, constraints: vec![] },
            ColumnDef { name: "content".to_string(), type_name: None, constraints: vec![] },
        ],
    });
    schema
}

#[test]
fn test_resolve_success() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);

    let sql = "SELECT id, name FROM users WHERE age > 18;";
    let mut stmts = parse_all(sql).unwrap();
    
    assert!(resolver.resolve_stmt(&mut stmts[0]).is_ok());
}

#[test]
fn test_resolve_no_such_table() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);

    let sql = "SELECT id FROM missing_table;";
    let mut stmts = parse_all(sql).unwrap();
    
    let res = resolver.resolve_stmt(&mut stmts[0]);
    assert!(matches!(res, Err(ResolveError::NoSuchTable(t)) if t == "missing_table"));
}

#[test]
fn test_resolve_no_such_column() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);

    let sql = "SELECT bogus FROM users;";
    let mut stmts = parse_all(sql).unwrap();
    
    let res = resolver.resolve_stmt(&mut stmts[0]);
    assert!(matches!(res, Err(ResolveError::NoSuchColumn(c)) if c == "bogus"));
}

#[test]
fn test_resolve_ambiguous_column() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);

    // Both users and posts have an 'id' column
    let sql = "SELECT id FROM users, posts;";
    let mut stmts = parse_all(sql).unwrap();
    
    let res = resolver.resolve_stmt(&mut stmts[0]);
    assert!(matches!(res, Err(ResolveError::AmbiguousColumn(c)) if c == "id"));
}

#[test]
fn test_resolve_star_expansion() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);

    let sql = "SELECT * FROM users;";
    let mut stmts = parse_all(sql).unwrap();
    
    assert!(resolver.resolve_stmt(&mut stmts[0]).is_ok());

    if let Stmt::Select(select) = &stmts[0] {
        if let liter_ast::SelectBody::Simple(simple) = &select.body {
            assert_eq!(simple.result_columns.len(), 3);
            
            // Check that they expanded correctly
            let names: Vec<String> = simple.result_columns.iter().filter_map(|c| {
                if let ResultColumn::Expr { expr: liter_ast::Expr::Column { name, table, .. }, .. } = c {
                    assert_eq!(table.as_deref(), Some("users"));
                    Some(name.clone())
                } else {
                    None
                }
            }).collect();
            
            assert_eq!(names, vec!["id", "name", "age"]);
        } else {
            panic!("Expected simple select");
        }
    } else {
        panic!("Expected select stmt");
    }
}
