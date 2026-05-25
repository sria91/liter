# SQLite3 v3.53.x → Rust: Comprehensive Porting Plan

> **Scope**: Full reimplementation of SQLite3 in safe + unsafe Rust, targeting
> API-level compatibility with the C library, with comprehensive test coverage
> at every layer.

---

## Table of Contents

1. [Project Overview](#1-project-overview)
2. [Architecture Decomposition](#2-architecture-decomposition)
3. [Phase Plan](#3-phase-plan)
4. [Module-by-Module Porting Guide](#4-module-by-module-porting-guide)
5. [Rust Crate Structure](#5-rust-crate-structure)
6. [C API Compatibility Layer](#6-c-api-compatibility-layer)
7. [Testing Strategy](#7-testing-strategy)
8. [Safety & Correctness Guarantees](#8-safety--correctness-guarantees)
9. [Performance Targets](#9-performance-targets)
10. [Tooling & CI/CD](#10-tooling--cicd)
11. [Risk Register](#11-risk-register)
12. [Milestones & Timeline](#12-milestones--timeline)

---

## 1. Project Overview

### Goals

| Goal | Detail |
|------|--------|
| **Functional parity** | Pass the full SQLite test suite (TCL + OOM + fuzz) |
| **ABI compatibility** | Drop-in `libsqlite3.so` / `.dylib` / `.dll` replacement |
| **Memory safety** | Eliminate all buffer overflows, use-after-free, and data races |
| **No unsafe by default** | `unsafe` restricted to OS I/O, FFI boundaries, and performance-critical hot paths with documented invariants |
| **Idiomatic Rust API** | Publish a `liter-rs` crate with ergonomic, ownership-based API |

### Non-Goals (v1.0)

- Extensions written in C will not be ported (they work through the existing extension API).
- The optional FTS5 / JSON / RTREE loadable extensions are **Phase 3** scope.
- WASM target is **Phase 4** scope.

### Reference Materials

- SQLite source: `https://sqlite.org/src` (tag `version-3.53.0`)
- SQLite file format spec: `https://sqlite.org/fileformat2.html`
- SQLite opcode reference: `https://sqlite.org/opcode.html`
- SQLite WAL spec: `https://sqlite.org/wal.html`

---

## 2. Architecture Decomposition

SQLite is a self-contained ~150 kLOC C program. The logical subsystems are:

```
┌──────────────────────────────────────────────────────────────┐
│                     C API  (sqlite3.h)                        │
├──────────────┬───────────────────────────────────────────────┤
│  SQL Parser  │  Tokenizer → Parser (Lemon LALR) → AST        │
├──────────────┼───────────────────────────────────────────────┤
│  Query Plan  │  Resolver → Optimizer → Code Generator        │
├──────────────┼───────────────────────────────────────────────┤
│    VDBE      │  Virtual Database Engine (bytecode VM)         │
├──────────────┼───────────────────────────────────────────────┤
│  B-Tree      │  In-memory and on-disk B-tree pages            │
├──────────────┼───────────────────────────────────────────────┤
│   Pager      │  Page cache, WAL, rollback journal             │
├──────────────┼───────────────────────────────────────────────┤
│    VFS       │  Virtual File System (unix / win32 / memdb)    │
├──────────────┼───────────────────────────────────────────────┤
│   Utilities  │  Memory allocator, UTF-8/16, printf, mutex     │
└──────────────┴───────────────────────────────────────────────┘
```

Each subsystem maps to one or more Rust crates (see §5).

---

## 3. Phase Plan

### Phase 0 — Foundation (Weeks 1–4)

Set up infrastructure before writing a single line of port code.

- [ ] Create the Cargo workspace with all crate stubs.
- [ ] Write a **differential testing harness** that runs queries against both the
      C SQLite and our Rust implementation simultaneously and diffs results.
- [ ] Integrate the official SQLite TCL test suite via a thin Rust-spawned subprocess.
- [ ] Configure CI: GitHub Actions with `cargo test`, `cargo clippy --deny warnings`,
      `cargo fmt --check`, `cargo miri test` (subset), address-sanitizer builds.
- [ ] Set up `criterion` benchmarks mirroring the SQLite speedtest1 benchmark.
- [ ] Import SQLite's `testfixture` fuzz corpus into libFuzzer / cargo-fuzz targets.

### Phase 1 — Bottom-Up Infrastructure (Weeks 5–12)

Port the stateless, easily-testable utilities first.

| Module | C files | Rust crate |
|--------|---------|------------|
| Memory allocator | `mem0–5.c`, `mem_malloc.c` | `liter-alloc` |
| Mutex / atomics | `mutex.c`, `mutex_unix.c` | `liter-sync` |
| UTF-8/16 codec | `utf.c` | `liter-unicode` |
| Printf / strftime | `printf.c`, `date.c` | `liter-fmt` |
| Hash tables | `hash.c` | (inline in `liter-util`) |
| Global config | `global.c`, `config.c` | `liter-config` |

### Phase 2 — Storage Engine (Weeks 13–26)

The most critical and complex subsystem.

| Module | C files | Rust crate |
|--------|---------|------------|
| VFS (Unix) | `os_unix.c` | `liter-vfs-unix` |
| VFS (Win32) | `os_win.c` | `liter-vfs-win` |
| VFS (in-memory) | `memdb.c` | `liter-vfs-mem` |
| Pager / WAL | `pager.c`, `wal.c` | `liter-pager` |
| B-Tree | `btree.c`, `btreeInt.h` | `liter-btree` |
| Record format | `record.c` | `liter-record` |

### Phase 3 — Query Engine (Weeks 27–40)

| Module | C files | Rust crate |
|--------|---------|------------|
| Tokenizer | `tokenize.c` | `liter-tokenizer` |
| Parser | `parse.y` (Lemon) | `liter-parser` (hand-written recursive-descent or lalrpop) |
| AST / Schema | `build.c`, `table.c`, etc. | `liter-ast` |
| Resolver | `resolve.c` | `liter-resolve` |
| Query optimizer | `where.c`, `whereInt.h` | `liter-optimizer` |
| Code generator | `select.c`, `insert.c`, `delete.c`, `update.c`, `trigger.c` | `liter-codegen` |
| VDBE | `vdbe.c`, `vdbeapi.c`, `vdbeaux.c`, `vdbemem.c`, `vdbesort.c`, `vdbeblob.c` | `liter-vdbe` |

### Phase 4 — C API Compatibility & Extensions (Weeks 41–52)

| Task | Detail |
|------|--------|
| `sqlite3.h` FFI layer | `#[no_mangle]` extern "C" wrappers in `liter-ffi` |
| Built-in functions | `func.c`, `math.c` — ported to `liter-functions` |
| FTS5 | `fts5.c` family |
| JSON1 | `json.c` |
| R*Tree | `rtree.c` |
| Session extension | `session.c` |
| WASM target | `wasm` feature flag, `wasm-bindgen` glue |

---

## 4. Module-by-Module Porting Guide

### 4.1 Memory Allocator (`liter-alloc`)

**C behavior**: SQLite uses a pluggable allocator (`sqlite3_mem_methods`). It
defaults to system `malloc` but can use a static buffer ("memsys3/5") or a
debug allocator that poison-fills memory.

**Rust approach**:

```rust
// Define the allocator trait mirroring sqlite3_mem_methods
pub trait SqliteAlloc: Send + Sync {
    fn malloc(&self, n: usize) -> *mut u8;
    fn free(&self, ptr: *mut u8);
    fn realloc(&self, ptr: *mut u8, n: usize) -> *mut u8;
    fn size(&self, ptr: *mut u8) -> usize;
    fn roundup(&self, n: usize) -> usize;
    fn init(&self) -> Result<(), AllocError>;
    fn shutdown(&self);
}

// Default implementation delegates to the global Rust allocator
pub struct SystemAlloc;
impl SqliteAlloc for SystemAlloc { ... }

// Static-buffer implementation for embedded targets
pub struct StaticAlloc<const N: usize> { buf: UnsafeCell<[u8; N]>, ... }
```

**Tests**:
- Allocate/free 10 million random-sized blocks; check for leaks via custom
  tracking wrapper.
- Mirror C's `memsys5` corruption tests.
- Fuzz: random malloc/realloc/free sequences.

---

### 4.2 VFS Layer (`liter-vfs-unix`, `liter-vfs-win`, `liter-vfs-mem`)

**C behavior**: The VFS provides `xOpen`, `xDelete`, `xAccess`, `xLock`,
`xUnlock`, `xSync`, `xFileSize`, `xRead`, `xWrite`, `xTruncate`, `xClose`.
File locking uses POSIX advisory locks (unix) or LockFileEx (win).

**Rust approach**:

```rust
pub trait Vfs: Send + Sync {
    type File: VfsFile;
    fn open(&self, path: &Path, flags: OpenFlags) -> io::Result<Self::File>;
    fn delete(&self, path: &Path, sync_dir: bool) -> io::Result<()>;
    fn access(&self, path: &Path, flags: AccessFlags) -> io::Result<bool>;
    fn full_pathname(&self, path: &Path) -> io::Result<PathBuf>;
    fn randomness(&self, buf: &mut [u8]);
    fn sleep(&self, micros: u64);
    fn current_time(&self) -> f64;
}

pub trait VfsFile: Send {
    fn read(&mut self, buf: &mut [u8], offset: u64) -> io::Result<usize>;
    fn write(&mut self, buf: &[u8], offset: u64) -> io::Result<()>;
    fn truncate(&mut self, size: u64) -> io::Result<()>;
    fn sync(&mut self, flags: SyncFlags) -> io::Result<()>;
    fn file_size(&self) -> io::Result<u64>;
    fn lock(&mut self, level: LockLevel) -> io::Result<()>;
    fn unlock(&mut self, level: LockLevel) -> io::Result<()>;
    fn check_reserved_lock(&self) -> io::Result<bool>;
    fn device_characteristics(&self) -> DeviceCharacteristics;
    fn sector_size(&self) -> u32;
}
```

**Tests**:
- Simulate power-loss mid-write using the in-memory VFS and verify recovery.
- Multi-process locking tests using `std::process::Command`.
- WAL mode concurrency: 8 writer threads + 4 reader threads on the in-memory VFS.
- Fuzz: random sequences of open/lock/write/crash/reopen.

---

### 4.3 Pager (`liter-pager`)

The pager is the most complex module (~6,000 LOC in C). It manages:

- Page cache (LRU eviction, dirty tracking)
- Rollback journal (header, page records, master journal pointer)
- WAL (write-ahead log, checkpointing, read marks)
- Savepoints
- Error recovery

**Rust approach**:

```rust
pub struct Pager {
    vfs_file: Box<dyn VfsFile>,
    page_size: u16,
    cache: PageCache,
    journal: Option<Journal>,
    wal: Option<Wal>,
    lock_state: LockState,
    read_only: bool,
    // ... ~40 more fields mirroring liter_pager
}

impl Pager {
    pub fn acquire(&mut self, pgno: PageNumber) -> Result<PageRef>;
    pub fn lookup(&self, pgno: PageNumber) -> Option<PageRef>;
    pub fn begin_write(&mut self) -> Result<()>;
    pub fn commit_phase_one(&mut self, super_journal: Option<&Path>) -> Result<()>;
    pub fn commit_phase_two(&mut self) -> Result<()>;
    pub fn rollback(&mut self) -> Result<()>;
    pub fn open_savepoint(&mut self, n: usize) -> Result<()>;
    pub fn savepoint_rollback(&mut self, n: usize) -> Result<()>;
    pub fn savepoint_release(&mut self, n: usize) -> Result<()>;
    pub fn checkpoint(&mut self, mode: CheckpointMode) -> Result<(u32, u32)>;
    pub fn set_page_size(&mut self, size: u16) -> Result<()>;
    pub fn move_page(&mut self, page: PageRef, new_pgno: PageNumber) -> Result<()>;
}
```

**Tests** (this module gets the heaviest test investment):
- Crash simulation: truncate journal at every possible byte offset, verify recovery.
- WAL wrap-around: fill WAL past the 1000-frame default, checkpoint, repeat.
- Savepoint nesting: 256-deep savepoints with interleaved commits and rollbacks.
- Concurrent readers during WAL checkpoint.
- Sector-aligned sync with simulated sector sizes of 512, 4096, 65536.
- Full OOM injection test: every pager function called with allocation failing at each Nth allocation.

---

### 4.4 B-Tree (`liter-btree`)

**C behavior**: Implements both table B-trees (row-id keyed) and index B-trees
(arbitrary key). Uses the pager for page management. Pages are either interior
(pointers only) or leaf (data). The on-disk format is fully specified by the
SQLite file format document.

**Rust approach**:

```rust
pub struct BTree {
    pager: Arc<Mutex<Pager>>,
    meta: [u32; BTREE_N_META],
    // ...
}

pub struct BTreeCursor<'bt> {
    btree: &'bt BTree,
    root_page: PageNumber,
    // Page stack for traversal
    page_stack: ArrayVec<CursorFrame, MAX_DEPTH>,
    state: CursorState,
}

impl BTreeCursor<'_> {
    pub fn move_to_first(&mut self) -> Result<bool>;
    pub fn move_to_last(&mut self) -> Result<bool>;
    pub fn move_to(&mut self, key: &[u8], bias: SeekBias) -> Result<SeekResult>;
    pub fn next(&mut self) -> Result<bool>;
    pub fn previous(&mut self) -> Result<bool>;
    pub fn key(&self) -> Result<&[u8]>;
    pub fn data(&self) -> Result<&[u8]>;
    pub fn insert(&mut self, key: &[u8], data: &[u8], append: bool) -> Result<()>;
    pub fn delete(&mut self) -> Result<()>;
}
```

**Tests**:
- Insert 10M sequential rows; verify all can be read back in order.
- Insert 10M random rows (uuid keys); verify sorted traversal.
- Delete every other row; verify consistency.
- Verify on-disk page format byte-for-byte against reference C SQLite.
- Overflow page chains: values larger than page_size - 100.
- Fill a 2-level B-tree, then make it degenerate via targeted deletes, verify rebalance.

---

### 4.5 VDBE (`liter-vdbe`)

The VDBE is SQLite's bytecode virtual machine — roughly 200 opcodes.

**Rust approach**: Use a big `match` dispatch loop with computed states, similar
to how CPython / Lua implement their VMs. Avoid a vtable dispatch per opcode.

```rust
pub struct Vdbe {
    ops: Vec<VdbeOp>,
    pc: usize,                      // program counter
    regs: Vec<Mem>,                 // register file
    cursors: Vec<Option<VdbeCursor>>,
    call_stack: Vec<usize>,         // for subroutine opcodes
    error_action: ErrorAction,
    // ...
}

pub enum Mem {
    Null,
    Int(i64),
    Real(f64),
    Text(Arc<str>, Encoding),
    Blob(Arc<[u8]>),
    Zero(i64),  // zero-blob of given length
}

impl Vdbe {
    pub fn step(&mut self) -> Result<StepResult>;
    pub fn reset(&mut self) -> Result<()>;
    pub fn finalize(self) -> Result<()>;
    // ... bind_* / column_* access
}
```

**Opcode implementation strategy**:
- Port opcodes in dependency order (arithmetic first, then memory, then cursor ops).
- Each opcode gets its own unit-test function exercising edge cases.
- Use the differential harness: run the same `sqlite3_prepare/step` against
  both C SQLite and our VDBE; compare result rows and return codes.

---

### 4.6 Parser (`liter-parser`)

**C behavior**: SQLite uses the Lemon LALR(1) parser generator (`parse.y`,
~1,000 grammar rules) feeding into a hand-written tokenizer (`tokenize.c`).

**Rust approach**: Hand-write a recursive-descent parser (more maintainable
than a generated one) backed by a fast tokenizer.

Alternatives considered:
- `lalrpop`: Good fit but adds build complexity.
- `nom` / `pest`: Would work but SQL grammar is large.
- **Recommendation**: Hand-written recursive-descent with `logos` for the tokenizer.

```rust
// Tokenizer using `logos`
#[derive(Logos, Debug, PartialEq)]
enum Token<'src> {
    #[token("SELECT", ignore(case))] Select,
    #[token("FROM", ignore(case))] From,
    // ... ~150 keywords
    #[regex(r#"'([^'\\]|\\.)*'"#)] StringLit(&'src str),
    #[regex(r"[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?")] Number(&'src str),
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*")] Ident(&'src str),
    // ...
}

// AST types
pub enum Stmt {
    Select(SelectStmt),
    Insert(InsertStmt),
    Update(UpdateStmt),
    Delete(DeleteStmt),
    Create(CreateStmt),
    Drop(DropStmt),
    // ... 30+ statement types
}
```

**Tests**:
- Parse every SQL statement in the official SQLite test corpus.
- Round-trip: parse → pretty-print → re-parse → compare ASTs.
- Error recovery: confirm informative error messages on malformed SQL.
- Fuzz: random byte sequences fed to the tokenizer + parser.

---

## 5. Rust Crate Structure

```
liter-rs/                     (Cargo workspace root)
├── crates/
│   ├── liter-alloc/          # Memory allocation traits + implementations
│   ├── liter-sync/           # Mutex, condvar, atomic wrappers
│   ├── liter-unicode/        # UTF-8/16 encode/decode, case folding
│   ├── liter-fmt/            # printf / strftime implementations
│   ├── liter-config/         # Global configuration (liter_config equivalent)
│   ├── liter-vfs/            # VFS trait definitions
│   ├── liter-vfs-unix/       # Unix VFS implementation
│   ├── liter-vfs-win/        # Windows VFS implementation
│   ├── liter-vfs-mem/        # In-memory VFS (tests + :memory: db)
│   ├── liter-pager/          # Page cache, WAL, rollback journal
│   ├── liter-btree/          # B-tree read/write/cursor
│   ├── liter-record/         # Record encoding/decoding
│   ├── liter-tokenizer/      # SQL tokenizer
│   ├── liter-parser/         # SQL parser → AST
│   ├── liter-ast/            # AST node types, schema representation
│   ├── liter-resolve/        # Name resolution, type affinity
│   ├── liter-optimizer/      # Query planner (WHERE analysis, index selection)
│   ├── liter-codegen/        # VDBE bytecode generation
│   ├── liter-vdbe/           # Virtual machine execution
│   ├── liter-functions/      # Built-in SQL functions
│   ├── liter-schema/         # Schema management, table/index catalog
│   ├── liter-ffi/            # #[no_mangle] C ABI compatibility layer
│   ├── liter-fts5/           # Full-text search (Phase 3)
│   ├── liter-json/           # JSON1 extension (Phase 3)
│   ├── liter-rtree/          # R*Tree extension (Phase 3)
│   └── sqlite3/                # Unified facade crate (public Rust API)
├── fuzz/
│   ├── fuzz_tokenizer/
│   ├── fuzz_parser/
│   ├── fuzz_pager/
│   ├── fuzz_btree/
│   ├── fuzz_vdbe/
│   └── fuzz_full_db/
├── benches/
│   ├── speedtest1.rs           # Port of sqlite speedtest1.c
│   ├── insert_bench.rs
│   ├── select_bench.rs
│   └── wal_bench.rs
├── tests/
│   ├── differential/           # Runs same SQL against C + Rust, diffs output
│   ├── format_compat/          # Opens .db files created by C SQLite
│   ├── tcl_suite/              # Wrapper that runs the official TCL tests
│   └── oom/                    # Out-of-memory injection tests
└── tools/
    ├── liter-shell/          # Rust port of the sqlite3 shell
    └── lemon-to-rust/          # Optional: Lemon .y → LALR table generator
```

---

## 6. C API Compatibility Layer

The `liter-ffi` crate exports the full `sqlite3.h` surface as `extern "C"` symbols, enabling drop-in replacement.

```rust
// liter-ffi/src/lib.rs

use liter::Connection;
use std::ffi::{CStr, c_char, c_int, c_void};

/// Opaque handle — the public sqlite3* pointer
pub struct sqlite3(Connection);

#[no_mangle]
pub unsafe extern "C" fn sqlite3_open(
    filename: *const c_char,
    ppDb: *mut *mut sqlite3,
) -> c_int {
    let path = CStr::from_ptr(filename).to_string_lossy();
    match Connection::open(path.as_ref()) {
        Ok(conn) => {
            *ppDb = Box::into_raw(Box::new(sqlite3(conn)));
            SQLITE_OK
        }
        Err(e) => e.result_code(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn sqlite3_close(db: *mut sqlite3) -> c_int {
    if !db.is_null() {
        drop(Box::from_raw(db));
    }
    SQLITE_OK
}

// ... all ~250 public API functions
```

**Verification**: The `liter-ffi` crate is tested by:
1. Compiling a set of C programs (from the official test suite) against our `.so`.
2. Running the Python `sqlite3` module's test suite via ctypes-level shim.
3. Running the Ruby `sqlite3` gem tests.

---

## 7. Testing Strategy

### 7.1 Test Pyramid

```
                    ┌──────────────┐
                    │  E2E / TCL   │  ~57,000 tests from official suite
                    │  Test Suite  │
                   ┌┴──────────────┴┐
                   │  Differential  │  Same SQL → C vs Rust, diff results
                   │  Harness       │
                  ┌┴────────────────┴┐
                  │  Integration     │  Per-module tests with real pager/btree
                  │  Tests           │
                 ┌┴──────────────────┴┐
                 │  Unit Tests         │  Pure function tests, no I/O
                 │  (cargo test)       │
                ┌┴────────────────────┴┐
                │  Property / Fuzz     │  proptest + cargo-fuzz
                │  Tests               │
                └──────────────────────┘
```

### 7.2 Differential Harness

```rust
// tests/differential/src/lib.rs
use rusqlite::Connection as CConn;   // links against C SQLite
use liter::Connection as RConn;    // our Rust implementation

pub fn diff_exec(sql: &str, params: &[Value]) {
    let c_conn = CConn::open_in_memory().unwrap();
    let r_conn = RConn::open_in_memory().unwrap();

    let c_rows = c_conn.query(sql, params).unwrap();
    let r_rows = r_conn.query(sql, params).unwrap();

    assert_eq!(c_rows, r_rows,
        "Differential failure for SQL:\n{sql}");
}

// Proptest: generate random valid SQL and diff
proptest! {
    #[test]
    fn fuzz_select(sql in sql_arb()) {
        diff_exec(&sql, &[]);
    }
}
```

### 7.3 OOM Injection

```rust
// Every allocation goes through a counting wrapper.
// Test: call function under test with limit = 1, 2, 3, ... until no OOM.
// Verify: no panic, no memory corruption, SQLITE_NOMEM returned.

pub fn oom_test<F: Fn() -> Result<()>>(f: F) {
    for limit in 1.. {
        ALLOC_FAIL_AFTER.store(limit, Ordering::SeqCst);
        match f() {
            Err(SqliteError::NoMem) => continue,
            Err(e) => panic!("Unexpected error at limit {limit}: {e}"),
            Ok(()) => break,  // limit was high enough, done
        }
    }
}
```

### 7.4 Official TCL Test Suite Integration

```rust
// tests/tcl_suite/src/main.rs
// Builds our libsqlite3.so, then spawns tclsh against the official test files.

fn run_tcl_suite() {
    let lib = build_ffi_crate();
    let tclsh = find_tclsh();
    let suite_dir = fetch_sqlite_test_files("3.53.0");

    let output = Command::new(tclsh)
        .env("SQLITE_LIBRARY", &lib)
        .arg(suite_dir.join("testrunner.tcl"))
        .args(["--status", "full"])
        .output()
        .unwrap();

    assert!(output.status.success(),
        "TCL suite failed:\n{}", String::from_utf8_lossy(&output.stdout));
}
```

### 7.5 File Format Compatibility Tests

```rust
// Open .db files created by C SQLite, read all tables, compare.
// Covers: legacy page sizes, freelist pages, overflow, WAL-mode files.
#[test]
fn open_reference_database() {
    let conn = Connection::open("tests/fixtures/chinook.db").unwrap();
    let count: i64 = conn.query_one("SELECT COUNT(*) FROM tracks", []).unwrap();
    assert_eq!(count, 3503);
}
```

### 7.6 Fuzzing Targets

| Target | Corpus | Sanitizers |
|--------|--------|------------|
| `fuzz_tokenizer` | All SQL tokens from TCL suite | ASan + UBSan |
| `fuzz_parser` | All SQL statements from TCL suite | ASan |
| `fuzz_pager` | Valid + corrupted page sequences | ASan + MemSan |
| `fuzz_btree` | Key/value sequences | ASan |
| `fuzz_vdbe` | Compiled VDBE programs | ASan |
| `fuzz_full_db` | Arbitrary SQL strings | ASan + UBSan |
| `fuzz_file_format` | Random .db file bytes | ASan |

```toml
# fuzz/Cargo.toml
[dependencies]
libfuzzer-sys = "0.4"
arbitrary = { version = "1", features = ["derive"] }
liter = { path = "../crates/sqlite3" }
```

### 7.7 Miri Tests

Run safety-critical modules under Miri to catch undefined behavior:

```bash
cargo +nightly miri test -p liter-alloc
cargo +nightly miri test -p liter-unicode
cargo +nightly miri test -p liter-record
cargo +nightly miri test -p liter-vdbe -- --test-threads=1
```

---

## 8. Safety & Correctness Guarantees

### 8.1 `unsafe` Policy

Every `unsafe` block must have a `// SAFETY:` comment explaining the invariant.
Categories of permitted `unsafe`:

| Category | Justification |
|----------|---------------|
| Raw pointer in FFI boundary | Required for C ABI compatibility |
| `mmap` in Unix VFS | No safe Rust abstraction has equivalent performance |
| In-place page mutation | Pager guarantees exclusive lock; documented per call site |
| VDBE register type punning | Type tag always checked before access; enum ensures exhaustiveness |

Prohibited: `unsafe` for "convenience" or to avoid a borrow-checker fight.
The borrow checker finding a conflict is a signal to restructure.

### 8.2 Panic Policy

- `unwrap()` / `expect()` are forbidden in library code.
- All errors propagate via `Result<T, SqliteError>`.
- OOM in the allocator returns `Err(SqliteError::NoMem)`, never panics.
- Arithmetic overflow: use `checked_*` or `saturating_*` everywhere.

### 8.3 Thread Safety

- `Connection` is `Send` but not `Sync` (same semantics as C SQLite's
  `SQLITE_THREADSAFE=1` mode).
- For shared-cache mode, `Arc<Mutex<SharedCache>>` guards the shared state.
- WAL reader/writer concurrency uses a `RwLock<WalIndex>` backed by the shm file.

---

## 9. Performance Targets

All benchmarks run on the same hardware as the C reference. Targets at v1.0:

| Benchmark | C SQLite | Target | Stretch |
|-----------|----------|--------|---------|
| speedtest1 (in-memory) | baseline | ≤ 110% | ≤ 100% |
| Sequential INSERT 1M rows | baseline | ≤ 115% | ≤ 100% |
| Random SELECT w/ index | baseline | ≤ 110% | ≤ 100% |
| Full-table scan 1M rows | baseline | ≤ 105% | ≤ 100% |
| WAL write throughput | baseline | ≤ 120% | ≤ 105% |
| Parse 10k SQL statements | baseline | ≤ 100% | ≤ 90% |

Profiling tools: `perf`, `heaptrack`, `cargo flamegraph`.

---

## 10. Tooling & CI/CD

### 10.1 Required Tools

```toml
# rust-toolchain.toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy", "rust-src"]
targets = ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc",
           "aarch64-apple-darwin", "wasm32-unknown-unknown"]
```

```bash
# Dev dependencies
cargo install cargo-fuzz cargo-criterion cargo-miri cargo-audit cargo-deny
```

### 10.2 GitHub Actions Pipeline

```yaml
# .github/workflows/ci.yml
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    steps:
      - run: cargo test --workspace --all-features
      - run: cargo clippy --workspace -- -D warnings
      - run: cargo fmt --check

  miri:
    runs-on: ubuntu-latest
    steps:
      - run: cargo +nightly miri test -p liter-alloc -p liter-unicode -p liter-record

  fuzz:
    runs-on: ubuntu-latest
    steps:
      - run: cargo fuzz run fuzz_parser -- -max_total_time=300
      - run: cargo fuzz run fuzz_full_db -- -max_total_time=300

  bench:
    runs-on: ubuntu-latest
    steps:
      - run: cargo criterion --workspace 2>&1 | tee bench-results.txt
      - uses: actions/upload-artifact@v4
        with: { path: bench-results.txt }

  tcl-suite:
    runs-on: ubuntu-latest
    steps:
      - run: sudo apt-get install -y tcl
      - run: cargo test -p liter-tcl-suite -- --nocapture

  audit:
    runs-on: ubuntu-latest
    steps:
      - run: cargo audit
      - run: cargo deny check
```

### 10.3 Code Coverage

```bash
cargo llvm-cov --workspace --html --open
# Target: ≥ 85% line coverage, ≥ 75% branch coverage
```

---

## 11. Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Pager correctness edge cases (WAL, crash recovery) | High | Critical | Heavy OOM/crash injection tests; diff against C on each commit |
| B-tree page format incompatibility | Medium | Critical | Byte-level format tests against C-generated files |
| VDBE opcode semantic drift | High | High | Differential harness runs on every push |
| Lemon grammar → recursive-descent translation gaps | Medium | High | Parse every SQL statement in TCL suite |
| Performance regression >20% | Medium | Medium | Criterion benchmarks on every PR; profile before optimizing |
| Unsafe code UB in hot paths | Low | Critical | Miri + AddressSanitizer in CI |
| C extension API breakage | Low | Medium | Run Python/Ruby/PHP adapter test suites against our .so |
| WAL shm concurrency bugs | Medium | High | Multi-process stress test with 16 concurrent writers |

---

## 12. Milestones & Timeline

| Week | Milestone | Deliverable |
|------|-----------|-------------|
| 4 | Foundation complete | CI green, differential harness running |
| 8 | Utilities ported | `liter-alloc`, `liter-sync`, `liter-unicode` passing unit + fuzz tests |
| 12 | VFS layer complete | Unix + in-memory VFS, all lock mode tests passing |
| 18 | Pager complete | WAL + rollback journal, crash-recovery tests passing |
| 26 | Storage engine complete | B-tree passing format-compat tests against C SQLite files |
| 32 | Tokenizer + Parser complete | Full SQL grammar, parser fuzz corpus exhausted |
| 38 | Query planner complete | Resolver + optimizer; EXPLAIN QUERY PLAN output matches C |
| 44 | VDBE complete | Differential harness passing for all TCL test SQL |
| 48 | C FFI layer complete | Python `sqlite3` module test suite passing |
| 50 | Full TCL suite green | ≥ 99% of 57k official tests passing |
| 52 | v1.0 Release | Published to crates.io; benchmarks within targets |

---

## Appendix A: Key C → Rust Translation Patterns

### Global mutable state

```c
// C: global variable
static int sqlite3_initialized = 0;
```

```rust
// Rust: OnceLock
static INITIALIZED: OnceLock<()> = OnceLock::new();
fn ensure_initialized() { INITIALIZED.get_or_init(|| { /* ... */ }); }
```

### Linked lists (pervasive in C SQLite)

```c
// C: intrusive linked list
struct BtCursor { BtCursor *pNext, *pPrev; ... };
```

```rust
// Rust: Vec or intrusive-collections crate
// Most SQLite lists are short (<10 elements); Vec is fine.
// For the pager's dirty-page list, use an intrusive list via raw pointers
// in a single `unsafe` module with a clear ownership contract.
```

### Out-parameters

```c
// C: out-parameter pattern
int sqlite3BtreeCursor(Btree*, int, int, ..., BtCursor **ppCur);
```

```rust
// Rust: return Result<Cursor, Error>
fn open_cursor(&self, table_num: i32, wrflag: bool) -> Result<BTreeCursor>;
```

### Error codes

```c
// C: int error codes
#define SQLITE_OK       0
#define SQLITE_ERROR    1
#define SQLITE_NOMEM    7
```

```rust
#[derive(Debug, thiserror::Error)]
pub enum SqliteError {
    #[error("SQL error: {0}")] Sql(String),
    #[error("out of memory")]  NoMem,
    #[error("I/O error: {0}")] Io(#[from] std::io::Error),
    // ... all 30+ error codes
}
```

---

## Appendix B: Recommended Crate Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `thiserror` | 2.x | Error type derivation |
| `logos` | 0.15 | Tokenizer |
| `arrayvec` | 0.7 | Fixed-size stack arrays (cursor page stack) |
| `parking_lot` | 0.12 | Faster Mutex / RwLock |
| `memmap2` | 0.9 | Memory-mapped file I/O (Unix VFS) |
| `criterion` | 0.5 | Benchmarking |
| `proptest` | 1.x | Property-based testing |
| `arbitrary` | 1.x | Structured fuzzing |
| `tempfile` | 3.x | Test temporary files |
| `rusqlite` | 0.32 | C SQLite bindings for differential harness |
| `once_cell` | 1.x | (or std `OnceLock`) Global initialization |
| `bitflags` | 2.x | VFS flags, open flags, device characteristics |

---

*Plan version: 1.0 — targeting SQLite 3.53.x*
*Estimated team size: 4–6 engineers, 12 months*