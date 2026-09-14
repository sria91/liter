use liter_ast::*;
use liter_parser::parse_all;
use liter_resolve::{Affinity, ResolveError, Resolver};
use liter_schema::{ObjectKind, Schema, SchemaObject};

fn make_schema() -> Schema {
    let schema = Schema::new();
    schema.insert(SchemaObject {
        kind: ObjectKind::Table,
        name: "users".to_string(),
        tbl_name: "users".to_string(),
        root_page: 2,
        sql: None,
        columns: vec![
            ColumnDef {
                name: "id".to_string(),
                type_name: None,
                constraints: vec![],
            },
            ColumnDef {
                name: "name".to_string(),
                type_name: None,
                constraints: vec![],
            },
            ColumnDef {
                name: "age".to_string(),
                type_name: None,
                constraints: vec![],
            },
        ],
    });
    schema.insert(SchemaObject {
        kind: ObjectKind::Table,
        name: "posts".to_string(),
        tbl_name: "posts".to_string(),
        root_page: 3,
        sql: None,
        columns: vec![
            ColumnDef {
                name: "id".to_string(),
                type_name: None,
                constraints: vec![],
            },
            ColumnDef {
                name: "author_id".to_string(),
                type_name: None,
                constraints: vec![],
            },
            ColumnDef {
                name: "content".to_string(),
                type_name: None,
                constraints: vec![],
            },
        ],
    });
    schema
}

fn simple_select(body: SimpleSelect) -> SelectStmt {
    SelectStmt {
        with: None,
        body: SelectBody::Simple(body),
        order_by: vec![],
        limit: None,
    }
}

fn empty_simple_select() -> SimpleSelect {
    SimpleSelect {
        distinct: DistinctKind::All,
        result_columns: vec![],
        from: None,
        where_: None,
        group_by: vec![],
        having: None,
        window: vec![],
    }
}

#[test]
fn test_affinity_from_type_name_covers_all_branches() {
    assert_eq!(Affinity::from_type_name("INTEGER"), Affinity::Integer);
    assert_eq!(Affinity::from_type_name("TINYINT"), Affinity::Integer);
    assert_eq!(Affinity::from_type_name("VARCHAR(10)"), Affinity::Text);
    assert_eq!(Affinity::from_type_name("CHARACTER"), Affinity::Text);
    assert_eq!(Affinity::from_type_name("CLOB"), Affinity::Text);
    assert_eq!(Affinity::from_type_name("TEXT"), Affinity::Text);
    assert_eq!(Affinity::from_type_name("BLOB"), Affinity::Blob);
    assert_eq!(Affinity::from_type_name(""), Affinity::Blob);
    assert_eq!(Affinity::from_type_name("REAL"), Affinity::Real);
    assert_eq!(Affinity::from_type_name("FLOAT"), Affinity::Real);
    assert_eq!(Affinity::from_type_name("DOUBLE PRECISION"), Affinity::Real);
    assert_eq!(Affinity::from_type_name("NUMERIC"), Affinity::Numeric);
    assert_eq!(Affinity::from_type_name("DECIMAL"), Affinity::Numeric);
    assert_eq!(Affinity::from_type_name("BOOLEAN"), Affinity::Numeric);
}

#[test]
fn test_resolve_stmt_non_select_is_noop() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);
    let mut stmt = Stmt::Delete(Box::new(DeleteStmt {
        with: None,
        table: QualifiedTable {
            schema: None,
            name: "users".to_string(),
            alias: None,
            indexed: IndexedKind::None,
        },
        where_: None,
        returning: vec![],
    }));
    assert!(resolver.resolve_stmt(&mut stmt).is_ok());
}

#[test]
fn test_resolve_select_compound_body_not_implemented() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);
    let mut select = SelectStmt {
        with: None,
        body: SelectBody::Compound {
            op: CompoundOp::Union,
            left: Box::new(SelectBody::Simple(empty_simple_select())),
            right: Box::new(SelectBody::Simple(empty_simple_select())),
        },
        order_by: vec![],
        limit: None,
    };
    let res = resolver.resolve_select(&mut select);
    assert!(matches!(res, Err(ResolveError::NotImplemented)));
}

#[test]
fn test_resolve_select_without_from_clause_succeeds() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);
    let mut select = simple_select(empty_simple_select());
    assert!(resolver.resolve_select(&mut select).is_ok());
}

#[test]
fn test_resolve_select_subquery_in_from_not_implemented() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);
    let subquery = simple_select(empty_simple_select());
    let mut body = empty_simple_select();
    body.from = Some(FromClause {
        tables: vec![TableOrSubquery::Subquery {
            select: Box::new(subquery),
            alias: Some("sub".to_string()),
        }],
        joins: vec![],
    });
    let mut select = simple_select(body);
    let res = resolver.resolve_select(&mut select);
    assert!(matches!(res, Err(ResolveError::NotImplemented)));
}

#[test]
fn test_resolve_select_table_star_found_and_not_found() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);

    let mut found_body = empty_simple_select();
    found_body.result_columns = vec![ResultColumn::TableStar("users".to_string())];
    found_body.from = Some(FromClause {
        tables: vec![TableOrSubquery::Table {
            schema: None,
            name: "users".to_string(),
            alias: None,
            indexed: IndexedKind::None,
        }],
        joins: vec![],
    });
    let mut found_select = simple_select(found_body);
    assert!(resolver.resolve_select(&mut found_select).is_ok());

    let mut missing_body = empty_simple_select();
    missing_body.result_columns = vec![ResultColumn::TableStar("bogus".to_string())];
    missing_body.from = Some(FromClause {
        tables: vec![TableOrSubquery::Table {
            schema: None,
            name: "users".to_string(),
            alias: None,
            indexed: IndexedKind::None,
        }],
        joins: vec![],
    });
    let mut missing_select = simple_select(missing_body);
    let res = resolver.resolve_select(&mut missing_select);
    assert!(matches!(res, Err(ResolveError::NoSuchTable(t)) if t == "bogus"));
}

#[test]
fn test_resolve_where_clause_and_binary() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);

    // Success with WHERE
    let sql = "SELECT id FROM users WHERE id = 1 AND age > 20;";
    let mut stmts = parse_all(sql).unwrap();
    assert!(resolver.resolve_stmt(&mut stmts[0]).is_ok());

    // Error in WHERE left
    let sql_err_left = "SELECT id FROM users WHERE nonexistent = 1;";
    let mut stmts_err_l = parse_all(sql_err_left).unwrap();
    assert!(matches!(
        resolver.resolve_stmt(&mut stmts_err_l[0]),
        Err(ResolveError::NoSuchColumn(c)) if c == "nonexistent"
    ));

    // Error in WHERE right
    let sql_err_right = "SELECT id FROM users WHERE id = nonexistent;";
    let mut stmts_err_r = parse_all(sql_err_right).unwrap();
    assert!(matches!(
        resolver.resolve_stmt(&mut stmts_err_r[0]),
        Err(ResolveError::NoSuchColumn(c)) if c == "nonexistent"
    ));

    // Error in result column
    let sql_col_err = "SELECT nonexistent FROM users;";
    let mut stmts_col = parse_all(sql_col_err).unwrap();
    assert!(matches!(
        resolver.resolve_stmt(&mut stmts_col[0]),
        Err(ResolveError::NoSuchColumn(c)) if c == "nonexistent"
    ));
}

#[test]
fn test_resolve_qualified_columns() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);
    let obj = schema.get("users").unwrap();
    let posts = schema.get("posts").unwrap();
    let tables = vec![("users".to_string(), obj), ("posts".to_string(), posts)];

    // Qualified match
    let mut expr = Expr::Column {
        schema: None,
        table: Some("users".to_string()),
        name: "name".to_string(),
    };
    assert!(resolver.resolve_expr(&mut expr, &tables).is_ok());

    // Qualified non-matching table / column
    let mut expr_bad_table = Expr::Column {
        schema: None,
        table: Some("other".to_string()),
        name: "name".to_string(),
    };
    assert!(matches!(
        resolver.resolve_expr(&mut expr_bad_table, &tables),
        Err(ResolveError::NoSuchColumn(_))
    ));

    let mut expr_bad_col = Expr::Column {
        schema: None,
        table: Some("users".to_string()),
        name: "nonexistent".to_string(),
    };
    assert!(matches!(
        resolver.resolve_expr(&mut expr_bad_col, &tables),
        Err(ResolveError::NoSuchColumn(_))
    ));
}

#[test]
fn test_resolve_expr_unsupported() {
    let schema = make_schema();
    let resolver = Resolver::new(&schema);
    let obj = schema.get("users").unwrap();
    let tables = vec![("users".to_string(), obj)];

    let mut expr = Expr::Unary {
        op: UnaryOp::Minus,
        operand: Box::new(Expr::Literal(LiteralValue::Integer(1))),
    };
    assert!(matches!(
        resolver.resolve_expr(&mut expr, &tables),
        Err(ResolveError::NotImplemented)
    ));
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
            let names: Vec<String> = simple
                .result_columns
                .iter()
                .filter_map(|c| {
                    if let ResultColumn::Expr {
                        expr: liter_ast::Expr::Column { name, table, .. },
                        ..
                    } = c
                    {
                        assert_eq!(table.as_deref(), Some("users"));
                        Some(name.clone())
                    } else {
                        None
                    }
                })
                .collect();

            assert_eq!(names, vec!["id", "name", "age"]);
        } else {
            panic!("Expected simple select");
        }
    } else {
        panic!("Expected select stmt");
    }
}
