use sqlite3_parser::parse_stmt;
use sqlite3_codegen::compile;
use sqlite3_vdbe::StepResult;
use sqlite3_btree::BTree;

#[test]
fn test_compile_arithmetic() {
    let sql = "SELECT 5 + 10 * 2;";
    let ast = parse_stmt(sql).unwrap();
    let mut vm = compile(&ast).unwrap();
    let btree = BTree::new_in_memory();
    let mut cursors = [];

    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
}

#[test]
fn test_compile_comparison() {
    let sql = "SELECT 5 > 2;";
    let ast = parse_stmt(sql).unwrap();
    let mut vm = compile(&ast).unwrap();
    let btree = BTree::new_in_memory();
    let mut cursors = [];

    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
}

#[test]
fn test_compile_complex() {
    let sql = "SELECT (10 + 5) / 3 = 5, 'hello';";
    let ast = parse_stmt(sql).unwrap();
    let mut vm = compile(&ast).unwrap();
    let btree = BTree::new_in_memory();
    let mut cursors = [];

    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
}
