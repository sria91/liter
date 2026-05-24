use sqlite3_vdbe::{Mem, Opcode, P4, StepResult, Vdbe, VdbeOp};
use sqlite3_btree::BTree;

#[test]
fn test_vdbe_math() {
    let mut vm = Vdbe::with_capacity(10, 4);
    let btree = BTree::new_in_memory();
    let mut cursors = [];

    // r0 = 5
    vm.emit(VdbeOp {
        opcode: Opcode::Integer,
        p1: 5,
        p2: 0,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    // r1 = 3
    vm.emit(VdbeOp {
        opcode: Opcode::Integer,
        p1: 3,
        p2: 1,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    // r2 = r0 + r1 (5 + 3 = 8)
    vm.emit(VdbeOp {
        opcode: Opcode::AddInt,
        p1: 0,
        p2: 1,
        p3: 2,
        p4: P4::None,
        p5: 0,
    });

    // Yield r2
    vm.emit(VdbeOp {
        opcode: Opcode::ResultRow,
        p1: 2,
        p2: 1, // count = 1
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    vm.emit(VdbeOp {
        opcode: Opcode::Halt,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
    
    let row = vm.current_result_row().unwrap();
    assert_eq!(row.len(), 1);
    assert_eq!(row[0], Mem::Int(8));

    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
}

#[test]
fn test_vdbe_goto() {
    let mut vm = Vdbe::with_capacity(5, 1);
    let btree = BTree::new_in_memory();
    let mut cursors = [];

    // 0: r0 = 10
    vm.emit(VdbeOp {
        opcode: Opcode::Integer,
        p1: 10,
        p2: 0,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    // 1: goto 3
    vm.emit(VdbeOp {
        opcode: Opcode::Goto,
        p1: 0,
        p2: 3,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    // 2: r0 = 99 (skipped)
    vm.emit(VdbeOp {
        opcode: Opcode::Integer,
        p1: 99,
        p2: 0,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    // 3: yield r0
    vm.emit(VdbeOp {
        opcode: Opcode::ResultRow,
        p1: 0,
        p2: 1,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
    let row = vm.current_result_row().unwrap();
    assert_eq!(row[0], Mem::Int(10));
}

#[test]
fn test_vdbe_loop() {
    let mut vm = Vdbe::with_capacity(10, 2);
    let btree = BTree::new_in_memory();
    let mut cursors = [];

    // r0 = 3 (counter)
    vm.emit(VdbeOp {
        opcode: Opcode::Integer,
        p1: 3,
        p2: 0,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });
    // r1 = 1 (decrement)
    vm.emit(VdbeOp {
        opcode: Opcode::Integer,
        p1: 1,
        p2: 1,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    // Loop start (addr 2)
    // Yield r0
    let loop_start = vm.emit(VdbeOp {
        opcode: Opcode::ResultRow,
        p1: 0,
        p2: 1,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    // r0 = r0 - r1
    vm.emit(VdbeOp {
        opcode: Opcode::SubtractInt,
        p1: 1,
        p2: 0,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    // if r0 > 0 goto loop_start
    let r0_gt_0 = vm.alloc_reg();
    vm.emit(VdbeOp {
        opcode: Opcode::Integer,
        p1: 0,
        p2: r0_gt_0 as i32,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });
    vm.emit(VdbeOp {
        opcode: Opcode::Gt,
        p1: r0_gt_0 as i32, // b is 0
        p2: loop_start as i32,
        p3: 0,              // a is counter
        p4: P4::None,
        p5: 0,
    });

    vm.emit(VdbeOp {
        opcode: Opcode::Halt,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row); // 3
    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row); // 2
    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row); // 1
    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done); // 0 -> exit
}

#[test]
fn test_vdbe_types() {
    let mut vm = Vdbe::with_capacity(5, 3);
    let btree = BTree::new_in_memory();
    let mut cursors = [];

    vm.emit(VdbeOp {
        opcode: Opcode::Real,
        p1: 0, p2: 0, p3: 0,
        p4: P4::Real(3.14),
        p5: 0,
    });
    vm.emit(VdbeOp {
        opcode: Opcode::String8,
        p1: 0, p2: 1, p3: 0,
        p4: P4::Text(std::sync::Arc::from("hello")),
        p5: 0,
    });
    vm.emit(VdbeOp {
        opcode: Opcode::Null,
        p1: 0, p2: 2, p3: 0,
        p4: P4::None,
        p5: 0,
    });
    vm.emit(VdbeOp {
        opcode: Opcode::ResultRow,
        p1: 0, p2: 3, p3: 0,
        p4: P4::None, p5: 0,
    });

    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Row);
    let row = vm.current_result_row().unwrap();
    assert_eq!(row[0], Mem::Real(3.14));
    assert_eq!(row[1], Mem::Text(std::sync::Arc::from("hello")));
    assert_eq!(row[2], Mem::Null);
    assert_eq!(vm.step(&btree, &mut cursors).unwrap(), StepResult::Done);
}
