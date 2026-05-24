use sqlite3_vdbe::*;

#[test]
fn test_register_movement() {
    let mut vm = Vdbe::new();
    let r0 = vm.alloc_reg();
    let r1 = vm.alloc_reg();

    vm.emit(VdbeOp {
        opcode: Opcode::Integer,
        p1: 42,
        p2: r0 as i32,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });
    vm.emit(VdbeOp {
        opcode: Opcode::Copy,
        p1: r0 as i32,
        p2: r1 as i32,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });
    vm.emit(VdbeOp {
        opcode: Opcode::ResultRow,
        p1: 0,
        p2: 0,
        p3: 0,
        p4: P4::None,
        p5: 0,
    });

    assert_eq!(vm.step().unwrap(), StepResult::Row);
    // VDBE internal state checks are generally private, but we can verify the step result behavior.
}

#[test]
fn test_arithmetic() {
    let mut vm = Vdbe::with_capacity(10, 5);
    
    vm.emit(VdbeOp { opcode: Opcode::Integer, p1: 10, p2: 0, p3: 0, p4: P4::None, p5: 0 }); // r[0] = 10
    vm.emit(VdbeOp { opcode: Opcode::Integer, p1: 5, p2: 1, p3: 0, p4: P4::None, p5: 0 }); // r[1] = 5
    
    vm.emit(VdbeOp { opcode: Opcode::AddInt, p1: 1, p2: 0, p3: 2, p4: P4::None, p5: 0 }); // r[2] = r[0] + r[1] (10 + 5)
    vm.emit(VdbeOp { opcode: Opcode::SubtractInt, p1: 1, p2: 0, p3: 3, p4: P4::None, p5: 0 }); // r[3] = r[0] - r[1] (10 - 5)
    
    vm.emit(VdbeOp { opcode: Opcode::ResultRow, p1: 0, p2: 0, p3: 0, p4: P4::None, p5: 0 });

    assert_eq!(vm.step().unwrap(), StepResult::Row);
}

#[test]
fn test_control_flow_loop() {
    let mut vm = Vdbe::with_capacity(10, 3);
    
    // r[0] = 3 (loop counter)
    vm.emit(VdbeOp { opcode: Opcode::Integer, p1: 3, p2: 0, p3: 0, p4: P4::None, p5: 0 });
    // r[1] = 0 (constant for comparison)
    vm.emit(VdbeOp { opcode: Opcode::Integer, p1: 0, p2: 1, p3: 0, p4: P4::None, p5: 0 });
    // r[2] = 1 (constant for decrement)
    vm.emit(VdbeOp { opcode: Opcode::Integer, p1: 1, p2: 2, p3: 0, p4: P4::None, p5: 0 });
    
    // PC = 3: loop start
    // if r[0] == r[1] (3 == 0) goto end (PC = 7)
    vm.emit(VdbeOp { opcode: Opcode::Eq, p1: 0, p2: 7, p3: 1, p4: P4::None, p5: 0 });
    
    // Yield Row
    vm.emit(VdbeOp { opcode: Opcode::ResultRow, p1: 0, p2: 0, p3: 0, p4: P4::None, p5: 0 });
    
    // r[0] = r[0] - r[2]
    vm.emit(VdbeOp { opcode: Opcode::SubtractInt, p1: 2, p2: 0, p3: 0, p4: P4::None, p5: 0 });
    
    // goto loop start (PC = 3)
    vm.emit(VdbeOp { opcode: Opcode::Goto, p1: 0, p2: 3, p3: 0, p4: P4::None, p5: 0 });
    
    // PC = 7: end
    vm.emit(VdbeOp { opcode: Opcode::Halt, p1: 0, p2: 0, p3: 0, p4: P4::None, p5: 0 });
    
    assert_eq!(vm.step().unwrap(), StepResult::Row); // 3
    assert_eq!(vm.step().unwrap(), StepResult::Row); // 2
    assert_eq!(vm.step().unwrap(), StepResult::Row); // 1
    assert_eq!(vm.step().unwrap(), StepResult::Done); // 0 -> exit
}

#[test]
fn test_gosub() {
    let mut vm = Vdbe::with_capacity(10, 0);
    
    vm.emit(VdbeOp { opcode: Opcode::Gosub, p1: 0, p2: 3, p3: 0, p4: P4::None, p5: 0 }); // 0
    vm.emit(VdbeOp { opcode: Opcode::Halt, p1: 0, p2: 0, p3: 0, p4: P4::None, p5: 0 });  // 1
    vm.emit(VdbeOp { opcode: Opcode::Noop, p1: 0, p2: 0, p3: 0, p4: P4::None, p5: 0 });  // 2 (skipped)
    
    // Subroutine at PC = 3
    vm.emit(VdbeOp { opcode: Opcode::ResultRow, p1: 0, p2: 0, p3: 0, p4: P4::None, p5: 0 }); // 3
    vm.emit(VdbeOp { opcode: Opcode::Return, p1: 0, p2: 0, p3: 0, p4: P4::None, p5: 0 }); // 4
    
    assert_eq!(vm.step().unwrap(), StepResult::Row);
    assert_eq!(vm.step().unwrap(), StepResult::Done);
}
