# liter

`liter` is a from-scratch, high-performance, strictly API-compatible reimplementation of SQLite3 (v3.53.x) written in Rust.

## Project Overview

`liter` aims to bring the reliability, performance, and ubiquity of SQLite to the Rust ecosystem while maintaining binary and ABI-level compatibility with the original C implementation where possible.

### Key Goals
- **API Compatibility**: Drop-in replacement for the SQLite3 C API.
- **Safety & Correctness**: Leverage Rust's memory safety guarantees without sacrificing performance.
- **Performance**: Match or exceed SQLite performance benchmarks.
- **Testability**: Comprehensive differential testing, OOM injection, and conformance suites.

## Architecture

`liter` follows the layered architecture of SQLite3:

1. **VFS (Virtual File System)**: Abstracted persistent storage (Unix, Windows, Memory).
2. **Pager**: Transaction-safe page management (WAL/Journal).
3. **B-Tree**: Page storage and indexing.
4. **Record**: Data format and parsing.
5. **VDBE (Virtual Database Engine)**: Bytecode VM for executing query plans.
6. **Parser & Codegen**: SQL parsing and query planning.
7. **FFI Layer**: C ABI compatibility.

## Getting Started

### Prerequisites
- Rust (latest stable recommended)

### Build
To build the workspace:
```bash
cargo build --workspace
```

### Testing
`liter` includes a comprehensive testing suite:
- **Unit Tests**: Run with `cargo test --workspace`.
- **Differential Harness**: Verifies `liter` against the C SQLite implementation.
- **TCL Conformance**: Ensures compatibility with SQLite's own extensive test suite.

## License

This project is licensed under either of:
- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)
(see LICENSE-MIT and LICENSE-APACHE files)

## Contributing

`liter` welcomes contributions! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for details on our workflow, testing requirements, and PR guidelines.
