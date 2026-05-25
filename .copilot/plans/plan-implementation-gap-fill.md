# liter-rs: Gap Fill Implementation Plan

## Background

All tests are green. The foundation (Phases 0–2 infra, Phase 3 write path) is solid.
The **critical gap** is that `SELECT ... FROM table` is entirely impossible: the VDBE has
no read-scan opcodes, and codegen hard-rejects any `FROM` clause. Everything below
is ordered by dependency — each phase unblocks the next.

---

## Phase 7 — VDBE Read-Scan Opcodes (Unblocks SELECT FROM)

> **This is the single highest-priority phase.** Nothing useful can be queried
> until these opcodes exist.

### What must exist in `liter-btree` first

The BTree cursor already has `move_to_first`, `next`, `key`, `data`, `is_valid`.
We need one extra helper:

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/liter-btree/src/lib.rs)
- Add `pub fn rowid(&self) -> BTreeResult<i64>` — decodes the current key's 8 bytes as a big-endian `u64`, reinterprets as `i64`.
- Add `pub fn max_rowid(&self) -> BTreeResult<i64>` — traverses to last leaf, reads rowid (used by `NewRowid`; already similar to `move_to_last`).

### New VDBE opcodes to implement in `step()`

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/liter-vdbe/src/lib.rs)

| Opcode | Semantics | Operands |
|--------|-----------|----------|
| `OpenRead` | Open a B-tree cursor in read-only mode on `root_page = p2`. Slot into `cursors[p1]`. | p1=cursor_id, p2=root_page |
| `Rewind` | `move_to_first` on cursor `p1`. If empty, jump to `p2`. | p1=cursor_id, p2=jump_addr |
| `Next` | Advance cursor `p1`. If exhausted, fall through; otherwise jump to `p2`. | p1=cursor_id, p2=loop_top |
| `Prev` | Step cursor `p1` backward. If exhausted, fall through; otherwise jump to `p2`. | p1=cursor_id, p2=loop_top |
| `Last` | `move_to_last` on cursor `p1`. If empty, jump to `p2`. | p1=cursor_id, p2=jump_addr |
| `Column` | Read column `p2` from current record at cursor `p1`, write to register `p3`. Calls `decode_record` + field index. | p1=cursor_id, p2=col_idx, p3=dest_reg |
| `Rowid` | Read the integer rowid from cursor `p1` into register `p2`. | p1=cursor_id, p2=dest_reg |
| `NullRow` | Put `Mem::Null` into the virtual "current row" of cursor `p1`. | p1=cursor_id |
| `If` | Jump to `p2` if register `p1` is truthy (non-zero, non-null). | p1=reg, p2=jump_addr |
| `IfNot` | Jump to `p2` if register `p1` is falsy. | p1=reg, p2=jump_addr |
| `IsNull` | Jump to `p2` if register `p1` is `Mem::Null`. | p1=reg, p2=jump_addr |
| `NotNull` | Jump to `p2` if register `p1` is not `Mem::Null`. | p1=reg, p2=jump_addr |

**The `Column` opcode** is the most important. It must call `liter_record::decode_record` on `cursor.data()`, then index into the resulting `Vec<Value>` at position `p2`, converting `record::Value` → `vdbe::Mem`.

Add to `Opcode` enum: `If`, `IfNot`, `IsNull`, `NotNull` (already have `OpenRead`, `Rewind`, `Next`, `Prev`, `Last`, `Column`, `Rowid`, `NullRow` declared — just need `step()` arms).

### Tests

Add to `crates/liter-vdbe/tests/vdbe_tests.rs`:
- `test_open_read_rewind_next_column` — hand-assemble a program that opens a btree, rewinds, loops with `Next`, reads `Column` into a register, yields `ResultRow`. Use a pre-populated in-memory BTree.
- `test_if_ifnot_isnull` — exercise the new branching opcodes.

---

## Phase 8 — Codegen: SELECT FROM table + WHERE

> Once the VDBE opcodes exist, we wire them through codegen.

### Codegen changes

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/liter-codegen/src/lib.rs)

**Remove the `from.is_some()` rejection guard** in `compile_select`.

**Implement `compile_select_with_from`** for a single-table scan:

```
compile_select_with_from(table_name, result_cols, where_expr):
  cursor_id = alloc_cursor()
  emit OpenRead(cursor_id, root_page)        # open read cursor
  emit Rewind(cursor_id, end_label)          # jump to end if empty
  
  loop_top = current_addr()
    for each result_col:
      emit Column(cursor_id, col_idx, reg_N)  # or Rowid if column is rowid
    
    if where_expr:
      emit [condition opcodes]                # evaluate predicate
      emit IfNot(pred_reg, next_label)        # skip ResultRow if false
    
    emit ResultRow(reg_start, count)
    
  next_label:
    emit Next(cursor_id, loop_top)           # advance & loop
  
  end_label:
    emit Close(cursor_id)
```

**Column resolution**: When a `ResultColumn::Expr { expr: Expr::Column { name, .. } }` is encountered with an active `FROM` table, look up `col_idx` from `schema.get(table_name).columns`. Emit `Column(cursor_id, col_idx, dest_reg)`. For `ResultColumn::Star`, expand all columns from the schema definition.

**WHERE clause codegen** (`compile_where_expr`):
- For `BinaryOp::{Eq, Ne, Lt, Le, Gt, Ge}`: compile lhs and rhs into registers, emit the comparison opcode.
- For `BinaryOp::{And, Or}`: short-circuit evaluation using `If`/`IfNot` jumps.
- For `Expr::IsNull { not: false }`: emit `IsNull`.
- For `Expr::Column`: resolve to `Column` opcode targeting a temporary register.

### Tests

Add to `crates/liter-codegen/tests/codegen_tests.rs`:
- `test_compile_select_from` — compile `SELECT id, name FROM users`, verify opcode sequence contains `OpenRead`, `Rewind`, `Column`, `ResultRow`, `Next`.
- `test_compile_select_where` — compile `SELECT * FROM t WHERE id = 1`, verify `IfNot` skip logic.

---

## Phase 9 — Connection: End-to-End SELECT FROM + Prepare API

### Connection wiring

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/sqlite3/src/lib.rs)

- `Connection::query` already passes cursors to `Vdbe::step`. The only change needed is that `SELECT FROM` queries don't need `begin_write` — keep `execute` for mutating statements and `query` for read-only.
- Add `begin_read` / `end_read` transaction brackets in `query` (currently none; needed for snapshot isolation consistency).

**Implement `Connection::prepare`**:
```rust
pub fn prepare<'c>(&'c self, sql: &str) -> SqliteResult<Statement<'c>> {
    let ast = liter_parser::parse_stmt(sql)?;
    let vm = liter_codegen::compile_with_schema(&ast, &self.schema)?;
    let n = vm.n_cursors;
    Ok(Statement {
        conn: self,
        vm,
        cursors: (0..n).map(|_| None).collect(),
        done: false,
    })
}
```

**Implement `Statement`**:
```rust
pub struct Statement<'conn> {
    conn: &'conn Connection,
    vm: Vdbe,
    cursors: Vec<Option<BTreeCursor<'conn>>>,  // lifetime tied to conn's btree
    done: bool,
}

impl Statement<'_> {
    pub fn step(&mut self) -> SqliteResult<StepResult> { ... }
    pub fn reset(&mut self) -> SqliteResult<()> { vm.reset(); done = false; Ok(()) }
    pub fn column_value(&self, col: usize) -> SqliteResult<Value> { ... }
    pub fn column_count(&self) -> usize { ... }
}
```

> [!IMPORTANT]
> `BTreeCursor<'bt>` borrows from `&'bt BTree`. Since `BTree` is owned by `Connection`,
> cursors in `Statement` must have a lifetime tied to `&'conn Connection`.
> The cleanest approach is to store `Vec<Option<BTreeCursor<'conn>>>` in `Statement<'conn>`.
> If Rust's borrow checker rejects the self-referential structure, use an `Arc<BTree>`
> (which `BTree` already uses internally via `Arc<Mutex<Pager>>`) and open cursors
> from a cloned `Arc` rather than a borrow.

### Integration test

Add to `crates/sqlite3/src/lib.rs` tests:
```rust
#[test]
fn test_select_from_table() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE users (id INTEGER, name TEXT)", []).unwrap();
    conn.execute("INSERT INTO users VALUES (1, 'alice')", []).unwrap();
    conn.execute("INSERT INTO users VALUES (2, 'bob')", []).unwrap();
    
    let rows = conn.query("SELECT id, name FROM users", []).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0], Value::Int(1));
    assert_eq!(rows[0][1], Value::Text(b"alice".to_vec()));
    assert_eq!(rows[1][0], Value::Int(2));
}
```

---

## Phase 10 — DELETE, UPDATE, and Transaction Statements

### Parser additions

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/liter-parser/src/lib.rs)
- `parse_delete_stmt` — `DELETE FROM table [WHERE expr]` (body already in AST)
- `parse_update_stmt` — `UPDATE table SET col = expr [WHERE expr]`
- `parse_transaction_stmt` — `BEGIN [DEFERRED|IMMEDIATE|EXCLUSIVE]`, `COMMIT`, `ROLLBACK [TO SAVEPOINT name]`
- `parse_pragma_stmt` — `PRAGMA name [= value | (value)]`

### VDBE opcodes for mutation

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/liter-vdbe/src/lib.rs)
- `Delete` — calls `cursor.delete()` at current position. Already declared in enum.
- `SeekGe/SeekGt/SeekLe/SeekLt` — calls `cursor.move_to(key, SeekBias::*)`, jumps if not found.

### Codegen

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/liter-codegen/src/lib.rs)
- `compile_delete` — `OpenWrite` → `Rewind` → loop: `[WHERE check]` → `Delete` → `Next`
- `compile_update` — `OpenWrite` → `Rewind` → loop: `[WHERE check]` → read old row → `MakeRecord` with new values → `Insert` at same rowid → `Next`

### Connection wiring

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/sqlite3/src/lib.rs)
- Handle `Stmt::Begin`, `Stmt::Commit`, `Stmt::Rollback` in `execute` by calling `btree.begin_write()`, `btree.commit()`, `btree.rollback()`.
- Track transaction state in `Connection` to prevent double-begin.

---

## Phase 11 — ORDER BY, LIMIT, Built-in Functions

### ORDER BY (Sorter)

This requires a temporary sort buffer. SQLite uses an ephemeral B-tree (the `OpenEphemeral` + `SorterOpen` opcodes). Minimal approach:

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/liter-vdbe/src/lib.rs)
- Add `SorterOpen` — creates an in-memory `Vec<(sort_key_bytes, record_bytes)>` sort buffer
- Add `SorterInsert` — appends current row to the sort buffer
- Add `SorterSort` — sorts the buffer (stable sort)
- Add `SorterData` — copies next row from sorted buffer to registers
- Add `SorterNext` — advances sort cursor

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/liter-codegen/src/lib.rs)
- `compile_order_by` — append a sorter phase after the scan loop:
  `SorterOpen` → scan + `SorterInsert` → `SorterSort` → yield loop with `SorterData` + `ResultRow` + `SorterNext`

### LIMIT / OFFSET

Add `Limit` opcode — decrements a counter register; jumps to end when zero.

#### [MODIFY] codegen — emit `Limit` opcode after scan setup when `limit.is_some()`.

### Built-in Functions in VDBE

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/liter-vdbe/src/lib.rs)
- Implement `Function` opcode:
  - `p4 = P4::FuncDef(name)` — look up in a static function dispatch table
  - Read `p2` arguments from registers `[p3 .. p3+p2]`, write result to `p1`
  - Dispatch to `liter_functions::{func_abs, func_length, ...}`

#### [MODIFY] codegen — emit `Function` opcode for `Expr::Function` nodes.

Add missing built-in functions to `liter-functions`:
- `count`, `sum`, `avg` (aggregate)  
- `substr`, `replace`, `trim`, `ltrim`, `rtrim`
- `hex`, `quote`, `zeroblob`
- `printf` / `format`
- `like`, `glob`
- `instr`, `char`, `unicode`
- `date`, `time`, `datetime`, `strftime`, `julianday`

---

## Phase 12 — GROUP BY, HAVING, Aggregates

### VDBE aggregate opcodes

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/liter-vdbe/src/lib.rs)
- Implement `AggStep` — accumulate a value into an aggregate accumulator register
  - `p4 = P4::FuncDef("count"|"sum"|"avg"|"min"|"max")`
  - Reads `p2` args from register `p3`, updates accumulator at `p1`
- Implement `AggFinal` — finalize the accumulator in register `p1`, write result to `p2`

### Codegen for aggregates

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/liter-codegen/src/lib.rs)
- Detect aggregate functions in result columns
- Emit: scan loop → `AggStep` per aggregate → `AggFinal` → `ResultRow`
- For `GROUP BY`: use ephemeral B-tree (key = group-by key bytes, value = accumulator)
- For `HAVING`: compile `HAVING` predicate identically to `WHERE`, applied after `AggFinal`

---

## Phase 13 — C FFI Layer Completion

### liter-ffi

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/crates/liter-ffi/src/lib.rs)

The `Statement` type from Phase 9 makes the rest of these trivial to implement:

| Function | Implementation |
|----------|---------------|
| `sqlite3_prepare_v2` | Calls `conn.prepare(sql)`, boxes `Statement`, stores pointer |
| `sqlite3_step` | Calls `stmt.step()`, maps to `SQLITE_ROW` / `SQLITE_DONE` |
| `sqlite3_column_int64` | Calls `stmt.column_value(i)`, downcasts to i64 |
| `sqlite3_column_double` | Calls `stmt.column_value(i)`, downcasts to f64 |
| `sqlite3_column_text` | Calls `stmt.column_value(i)`, returns pointer to interned UTF-8 |
| `sqlite3_column_blob` | Calls `stmt.column_value(i)`, returns raw blob pointer |
| `sqlite3_column_bytes` | Returns byte length of last column value |
| `sqlite3_column_type` | Returns `SQLITE_INTEGER / SQLITE_FLOAT / SQLITE_TEXT / SQLITE_BLOB / SQLITE_NULL` |
| `sqlite3_column_count` | Calls `stmt.column_count()` |
| `sqlite3_column_name` | Returns column name from compiled query |
| `sqlite3_bind_int64` | Sets bind register for parameter slot |
| `sqlite3_bind_double` | Sets bind register for parameter slot |
| `sqlite3_bind_text` | Sets bind register for parameter slot |
| `sqlite3_bind_null` | Sets bind register for parameter slot |
| `sqlite3_bind_parameter_count` | Returns number of `?` parameters |
| `sqlite3_finalize` | Drops statement |
| `sqlite3_reset` | Calls `stmt.reset()` |
| `sqlite3_errmsg` | Returns last error string from connection |
| `sqlite3_changes` | Returns rows modified by last DML |
| `sqlite3_last_insert_rowid` | Returns last auto-incremented rowid |

---

## Phase 14 — Testing Infrastructure

### Differential Harness (activate)

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/tests/differential/src/lib.rs)
- Enable `feature = "differential"` by default in `Cargo.toml`
- Add real differential tests:
  ```rust
  #[test] fn diff_select_literal() { diff_exec("SELECT 1 + 1, 'hello'"); }
  #[test] fn diff_create_insert_select() {
      diff_exec_multi(&["CREATE TABLE t(x INT)", "INSERT INTO t VALUES(1)", "SELECT x FROM t"]);
  }
  ```
- Add proptest-based SQL fuzzing:
  ```rust
  proptest! { #[test] fn fuzz_select(sql in valid_select_arb()) { diff_exec(&sql); } }
  ```

### Format Compatibility Tests

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/tests/format_compat/src/lib.rs)
- Download / commit a small reference `.db` file created by C SQLite (e.g., a Chinook subset)
- Add test: `open_and_read_reference_db` using `Connection::open(path)`

### OOM Injection

#### [MODIFY] [lib.rs](file:///Users/sri/Developer/liter-rs/tests/oom/src/lib.rs)
- Wire counting allocator wrapper in `liter-alloc`
- Run key operations with decreasing allocation budget; verify `Err(NoMem)` not panic

### Fuzz Targets

#### [NEW] `fuzz/` directory
```
fuzz/
  fuzz_targets/
    fuzz_tokenizer.rs    — random bytes to tokenizer
    fuzz_parser.rs       — random bytes to parser
    fuzz_full_db.rs      — random SQL through full execute path
    fuzz_btree.rs        — random insert/delete/seek sequences
```

### CI Pipeline

#### [NEW] `.github/workflows/ci.yml`
```yaml
jobs:
  test:     cargo test --workspace --all-features
  clippy:   cargo clippy --workspace -- -D warnings
  fmt:      cargo fmt --check
  miri:     cargo +nightly miri test -p liter-alloc -p liter-record
  fuzz:     cargo fuzz run fuzz_parser -- -max_total_time=60
  audit:    cargo audit
```

---

## Phase 15 — Extensions (Phase 4 Scope)

### `liter-functions` completion
- Add `count(*)`/`sum`/`avg` aggregates
- Add all string functions (`substr`, `replace`, `trim`, `printf`, etc.)
- Add date/time functions using system clock + `strftime`

### `liter-json`
- Complete `json()`, `json_extract()`, `json_object()`, `json_array()`, `json_each()` table-valued function

### `liter-fts5`
- Full-text search tokenizer, inverted index, `MATCH` operator plumbing

### `liter-rtree`
- 2D R-tree, `rtree` virtual table, spatial queries

### `liter-session`
- Session/changeset recording API (`sqlite3session_*`)

---

## Open Questions

> [!IMPORTANT]
> **Cursor lifetime in `Statement`**: `BTreeCursor<'bt>` borrows `&'bt BTree`.
> Since `BTree` is owned by `Connection`, `Statement<'conn>` cursors must borrow from `conn`.
> If self-referential structures become a problem, the cleanest fix is to change `BTree`
> to hold `Arc<Pager>` (already done) and make `BTreeCursor` hold a cloned `Arc<BTree>`
> instead of a `&'bt BTree` reference. Should we make this change as part of Phase 9,
> or work around it with `unsafe` pin + self-reference?

> [!IMPORTANT]
> **`sqlite_master` persistence**: `CREATE TABLE` currently stores schema only in the
> in-memory `Schema` hash map. A real SQLite file opened after restart won't see the tables.
> Should we implement `sqlite_master` row insertion in Phase 9 (schema persistence) or defer
> to a later phase? This is required for `Connection::open(path)` to be useful.

> [!IMPORTANT]
> **Parameterized queries**: `?`, `?1`, `:name` bind parameters. The AST has `Expr::Bind`
> and the `IntoParams` trait exists. Should bind parameter compilation and `Statement::bind_*`
> be part of Phase 9 or Phase 13?

---

## Execution Order Summary

```
Phase 7  →  VDBE read opcodes (OpenRead, Rewind, Next, Column, Rowid, If, IfNot, IsNull)
Phase 8  →  Codegen: SELECT FROM + WHERE clause emission
Phase 9  →  Connection: end-to-end SELECT, implement prepare/Statement
Phase 10 →  DELETE, UPDATE, transaction statements
Phase 11 →  ORDER BY, LIMIT, Function opcode, built-in functions
Phase 12 →  GROUP BY, HAVING, aggregate opcodes
Phase 13 →  C FFI layer (sqlite3_prepare_v2, sqlite3_column_*, sqlite3_bind_*)
Phase 14 →  Testing infra: differential harness, fuzz targets, CI, format-compat
Phase 15 →  Extensions (JSON, FTS5, RTree, Session)
```

After Phase 9, `SELECT id, name FROM users WHERE id > 1` works end-to-end.  
After Phase 13, the C `sqlite3.h` API is drop-in compatible.  
After Phase 14, the differential harness continuously guards correctness.
