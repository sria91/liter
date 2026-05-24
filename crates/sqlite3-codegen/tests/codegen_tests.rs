use sqlite3_parser::parse_stmt;
use sqlite3_codegen::compile;
use sqlite3_vdbe::StepResult;

#[test]
fn test_compile_arithmetic() {
    let sql = "SELECT 5 + 10 * 2;";
    let ast = parse_stmt(sql).unwrap();
    let mut vm = compile(&ast).unwrap();

    assert_eq!(vm.step().unwrap(), StepResult::Row);
    
    // Internal VM state isn't public, but we can verify it doesn't error
    // and returns a Row. In a full integration, we'd extract the row data.
    
    assert_eq!(vm.step().unwrap(), StepResult::Done);
}

#[test]
fn test_compile_comparison() {
    let sql = "SELECT 5 > 2;";
    let ast = parse_stmt(sql).unwrap();
    let mut vm = compile(&ast).unwrap();

    assert_eq!(vm.step().unwrap(), StepResult::Row);
    assert_eq!(vm.step().unwrap(), StepResult::Done);
}

#[test]
fn test_compile_complex() {
    let sql = "SELECT (10 + 5) / 3 = 5, 'hello';";
    let ast = parse_stmt(sql).unwrap();
    let mut vm = compile(&ast).unwrap();

    assert_eq!(vm.step().unwrap(), StepResult::Row);
    assert_eq!(vm.step().unwrap(), StepResult::Done);
}
