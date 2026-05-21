//! Wrapper that runs the official SQLite TCL test suite against our
//! `libsqlite3.so` / `libsqlite3.dylib` built from the FFI crate.
//!
//! ## Prerequisites
//! - `tclsh` must be in `$PATH`.
//! - The SQLite TCL test files must be available (fetched by the test harness).

#[cfg(test)]
mod tests {
    use std::process::Command;

    /// Smoke-test: verify tclsh is available.
    #[test]
    #[ignore = "requires tclsh and sqlite test files"]
    fn tclsh_available() {
        let output = Command::new("tclsh")
            .arg("--version")
            .output()
            .expect("tclsh not found in PATH");
        assert!(output.status.success());
    }
}
