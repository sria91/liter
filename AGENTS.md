# liter — Agent Instructions

`liter` is a from-scratch Rust reimplementation of SQLite3 (v3.53.x), organized as a Cargo workspace of 25+ crates mirroring the SQLite architecture layers.

## Project Context

- **Language**: Rust (stable toolchain, edition 2021)
- **License**: MIT OR Apache-2.0
- **Architecture**: VFS → Pager → B-Tree → Record → VDBE → Parser/Codegen → FFI
- **Naming**: Production crates use the `liter-` prefix (e.g., `liter-pager`, `liter-vdbe`, `liter-ffi`); test and benchmark packages use descriptive names (`differential`, `format-compat`, `tcl-suite`, `oom-tests`, `benches`)
- **Entry points**: `crates/liter` (Rust API), `crates/liter-ffi` (C ABI), `tools/liter-shell` (CLI)
- **Testing**: Unit tests, differential harness (`tests/differential`), TCL conformance (`tests/tcl_suite`), OOM injection (`tests/oom`), format compat (`tests/format_compat`), fuzz targets (`fuzz/`)

## Coding Conventions

- Run `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test --all-targets` before committing.
- Every `unsafe` block must include a `// SAFETY:` comment.
- Each crate's `lib.rs` starts with `//!` module-level doc comments describing purpose and which C SQLite source file(s) it mirrors.
- Prefer `thiserror` for error types; return `Result` rather than panicking.

## Codebase Memory MCP

**Use Codebase Memory MCP graph tools first for codebase exploration — before reading files or making code changes.**

1. Call `list_projects` to discover the correct project name.
2. Call `get_architecture(project)` to understand the codebase structure.
3. Use `search_graph` to find relevant symbols, `trace_path` for call chains.
4. Use `get_code_snippet` to read specific function implementations.
5. Use `check_index_coverage` to verify graph coverage for files you cite.
6. Only use `read_file` / grep when you need exact raw content or when graph coverage is insufficient.
