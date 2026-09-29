# Contributing to liter

Thank you for your interest in contributing to `liter`! This document describes the development workflow, testing expectations, and PR guidelines.

## Development Setup

1. **Clone the repository**:
   ```bash
   git clone https://github.com/<your-fork>/liter.git
   cd liter
   ```

2. **Install the Rust toolchain** (the repo includes a `rust-toolchain.toml`):
   ```bash
   rustup show   # automatically installs the pinned toolchain
   ```

3. **Build the workspace**:
   ```bash
   cargo build --workspace
   ```

## Code Quality

Before submitting a PR, ensure:

```bash
# Formatting
cargo fmt --all -- --check

# Lints
cargo clippy --all-targets --all-features -- -D warnings

# Unit tests
cargo test --all-targets
```

All three checks run in CI and must pass.

## Testing

`liter` uses a multi-layered testing strategy:

| Suite | Command | Purpose |
|---|---|---|
| Unit tests | `cargo test --workspace` | Per-crate unit tests |
| Differential | `cargo test -p differential` | Verifies output matches C SQLite (via `rusqlite`) |
| Format compat | `cargo test -p format-compat` | Ensures on-disk format compatibility |
| OOM injection | `cargo test -p oom` | Tests behavior under allocation failure |
| TCL conformance | See CI workflow | Runs the official SQLite TCL test suite |
| Fuzz targets | `cargo fuzz run <target>` | libfuzzer-based fuzz testing |

When adding new functionality, include unit tests in the relevant crate. For changes that affect query results or the on-disk format, add or update differential tests.

## Project Structure

The workspace is organized into layers that mirror the SQLite architecture:

```
crates/
├── liter-alloc        # Memory allocator traits (sqlite3_mem_methods)
├── liter-sync         # Mutex and concurrency primitives
├── liter-unicode      # UTF-8/16, case folding
├── liter-fmt          # printf/strftime formatting
├── liter-config       # Global configuration singleton
├── liter-vfs          # VFS trait definitions
├── liter-vfs-unix     # Unix VFS (POSIX locks)
├── liter-vfs-win      # Windows VFS (LockFileEx)
├── liter-vfs-mem      # In-memory VFS (:memory: databases)
├── liter-pager        # Page cache, rollback journal, WAL
├── liter-btree        # B-tree read/write/cursor
├── liter-record       # Row encoding/decoding
├── liter-tokenizer    # SQL tokenizer (logos)
├── liter-parser       # Recursive-descent SQL parser
├── liter-ast          # AST node types
├── liter-resolve      # Name resolution and type affinity
├── liter-optimizer    # Query planner (WHERE analysis)
├── liter-codegen      # VDBE bytecode generator
├── liter-vdbe         # Virtual Database Engine (bytecode VM)
├── liter-functions    # Built-in SQL functions
├── liter-schema       # Schema catalog
├── liter-ffi          # C ABI compatibility (libsqlite3 drop-in)
├── liter-fts5         # Full-text search extension
├── liter-json         # JSON1 extension
├── liter-rtree        # R*Tree spatial index extension
├── liter-session      # Session/changeset extension
└── liter              # Unified public Rust API
tools/
└── liter-shell        # Interactive CLI (mirrors sqlite3 shell)
```

## Pull Request Guidelines

1. **Branch from `main`** and keep PRs focused on a single change.
2. **Write clear commit messages** describing what changed and why.
3. **Add tests** for new functionality or bug fixes.
4. **Run the full check suite** (`fmt`, `clippy`, `test`) before pushing.
5. **Keep unsafe to a minimum**. Any new `unsafe` block must include a `// SAFETY:` comment explaining the invariant.

## License

By contributing, you agree that your contributions will be licensed under the same dual license as the project: MIT OR Apache-2.0.
