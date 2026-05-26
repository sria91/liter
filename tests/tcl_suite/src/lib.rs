//! Wrapper that runs the official SQLite TCL test suite against our
//! `libsqlite3.so` / `libsqlite3.dylib` built from the FFI crate.
//!
//! ## Prerequisites
//! - `tclsh` must be in `$PATH`.
//! - The SQLite TCL test files must be available.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct TclRunSummary {
    pub success: bool,
    pub exit_code: i32,
    pub test_count: usize,
    pub tests: Vec<String>,
    pub stdout: String,
    pub stderr: String,
}

fn find_tclsh() -> Result<(), String> {
    let output = Command::new("tclsh")
        .arg("--version")
        .output()
        .map_err(|e| format!("tclsh not found in PATH: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err("tclsh exists but did not execute successfully".to_string())
    }
}

fn resolve_tcl_shell(sqlite_tcl_dir: &Path) -> Result<PathBuf, String> {
    if let Ok(raw) = env::var("LITER_TCL_SHELL") {
        let path = PathBuf::from(raw);
        if path.exists() {
            return Ok(path);
        }
        return Err(format!(
            "LITER_TCL_SHELL was set but path does not exist: {}",
            path.display()
        ));
    }

    let testfixture = sqlite_tcl_dir.join("testfixture");
    if testfixture.exists() {
        return Ok(testfixture);
    }

    #[cfg(target_os = "windows")]
    {
        let testfixture_exe = sqlite_tcl_dir.join("testfixture.exe");
        if testfixture_exe.exists() {
            return Ok(testfixture_exe);
        }
    }

    // Fallback to plain tclsh if testfixture is not available.
    find_tclsh()?;
    Ok(PathBuf::from("tclsh"))
}

fn read_required_env_path(var: &str) -> Result<PathBuf, String> {
    let val = env::var(var).map_err(|_| format!("missing required env var {var}"))?;
    let path = PathBuf::from(val);
    if !path.exists() {
        return Err(format!(
            "path from {var} does not exist: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn default_lib_path() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    #[cfg(target_os = "linux")]
    {
        return workspace_root.join("target/debug/libliter_ffi.so");
    }
    #[cfg(target_os = "macos")]
    {
        return workspace_root.join("target/debug/libliter_ffi.dylib");
    }
    #[cfg(target_os = "windows")]
    {
        return workspace_root.join("target/debug/liter_ffi.dll");
    }
    #[allow(unreachable_code)]
    workspace_root.join("target/debug/libliter_ffi.so")
}

fn resolve_lib_path(raw: PathBuf) -> PathBuf {
    if raw.is_absolute() {
        return raw;
    }

    if raw.exists() {
        return raw;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let candidate = workspace_root.join(&raw);
    if candidate.exists() {
        return candidate;
    }

    raw
}

fn testrunner_path(sqlite_tcl_dir: &Path) -> PathBuf {
    // The sqlite source tree typically contains `test/testrunner.tcl`.
    sqlite_tcl_dir.join("test").join("testrunner.tcl")
}

fn parse_tests_from_env() -> Vec<String> {
    let raw = env::var("LITER_TCL_TESTS").unwrap_or_else(|_| "main.test".to_string());
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn extract_quoted_values(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_quote = false;
    let mut buf = String::new();
    for ch in input.chars() {
        if ch == '"' {
            if in_quote {
                if !buf.is_empty() {
                    out.push(buf.clone());
                    buf.clear();
                }
                in_quote = false;
            } else {
                in_quote = true;
            }
            continue;
        }
        if in_quote {
            buf.push(ch);
        }
    }
    out
}

fn parse_tests_from_manifest_file(
    manifest_path: &Path,
    scope: &str,
) -> Result<Vec<String>, String> {
    let content = fs::read_to_string(manifest_path)
        .map_err(|e| format!("failed reading manifest {}: {e}", manifest_path.display()))?;

    let section_header = format!("[{scope}]");
    let mut in_target_section = false;
    let mut collecting_include = false;
    let mut include_buffer = String::new();

    for raw_line in content.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            if in_target_section && collecting_include {
                let tests = extract_quoted_values(&include_buffer);
                if !tests.is_empty() {
                    return Ok(tests);
                }
            }
            in_target_section = line == section_header;
            collecting_include = false;
            include_buffer.clear();
            continue;
        }

        if !in_target_section {
            continue;
        }

        if !collecting_include {
            if let Some(rest) = line.strip_prefix("include") {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    include_buffer.push_str(rest.trim());
                    if include_buffer.contains(']') {
                        let tests = extract_quoted_values(&include_buffer);
                        if !tests.is_empty() {
                            return Ok(tests);
                        }
                    } else {
                        collecting_include = true;
                    }
                }
            }
        } else {
            include_buffer.push(' ');
            include_buffer.push_str(line);
            if line.contains(']') {
                let tests = extract_quoted_values(&include_buffer);
                if !tests.is_empty() {
                    return Ok(tests);
                }
                collecting_include = false;
                include_buffer.clear();
            }
        }
    }

    if in_target_section && !include_buffer.is_empty() {
        let tests = extract_quoted_values(&include_buffer);
        if !tests.is_empty() {
            return Ok(tests);
        }
    }

    Err(format!(
        "no include list found for section {section_header} in {}",
        manifest_path.display()
    ))
}

fn parse_tests_from_manifest() -> Result<Vec<String>, String> {
    let manifest_path = env::var("LITER_TCL_MANIFEST")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("tests/tcl_suite/conformance-manifest.toml"));
    let scope = env::var("LITER_TCL_SCOPE").unwrap_or_else(|_| "smoke".to_string());
    parse_tests_from_manifest_file(&manifest_path, &scope)
}

fn resolve_tests() -> Result<Vec<String>, String> {
    if env::var("LITER_TCL_TESTS").is_ok() {
        let tests = parse_tests_from_env();
        if tests.is_empty() {
            return Err("LITER_TCL_TESTS was set but produced an empty test list".to_string());
        }
        return Ok(tests);
    }

    match parse_tests_from_manifest() {
        Ok(tests) if !tests.is_empty() => Ok(tests),
        Ok(_) => Err("manifest resolved an empty test list".to_string()),
        Err(_) => Ok(vec!["main.test".to_string()]),
    }
}

pub fn run_tcl_suite_smoke() -> Result<TclRunSummary, String> {
    let sqlite_tcl_dir = read_required_env_path("SQLITE_TCL_DIR")?;
    let tcl_shell = resolve_tcl_shell(&sqlite_tcl_dir)?;
    let testrunner = testrunner_path(&sqlite_tcl_dir);
    if !testrunner.exists() {
        return Err(format!("testrunner not found at {}", testrunner.display()));
    }

    let sqlite_library = env::var("LITER_SQLITE_LIB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_lib_path());
    let sqlite_library = resolve_lib_path(sqlite_library);
    if !sqlite_library.exists() {
        return Err(format!(
            "sqlite dynamic library not found at {} (set LITER_SQLITE_LIB)",
            sqlite_library.display()
        ));
    }

    let tests = resolve_tests()?;
    let mut cmd = Command::new(&tcl_shell);
    cmd.arg(&testrunner);
    for test in &tests {
        cmd.arg(test);
    }

    // testrunner.tcl expects to run from sqlite source root for relative paths.
    let output = cmd
        .current_dir(&sqlite_tcl_dir)
        .env("SQLITE_LIBRARY", &sqlite_library)
        .output()
        .map_err(|e| format!("failed to run tcl suite with {}: {e}", tcl_shell.display()))?;

    let summary = TclRunSummary {
        success: output.status.success(),
        exit_code: output.status.code().unwrap_or(-1),
        test_count: tests.len(),
        tests,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    };

    // Machine-readable one-line summary for CI log parsing.
    println!(
        "TCL_SUMMARY success={} exit_code={} test_count={} tests={}",
        summary.success,
        summary.exit_code,
        summary.test_count,
        summary.tests.join(",")
    );

    if let Ok(path) = env::var("LITER_TCL_SUMMARY_FILE") {
        let mut body = String::new();
        body.push_str(&format!("success={}\n", summary.success));
        body.push_str(&format!("exit_code={}\n", summary.exit_code));
        body.push_str(&format!("test_count={}\n", summary.test_count));
        body.push_str(&format!("tests={}\n", summary.tests.join(",")));
        if !summary.stdout.is_empty() {
            body.push_str("stdout_begin\n");
            body.push_str(&summary.stdout);
            if !summary.stdout.ends_with('\n') {
                body.push('\n');
            }
            body.push_str("stdout_end\n");
        }
        if !summary.stderr.is_empty() {
            body.push_str("stderr_begin\n");
            body.push_str(&summary.stderr);
            if !summary.stderr.ends_with('\n') {
                body.push('\n');
            }
            body.push_str("stderr_end\n");
        }
        fs::write(&path, body)
            .map_err(|e| format!("failed writing LITER_TCL_SUMMARY_FILE {}: {e}", path))?;
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke-test: verify tclsh is available.
    #[test]
    #[ignore = "requires tclsh and sqlite test files"]
    fn tclsh_available() {
        find_tclsh().expect("tclsh unavailable");
    }

    /// Smoke conformance run for a small deterministic subset.
    ///
    /// Required environment variables:
    /// - `SQLITE_TCL_DIR`: SQLite source root containing `test/testrunner.tcl`
    /// Optional environment variables:
    /// - `LITER_SQLITE_LIB`: path to `libliter_ffi` dynamic library
    /// - `LITER_TCL_TESTS`: comma-separated list of test files (overrides manifest)
    /// - `LITER_TCL_MANIFEST`: conformance manifest path
    /// - `LITER_TCL_SCOPE`: manifest section (`smoke` by default)
    /// - `LITER_TCL_SHELL`: path to Tcl shell/testfixture binary
    #[test]
    #[ignore = "requires tclsh, sqlite test corpus, and built liter-ffi cdylib"]
    fn tcl_conformance_smoke() {
        let summary = run_tcl_suite_smoke().expect("failed to run tcl smoke suite");
        assert!(
            summary.success,
            "TCL smoke failed (exit={}):\nSTDOUT:\n{}\nSTDERR:\n{}",
            summary.exit_code, summary.stdout, summary.stderr
        );
    }
}
