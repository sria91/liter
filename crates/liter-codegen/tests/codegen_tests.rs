use liter_ast::*;
use liter_btree::BTree;
use liter_codegen::{compile, compile_with_schema, CodegenError, Compiler};
use liter_parser::parse_stmt;
use liter_schema::{ObjectKind, Schema, SchemaObject};
use liter_vdbe::StepResult;

fn create_test_schema() -> Schema {
    let schema = Schema::new();
    schema.insert(SchemaObject {
        kind: ObjectKind::Table,
        name: "users".to_owned(),
        tbl_name: "users".to_owned(),
        root_page: 2,
        sql: Some("CREATE TABLE users (id INTEGER, name TEXT, age INTEGER, score REAL)".to_owned()),
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
            ColumnDef {
                name: "score".to_string(),
                type_name: None,
                constraints: vec![],
            },
        ],
    });
    schema
}

#[test]
fn test_default_compiler() {
    let compiler = Compiler::default();
    assert!(compiler.schema.is_none());
}

#[test]
fn test_compile_arithmetic() {
    let sql = "SELECT 5 + 10 * 2 - 4 / 2 % 3;";
    let ast = parse_stmt(sql).unwrap();
    let mut vm = compile(&ast).unwrap();
    let btree = BTree::new_in_memory();
    let mut cursors = [];

    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
}

#[test]
fn test_compile_comparison() {
    let sql = "SELECT 5 > 2, 5 >= 5, 5 < 10, 5 <= 5, 5 == 5, 5 != 2;";
    let ast = parse_stmt(sql).unwrap();
    let mut vm = compile(&ast).unwrap();
    let btree = BTree::new_in_memory();
    let mut cursors = [];

    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
}

#[test]
fn test_compile_complex() {
    let sql = "SELECT (10 + 5) / 3 = 5, 'hello', 3.14, NULL;";
    let ast = parse_stmt(sql).unwrap();
    let mut vm = compile(&ast).unwrap();
    let btree = BTree::new_in_memory();
    let mut cursors = [];

    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
}

#[test]
fn test_compile_create_table() {
    let sql = "CREATE TABLE users (id INTEGER);";
    let ast = parse_stmt(sql).unwrap();
    let vm = compile(&ast).unwrap();
    assert!(!vm.ops.is_empty());
}

#[test]
fn test_compile_transaction_stmts() {
    let stmts = [
        "BEGIN;",
        "COMMIT;",
        "ROLLBACK;",
        "SAVEPOINT sp;",
        "RELEASE sp;",
    ];
    for sql in stmts {
        let ast = parse_stmt(sql).unwrap();
        let mut vm = compile(&ast).unwrap();
        let btree = BTree::new_in_memory();
        let mut cursors = [];
        assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
    }
}

#[test]
fn test_compile_select_literal_where() {
    let sql = "SELECT 1 WHERE 1 AND (0 OR 1);";
    let ast = parse_stmt(sql).unwrap();
    let vm = compile(&ast).unwrap();
    assert!(!vm.ops.is_empty());
}

#[test]
fn test_compile_select_from() {
    let schema = create_test_schema();

    let sql = "SELECT id, name, age + 10, * FROM users WHERE id = 1 AND (age > 20 OR score < 5.0) ORDER BY age LIMIT 10;";
    let ast = parse_stmt(sql).unwrap();
    let vm = compile_with_schema(&ast, &schema).unwrap();
    assert!(!vm.ops.is_empty());
}

#[test]
fn test_compile_select_from_no_order_with_limit() {
    let schema = create_test_schema();

    let sql = "SELECT * FROM users LIMIT 5;";
    let ast = parse_stmt(sql).unwrap();
    let vm = compile_with_schema(&ast, &schema).unwrap();
    assert!(!vm.ops.is_empty());
}

#[test]
fn test_compile_aggregate_select() {
    let schema = create_test_schema();

    let sql = "SELECT count(*), sum(age), avg(score), min(age), max(age) FROM users WHERE age > 10 HAVING count(*) > 0;";
    let ast = parse_stmt(sql).unwrap();
    let vm = compile_with_schema(&ast, &schema).unwrap();
    assert!(!vm.ops.is_empty());
}

#[test]
fn test_compile_group_by_aggregate_select() {
    let schema = create_test_schema();

    let sql = "SELECT name, count(*), sum(age) FROM users WHERE id > 0 GROUP BY name HAVING sum(age) > 100;";
    let ast = parse_stmt(sql).unwrap();
    let vm = compile_with_schema(&ast, &schema).unwrap();
    assert!(!vm.ops.is_empty());
}

#[test]
fn test_compile_insert() {
    let schema = create_test_schema();

    let sql = "INSERT INTO users VALUES (1, 'Alice', 30, 99.5), (2, 'Bob', 25, 88.0);";
    let ast = parse_stmt(sql).unwrap();
    let vm = compile_with_schema(&ast, &schema).unwrap();
    assert!(!vm.ops.is_empty());
}

#[test]
fn test_compile_delete() {
    let schema = create_test_schema();

    let sql = "DELETE FROM users WHERE id = 1;";
    let ast = parse_stmt(sql).unwrap();
    let vm = compile_with_schema(&ast, &schema).unwrap();
    assert!(!vm.ops.is_empty());

    let sql_all = "DELETE FROM users;";
    let ast_all = parse_stmt(sql_all).unwrap();
    let vm_all = compile_with_schema(&ast_all, &schema).unwrap();
    assert!(!vm_all.ops.is_empty());
}

#[test]
fn test_compile_update() {
    let schema = create_test_schema();

    let sql = "UPDATE users SET age = age + 1, name = 'Alice Updated' WHERE id = 1;";
    let ast = parse_stmt(sql).unwrap();
    let vm = compile_with_schema(&ast, &schema).unwrap();
    assert!(!vm.ops.is_empty());

    let sql_all = "UPDATE users SET age = 0;";
    let ast_all = parse_stmt(sql_all).unwrap();
    let vm_all = compile_with_schema(&ast_all, &schema).unwrap();
    assert!(!vm_all.ops.is_empty());
}

#[test]
fn test_compile_errors_and_edge_cases() {
    let schema = create_test_schema();

    // 1. Missing schema context
    let ast = parse_stmt("SELECT * FROM users;").unwrap();
    let res = compile(&ast);
    assert!(matches!(res, Err(CodegenError::Schema(_))));

    // 2. Table not found
    let ast = parse_stmt("SELECT * FROM nonexistent;").unwrap();
    let res = compile_with_schema(&ast, &schema);
    assert!(matches!(res, Err(CodegenError::Schema(_))));

    // 3. Column not found
    let ast = parse_stmt("SELECT nonexistent FROM users;").unwrap();
    let res = compile_with_schema(&ast, &schema);
    assert!(matches!(res, Err(CodegenError::Schema(_))));

    // 4. Column used outside FROM context
    let ast = parse_stmt("SELECT id;").unwrap();
    let res = compile(&ast);
    assert!(matches!(res, Err(CodegenError::Schema(_))));

    // 5. Unary minus in select literal
    let ast = parse_stmt("SELECT 0 - 5;").unwrap();
    let vm = compile(&ast).unwrap();
    assert!(!vm.ops.is_empty());

    // 6. Function calls (none, star, list, distinct)
    let ast = parse_stmt("SELECT abs(10), count(), count(*);").unwrap();
    let vm = compile(&ast).unwrap();
    assert!(!vm.ops.is_empty());

    // 7. WHERE with IS NULL and IS NOT NULL
    let sql = "SELECT * FROM users WHERE name IS NULL AND age IS NOT NULL;";
    let ast = parse_stmt(sql).unwrap();
    let vm = compile_with_schema(&ast, &schema).unwrap();
    assert!(!vm.ops.is_empty());

    // 8. Insert errors
    let ast_insert_no_schema = parse_stmt("INSERT INTO users VALUES (1);").unwrap();
    assert!(matches!(compile(&ast_insert_no_schema), Err(CodegenError::Schema(_))));

    let ast_insert_nonexistent = parse_stmt("INSERT INTO nonexistent VALUES (1);").unwrap();
    assert!(matches!(compile_with_schema(&ast_insert_nonexistent, &schema), Err(CodegenError::Schema(_))));

    // 9. Delete errors
    let ast_delete_no_schema = parse_stmt("DELETE FROM users;").unwrap();
    assert!(matches!(compile(&ast_delete_no_schema), Err(CodegenError::Schema(_))));

    let ast_delete_nonexistent = parse_stmt("DELETE FROM nonexistent;").unwrap();
    assert!(matches!(compile_with_schema(&ast_delete_nonexistent, &schema), Err(CodegenError::Schema(_))));

    // 10. Update errors
    let ast_update_no_schema = parse_stmt("UPDATE users SET id = 1;").unwrap();
    assert!(matches!(compile(&ast_update_no_schema), Err(CodegenError::Schema(_))));

    let ast_update_nonexistent = parse_stmt("UPDATE nonexistent SET id = 1;").unwrap();
    assert!(matches!(compile_with_schema(&ast_update_nonexistent, &schema), Err(CodegenError::Schema(_))));

    let ast_update_bad_col = parse_stmt("UPDATE users SET bad_col = 1;").unwrap();
    assert!(matches!(compile_with_schema(&ast_update_bad_col, &schema), Err(CodegenError::Schema(_))));
}

#[test]
fn test_compile_literals_and_unary() {
    // Manually construct AST for expressions the parser doesn't fully support yet
    // SELECT TRUE, FALSE, NULL, +42, -(-10), NOT 0, NOT 1;
    let ast = Stmt::Select(Box::new(SelectStmt {
        with: None,
        body: SelectBody::Simple(SimpleSelect {
            distinct: DistinctKind::All,
            result_columns: vec![
                ResultColumn::Expr { expr: Expr::Literal(LiteralValue::True), alias: None },
                ResultColumn::Expr { expr: Expr::Literal(LiteralValue::False), alias: None },
                ResultColumn::Expr { expr: Expr::Literal(LiteralValue::Null), alias: None },
                ResultColumn::Expr {
                    expr: Expr::Unary { op: UnaryOp::Plus, operand: Box::new(Expr::Literal(LiteralValue::Integer(42))) },
                    alias: None
                },
                ResultColumn::Expr {
                    expr: Expr::Unary {
                        op: UnaryOp::Minus,
                        operand: Box::new(Expr::Unary { op: UnaryOp::Minus, operand: Box::new(Expr::Literal(LiteralValue::Integer(10))) })
                    },
                    alias: None
                },
                ResultColumn::Expr {
                    expr: Expr::Unary { op: UnaryOp::Not, operand: Box::new(Expr::Literal(LiteralValue::Integer(0))) },
                    alias: None
                },
                ResultColumn::Expr {
                    expr: Expr::Unary { op: UnaryOp::Not, operand: Box::new(Expr::Literal(LiteralValue::Integer(1))) },
                    alias: None
                },
            ],
            from: None,
            where_: None,
            group_by: vec![],
            having: None,
            window: vec![],
        }),
        order_by: vec![],
        limit: None,
    }));

    let mut vm = compile(&ast).unwrap();
    let btree = BTree::new_in_memory();
    let mut cursors = [];
    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
}

#[test]
fn test_compile_unimplemented_and_errors() {
    let schema = create_test_schema();

    // 1. Unimplemented Stmt
    let drop_stmt = Stmt::Drop(Box::new(DropStmt {
        kind: DropKind::Table,
        if_exists: false,
        schema: None,
        name: "users".to_string(),
    }));
    assert!(matches!(compile(&drop_stmt), Err(CodegenError::NotImplemented)));

    // 2. Unimplemented SelectBody (Values / Compound)
    let compound_select = Stmt::Select(Box::new(SelectStmt {
        with: None,
        body: SelectBody::Compound {
            left: Box::new(SelectBody::Simple(SimpleSelect {
                distinct: DistinctKind::All,
                result_columns: vec![ResultColumn::Star],
                from: None,
                where_: None,
                group_by: vec![],
                having: None,
                window: vec![],
            })),
            op: CompoundOp::Union,
            right: Box::new(SelectBody::Simple(SimpleSelect {
                distinct: DistinctKind::All,
                result_columns: vec![ResultColumn::Star],
                from: None,
                where_: None,
                group_by: vec![],
                having: None,
                window: vec![],
            })),
        },
        order_by: vec![],
        limit: None,
    }));
    assert!(matches!(compile(&compound_select), Err(CodegenError::NotImplemented)));

    // 3. Select literal with Star result column -> NotImplemented
    let star_literal_select = Stmt::Select(Box::new(SelectStmt {
        with: None,
        body: SelectBody::Simple(SimpleSelect {
            distinct: DistinctKind::All,
            result_columns: vec![ResultColumn::Star],
            from: None,
            where_: None,
            group_by: vec![],
            having: None,
            window: vec![],
        }),
        order_by: vec![],
        limit: None,
    }));
    assert!(matches!(compile(&star_literal_select), Err(CodegenError::NotImplemented)));

    // 4. Select from multiple tables or joins -> NotImplemented
    let join_select = Stmt::Select(Box::new(SelectStmt {
        with: None,
        body: SelectBody::Simple(SimpleSelect {
            distinct: DistinctKind::All,
            result_columns: vec![ResultColumn::Star],
            from: Some(FromClause {
                tables: vec![
                    TableOrSubquery::Table {
                        schema: None,
                        name: "users".to_string(),
                        alias: None,
                        indexed: IndexedKind::None,
                    },
                    TableOrSubquery::Table {
                        schema: None,
                        name: "orders".to_string(),
                        alias: None,
                        indexed: IndexedKind::None,
                    },
                ],
                joins: vec![],
            }),
            where_: None,
            group_by: vec![],
            having: None,
            window: vec![],
        }),
        order_by: vec![],
        limit: None,
    }));
    assert!(matches!(compile_with_schema(&join_select, &schema), Err(CodegenError::NotImplemented)));

    // 5. Select from subquery -> NotImplemented
    let subquery_select = Stmt::Select(Box::new(SelectStmt {
        with: None,
        body: SelectBody::Simple(SimpleSelect {
            distinct: DistinctKind::All,
            result_columns: vec![ResultColumn::Star],
            from: Some(FromClause {
                tables: vec![TableOrSubquery::Subquery {
                    select: Box::new(SelectStmt {
                        with: None,
                        body: SelectBody::Simple(SimpleSelect {
                            distinct: DistinctKind::All,
                            result_columns: vec![ResultColumn::Star],
                            from: None,
                            where_: None,
                            group_by: vec![],
                            having: None,
                            window: vec![],
                        }),
                        order_by: vec![],
                        limit: None,
                    }),
                    alias: None,
                }],
                joins: vec![],
            }),
            where_: None,
            group_by: vec![],
            having: None,
            window: vec![],
        }),
        order_by: vec![],
        limit: None,
    }));
    assert!(matches!(compile_with_schema(&subquery_select, &schema), Err(CodegenError::NotImplemented)));

    // 6. Aggregate select errors
    let agg_no_schema = Stmt::Select(Box::new(SelectStmt {
        with: None,
        body: SelectBody::Simple(SimpleSelect {
            distinct: DistinctKind::All,
            result_columns: vec![ResultColumn::Expr {
                expr: Expr::Function {
                    schema: None,
                    name: "count".to_string(),
                    args: FunctionArgs::Star,
                    filter: None,
                    over: None,
                },
                alias: None,
            }],
            from: Some(FromClause {
                tables: vec![TableOrSubquery::Table {
                    schema: None,
                    name: "users".to_string(),
                    alias: None,
                    indexed: IndexedKind::None,
                }],
                joins: vec![],
            }),
            where_: None,
            group_by: vec![],
            having: None,
            window: vec![],
        }),
        order_by: vec![],
        limit: None,
    }));
    assert!(matches!(compile(&agg_no_schema), Err(CodegenError::Schema(_))));

    // 7. Aggregate select on join -> NotImplemented
    let mut agg_join = agg_no_schema.clone();
    if let Stmt::Select(s) = &mut agg_join {
        if let SelectBody::Simple(b) = &mut s.body {
            if let Some(f) = &mut b.from {
                f.tables.push(TableOrSubquery::Table {
                    schema: None,
                    name: "t2".to_string(),
                    alias: None,
                    indexed: IndexedKind::None,
                });
            }
        }
    }
    assert!(matches!(compile_with_schema(&agg_join, &schema), Err(CodegenError::NotImplemented)));

    // 8. Aggregate select on subquery -> NotImplemented
    let mut agg_subquery = agg_no_schema.clone();
    if let Stmt::Select(s) = &mut agg_subquery {
        if let SelectBody::Simple(b) = &mut s.body {
            if let Some(f) = &mut b.from {
                f.tables[0] = TableOrSubquery::Subquery {
                    select: Box::new(SelectStmt {
                        with: None,
                        body: SelectBody::Simple(SimpleSelect {
                            distinct: DistinctKind::All,
                            result_columns: vec![],
                            from: None,
                            where_: None,
                            group_by: vec![],
                            having: None,
                            window: vec![],
                        }),
                        order_by: vec![],
                        limit: None,
                    }),
                    alias: None,
                };
            }
        }
    }
    assert!(matches!(compile_with_schema(&agg_subquery, &schema), Err(CodegenError::NotImplemented)));

    // 9. Aggregate select table not found
    let mut agg_not_found = agg_no_schema.clone();
    if let Stmt::Select(s) = &mut agg_not_found {
        if let SelectBody::Simple(b) = &mut s.body {
            if let Some(f) = &mut b.from {
                f.tables[0] = TableOrSubquery::Table {
                    schema: None,
                    name: "nonexistent".to_string(),
                    alias: None,
                    indexed: IndexedKind::None,
                };
            }
        }
    }
    assert!(matches!(compile_with_schema(&agg_not_found, &schema), Err(CodegenError::Schema(_))));

    // 10. Unsupported Expr in compile_expr
    let ast_subquery_expr = Stmt::Select(Box::new(SelectStmt {
        with: None,
        body: SelectBody::Simple(SimpleSelect {
            distinct: DistinctKind::All,
            result_columns: vec![ResultColumn::Expr {
                expr: Expr::Exists {
                    not: false,
                    select: Box::new(SelectStmt {
                        with: None,
                        body: SelectBody::Simple(SimpleSelect {
                            distinct: DistinctKind::All,
                            result_columns: vec![],
                            from: None,
                            where_: None,
                            group_by: vec![],
                            having: None,
                            window: vec![],
                        }),
                        order_by: vec![],
                        limit: None,
                    }),
                },
                alias: None,
            }],
            from: None,
            where_: None,
            group_by: vec![],
            having: None,
            window: vec![],
        }),
        order_by: vec![],
        limit: None,
    }));
    assert!(matches!(compile(&ast_subquery_expr), Err(CodegenError::NotImplemented)));

    // 11. Unsupported BinaryOp (e.g. Concat)
    let ast_concat = Stmt::Select(Box::new(SelectStmt {
        with: None,
        body: SelectBody::Simple(SimpleSelect {
            distinct: DistinctKind::All,
            result_columns: vec![ResultColumn::Expr {
                expr: Expr::Binary {
                    op: BinaryOp::Concat,
                    left: Box::new(Expr::Literal(LiteralValue::Text("a".to_string()))),
                    right: Box::new(Expr::Literal(LiteralValue::Text("b".to_string()))),
                },
                alias: None,
            }],
            from: None,
            where_: None,
            group_by: vec![],
            having: None,
            window: vec![],
        }),
        order_by: vec![],
        limit: None,
    }));
    assert!(matches!(compile(&ast_concat), Err(CodegenError::NotImplemented)));

    // 12. Aggregate select with Star result column in group by / non-group by
    let agg_star_result = Stmt::Select(Box::new(SelectStmt {
        with: None,
        body: SelectBody::Simple(SimpleSelect {
            distinct: DistinctKind::All,
            result_columns: vec![
                ResultColumn::Star,
                ResultColumn::Expr {
                    expr: Expr::Function {
                        schema: None,
                        name: "count".to_string(),
                        args: FunctionArgs::Star,
                        filter: None,
                        over: None,
                    },
                    alias: None,
                },
            ],
            from: Some(FromClause {
                tables: vec![TableOrSubquery::Table {
                    schema: None,
                    name: "users".to_string(),
                    alias: None,
                    indexed: IndexedKind::None,
                }],
                joins: vec![],
            }),
            where_: None,
            group_by: vec![Expr::Column {
                schema: None,
                table: None,
                name: "name".to_string(),
            }],
            having: None,
            window: vec![],
        }),
        order_by: vec![],
        limit: None,
    }));
    assert!(matches!(compile_with_schema(&agg_star_result, &schema), Err(CodegenError::NotImplemented)));

    let agg_star_no_gb = Stmt::Select(Box::new(SelectStmt {
        with: None,
        body: SelectBody::Simple(SimpleSelect {
            distinct: DistinctKind::All,
            result_columns: vec![
                ResultColumn::Star,
                ResultColumn::Expr {
                    expr: Expr::Function {
                        schema: None,
                        name: "count".to_string(),
                        args: FunctionArgs::Star,
                        filter: None,
                        over: None,
                    },
                    alias: None,
                },
            ],
            from: Some(FromClause {
                tables: vec![TableOrSubquery::Table {
                    schema: None,
                    name: "users".to_string(),
                    alias: None,
                    indexed: IndexedKind::None,
                }],
                joins: vec![],
            }),
            where_: None,
            group_by: vec![],
            having: None,
            window: vec![],
        }),
        order_by: vec![],
        limit: None,
    }));
    assert!(matches!(compile_with_schema(&agg_star_no_gb, &schema), Err(CodegenError::NotImplemented)));
}

#[test]
fn test_compile_aggregate_and_group_by_variations() {
    let schema = create_test_schema();

    // 1. Group by with count(*) without args
    let sql1 = "SELECT name, count(*) FROM users GROUP BY name;";
    let ast1 = parse_stmt(sql1).unwrap();
    let vm1 = compile_with_schema(&ast1, &schema).unwrap();
    assert!(!vm1.ops.is_empty());

    // 2. Group by without where
    let sql2 = "SELECT name, sum(age) FROM users GROUP BY name;";
    let ast2 = parse_stmt(sql2).unwrap();
    let vm2 = compile_with_schema(&ast2, &schema).unwrap();
    assert!(!vm2.ops.is_empty());

    // 3. Aggregate without group by and without where
    let sql3 = "SELECT sum(age), count(*) FROM users;";
    let ast3 = parse_stmt(sql3).unwrap();
    let vm3 = compile_with_schema(&ast3, &schema).unwrap();
    assert!(!vm3.ops.is_empty());

    // 4. Aggregate with multi-arg function in group by
    let sql4 = "SELECT name, coalesce(name, 'default') FROM users GROUP BY name;";
    let ast4 = parse_stmt(sql4).unwrap();
    let vm4 = compile_with_schema(&ast4, &schema).unwrap();
    assert!(!vm4.ops.is_empty());

    // 5. Select with complex WHERE predicate expressions
    let sql5 = "SELECT * FROM users WHERE (id + 1 = 2) AND age IS NOT NULL;";
    let ast5 = parse_stmt(sql5).unwrap();
    let vm5 = compile_with_schema(&ast5, &schema).unwrap();
    assert!(!vm5.ops.is_empty());

    // 6. Select literal with WHERE
    let sql6 = "SELECT 1 WHERE 1 = 1 AND 2 IS NOT NULL;";
    let ast6 = parse_stmt(sql6).unwrap();
    let vm6 = compile(&ast6).unwrap();
    assert!(!vm6.ops.is_empty());

    // 7. Multi-arg aggregate function without group by
    let multi_agg_no_gb = Stmt::Select(Box::new(SelectStmt {
        with: None,
        body: SelectBody::Simple(SimpleSelect {
            distinct: DistinctKind::All,
            result_columns: vec![ResultColumn::Expr {
                expr: Expr::Function {
                    schema: None,
                    name: "min".to_string(),
                    args: FunctionArgs::Distinct(vec![
                        Expr::Column {
                            schema: None,
                            table: None,
                            name: "id".to_string(),
                        },
                        Expr::Column {
                            schema: None,
                            table: None,
                            name: "age".to_string(),
                        },
                    ]),
                    filter: None,
                    over: None,
                },
                alias: None,
            }],
            from: Some(FromClause {
                tables: vec![TableOrSubquery::Table {
                    schema: None,
                    name: "users".to_string(),
                    alias: None,
                    indexed: IndexedKind::None,
                }],
                joins: vec![],
            }),
            where_: None,
            group_by: vec![],
            having: None,
            window: vec![],
        }),
        order_by: vec![],
        limit: None,
    }));
    let vm7 = compile_with_schema(&multi_agg_no_gb, &schema).unwrap();
    assert!(!vm7.ops.is_empty());

    // 8. Multi-arg aggregate function with group by
    let multi_agg_gb = Stmt::Select(Box::new(SelectStmt {
        with: None,
        body: SelectBody::Simple(SimpleSelect {
            distinct: DistinctKind::All,
            result_columns: vec![
                ResultColumn::Expr {
                    expr: Expr::Column {
                        schema: None,
                        table: None,
                        name: "name".to_string(),
                    },
                    alias: None,
                },
                ResultColumn::Expr {
                    expr: Expr::Function {
                        schema: None,
                        name: "max".to_string(),
                        args: FunctionArgs::Distinct(vec![
                            Expr::Column {
                                schema: None,
                                table: None,
                                name: "id".to_string(),
                            },
                            Expr::Column {
                                schema: None,
                                table: None,
                                name: "age".to_string(),
                            },
                        ]),
                        filter: None,
                        over: None,
                    },
                    alias: None,
                },
            ],
            from: Some(FromClause {
                tables: vec![TableOrSubquery::Table {
                    schema: None,
                    name: "users".to_string(),
                    alias: None,
                    indexed: IndexedKind::None,
                }],
                joins: vec![],
            }),
            where_: None,
            group_by: vec![Expr::Column {
                schema: None,
                table: None,
                name: "name".to_string(),
            }],
            having: None,
            window: vec![],
        }),
        order_by: vec![],
        limit: None,
    }));
    let vm8 = compile_with_schema(&multi_agg_gb, &schema).unwrap();
    assert!(!vm8.ops.is_empty());
}


