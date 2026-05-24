use sqlite3_parser::parse_all;
use sqlite3_schema::{Schema, SchemaObject, ObjectKind};
use sqlite3_ast::ColumnDef;
use sqlite3_optimizer::{Optimizer, ScanKind};

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
            ColumnDef { name: "email".to_string(), type_name: None, constraints: vec![] },
        ],
    });
    schema.insert(SchemaObject {
        kind: ObjectKind::Index,
        name: "idx_users_email".to_string(),
        tbl_name: "users".to_string(),
        root_page: 3,
        sql: None,
        columns: vec![],
    });
    schema
}

#[test]
fn test_optimize_full_scan() {
    let schema = make_schema();
    let optimizer = Optimizer::new(&schema);

    let sql = "SELECT * FROM users;";
    let stmts = parse_all(sql).unwrap();
    
    let plan = optimizer.optimize_stmt(&stmts[0]).unwrap();
    assert_eq!(plan.loops.len(), 1);
    assert_eq!(plan.loops[0].table, "users");
    assert_eq!(plan.loops[0].scan_kind, ScanKind::FullScan);
}

#[test]
fn test_optimize_rowid_lookup() {
    let schema = make_schema();
    let optimizer = Optimizer::new(&schema);

    let sql = "SELECT * FROM users WHERE id = 42;";
    let stmts = parse_all(sql).unwrap();
    
    let plan = optimizer.optimize_stmt(&stmts[0]).unwrap();
    assert_eq!(plan.loops.len(), 1);
    assert_eq!(plan.loops[0].scan_kind, ScanKind::RowIdLookup);
}

#[test]
fn test_optimize_index_scan() {
    let schema = make_schema();
    let optimizer = Optimizer::new(&schema);

    // email constraint matches our dummy index logic
    let sql = "SELECT * FROM users WHERE email = 'test@example.com';";
    let stmts = parse_all(sql).unwrap();
    
    let plan = optimizer.optimize_stmt(&stmts[0]).unwrap();
    assert_eq!(plan.loops.len(), 1);
    
    if let ScanKind::IndexScan { index, constraints } = &plan.loops[0].scan_kind {
        assert_eq!(index, "idx_users_email");
        assert!(constraints.contains(&"email".to_string()));
    } else {
        panic!("Expected IndexScan");
    }
}
