//! liter-shell — interactive command-line interface for liter-rs.
//!
//! Mirrors the `sqlite3` shell from the C SQLite distribution. Supports
//! interactive SQL entry, dot-commands, and scripting via stdin (piped or
//! interactive).  When stdin is not a TTY the banner and continuation prompts
//! are suppressed so the shell can be driven like the C `sqlite3` binary.

#[cfg(not(test))]
use std::io::IsTerminal;
use std::io::{self, BufRead, Write};

#[cfg(not(test))]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let interactive = io::stdin().is_terminal();
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    if run_cli(&args, stdin.lock(), &mut out, interactive).is_err() {
        std::process::exit(1);
    }
}

pub fn run_cli<R: BufRead, W: Write>(
    args: &[String],
    input: R,
    mut out: W,
    interactive: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = args.get(1).map(|s| s.as_str()).unwrap_or(":memory:");
    let conn = match liter::Connection::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error opening database '{}': {}", db_path, e);
            return Err(Box::new(e));
        }
    };

    if interactive {
        let res = writeln!(
            out,
            "Liter-rs v{} — targeting SQLite 3.53.x",
            env!("CARGO_PKG_VERSION")
        );
        res?;
        writeln!(out, "Connected to: {}", conn.path())?;
        let res = writeln!(
            out,
            "Enter SQL statements terminated by ';', or '.quit' to exit."
        );
        res?;
    }

    run_session(input, &mut out, &conn, interactive)?;
    Ok(())
}

pub fn run_session<R: BufRead, W: Write>(
    mut input: R,
    mut out: W,
    conn: &liter::Connection,
    interactive: bool,
) -> io::Result<()> {
    let mut sql_buf = String::new();
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = input.read_line(&mut line)?;
        if bytes_read == 0 {
            break;
        }

        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case(".quit") || trimmed.eq_ignore_ascii_case(".exit") {
            break;
        }

        if trimmed.starts_with('.') {
            handle_dot_command(trimmed, conn, &mut out)?;
            continue;
        }

        sql_buf.push_str(&line);

        if trimmed.ends_with(';') {
            let sql = sql_buf.trim().to_owned();
            sql_buf.clear();
            exec_sql(&sql, conn, &mut out, interactive)?;
        } else if interactive {
            write!(out, "   ...> ")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Returns true when `sql` is a read-only statement that produces result rows.
fn is_returning_sql(sql: &str) -> bool {
    let first = sql
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        first.as_str(),
        "select" | "explain" | "with" | "values" | "pragma"
    )
}

fn exec_sql(
    sql: &str,
    conn: &liter::Connection,
    out: &mut impl Write,
    interactive: bool,
) -> io::Result<()> {
    // `liter::SqliteError::NotImplemented` is a defined error variant, but
    // nothing in the `liter` crate's public API (query/execute and their
    // error conversions) actually constructs one today — every codegen
    // "not implemented" case surfaces as `SqliteError::Sql(...)` instead.
    // These arms are kept as real, explicit handling (rather than folded
    // into the generic `Err(e)` arm below) so the shell keeps working if
    // that ever changes.
    if is_returning_sql(sql) {
        match conn.query(sql, [] as [(); 0]) {
            Ok(rows) => {
                for row in &rows {
                    let cols: Vec<String> = row.iter().map(format_value).collect();
                    writeln!(out, "{}", cols.join("|"))?;
                }
            }
            Err(liter::SqliteError::NotImplemented) => {
                if interactive {
                    writeln!(out, "-- not yet implemented --")?;
                } else {
                    eprintln!("Error: not yet implemented");
                    return Err(io::Error::other("not yet implemented"));
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
                if !interactive {
                    return Err(io::Error::other(e.to_string()));
                }
            }
        }
    } else {
        match conn.execute(sql, [] as [(); 0]) {
            Ok(_) => {}
            Err(liter::SqliteError::NotImplemented) => {
                if interactive {
                    writeln!(out, "-- not yet implemented --")?;
                } else {
                    eprintln!("Error: not yet implemented");
                    return Err(io::Error::other("not yet implemented"));
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
                if !interactive {
                    return Err(io::Error::other(e.to_string()));
                }
            }
        }
    }
    Ok(())
}

fn format_value(v: &liter::Value) -> String {
    match v {
        liter::Value::Null => String::new(),
        liter::Value::Int(i) => i.to_string(),
        liter::Value::Real(f) => f.to_string(),
        liter::Value::Text(b) => String::from_utf8_lossy(b).into_owned(),
        liter::Value::Blob(b) => format!("{b:?}"),
        liter::Value::ZeroBlob(n) => format!("(zeroblob {n})"),
    }
}

fn handle_dot_command(cmd: &str, _conn: &liter::Connection, mut out: impl Write) -> io::Result<()> {
    match cmd {
        ".help" => {
            writeln!(out, ".help      Show this help")?;
            writeln!(out, ".tables    List tables")?;
            writeln!(out, ".schema    Show schema")?;
            writeln!(out, ".quit      Exit")?;
        }
        ".tables" => {
            writeln!(out, "-- .tables not yet implemented --")?;
        }
        ".schema" => {
            writeln!(out, "-- .schema not yet implemented --")?;
        }
        _ => {
            eprintln!("Unknown dot-command: {cmd}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_returning_sql() {
        assert!(is_returning_sql("SELECT 1;"));
        assert!(is_returning_sql("select * from t;"));
        assert!(is_returning_sql("EXPLAIN SELECT 1;"));
        assert!(is_returning_sql(
            "with cte as (select 1) select * from cte;"
        ));
        assert!(is_returning_sql("VALUES (1, 2);"));
        assert!(is_returning_sql("pragma table_info('t');"));

        assert!(!is_returning_sql("INSERT INTO t VALUES (1);"));
        assert!(!is_returning_sql("UPDATE t SET x = 1;"));
        assert!(!is_returning_sql("DELETE FROM t;"));
        assert!(!is_returning_sql("CREATE TABLE t (x INT);"));
        assert!(!is_returning_sql(""));
    }

    #[test]
    fn test_format_value() {
        assert_eq!(format_value(&liter::Value::Null), "");
        assert_eq!(format_value(&liter::Value::Int(42)), "42");
        assert_eq!(format_value(&liter::Value::Real(3.5)), "3.5");
        assert_eq!(
            format_value(&liter::Value::Text(b"hello".to_vec())),
            "hello"
        );
        assert_eq!(
            format_value(&liter::Value::Blob(vec![1, 2, 3])),
            "[1, 2, 3]"
        );
        assert_eq!(format_value(&liter::Value::ZeroBlob(10)), "(zeroblob 10)");
    }

    #[test]
    fn test_handle_dot_command() {
        let conn = liter::Connection::open(":memory:").unwrap();
        let mut out = Vec::new();

        handle_dot_command(".help", &conn, &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains(".help"));
        assert!(s.contains(".tables"));
        assert!(s.contains(".schema"));
        assert!(s.contains(".quit"));

        let mut out_tables = Vec::new();
        handle_dot_command(".tables", &conn, &mut out_tables).unwrap();
        assert_eq!(
            String::from_utf8(out_tables).unwrap(),
            "-- .tables not yet implemented --\n"
        );

        let mut out_schema = Vec::new();
        handle_dot_command(".schema", &conn, &mut out_schema).unwrap();
        assert_eq!(
            String::from_utf8(out_schema).unwrap(),
            "-- .schema not yet implemented --\n"
        );

        let mut out_unknown = Vec::new();
        handle_dot_command(".foobar", &conn, &mut out_unknown).unwrap();
        assert!(out_unknown.is_empty());
    }

    #[test]
    fn test_exec_sql_and_run_session() {
        let conn = liter::Connection::open(":memory:").unwrap();

        let input = b"CREATE TABLE users (id INT, name TEXT);\nINSERT INTO users VALUES (1, 'Alice');\nSELECT * FROM users;\n.exit\n";
        let mut out = Vec::new();

        run_session(&input[..], &mut out, &conn, false).unwrap();
        let output_str = String::from_utf8(out).unwrap();
        assert!(output_str.contains("1|Alice"));
    }

    #[test]
    fn test_run_session_interactive_continuation_and_dot_commands() {
        let conn = liter::Connection::open(":memory:").unwrap();

        let input = b"SELECT\n1\n;\n.help\n.tables\n.schema\n.unknown\n.quit\n";
        let mut out = Vec::new();

        run_session(&input[..], &mut out, &conn, true).unwrap();
        let output_str = String::from_utf8(out).unwrap();
        assert!(output_str.contains("   ...> "));
        assert!(output_str.contains("1"));
        assert!(output_str.contains(".help"));
    }

    #[test]
    fn test_exec_sql_error_paths() {
        let conn = liter::Connection::open(":memory:").unwrap();
        let mut out = Vec::new();

        // Query error in interactive mode returns Ok(())
        assert!(exec_sql("SELECT * FROM non_existent_table;", &conn, &mut out, true).is_ok());

        // Execute error in interactive mode returns Ok(())
        assert!(exec_sql(
            "INSERT INTO non_existent_table VALUES (1);",
            &conn,
            &mut out,
            true,
        )
        .is_ok());

        // Non-interactive mode returns Err
        assert!(exec_sql("SELECT * FROM non_existent_table;", &conn, &mut out, false).is_err());
        assert!(exec_sql(
            "INSERT INTO non_existent_table VALUES (1);",
            &conn,
            &mut out,
            false,
        )
        .is_err());
    }

    #[test]
    fn test_run_session_non_interactive_error() {
        let conn = liter::Connection::open(":memory:").unwrap();
        let mut out = Vec::new();
        let input = b"SELECT * FROM nonexistent;\n";
        assert!(run_session(&input[..], &mut out, &conn, false).is_err());
    }

    #[test]
    fn test_run_session_non_interactive_unterminated_line_no_prompt() {
        // Non-interactive: an unterminated line takes neither the
        // "statement complete" branch nor the "print continuation prompt"
        // branch (that one only fires when `interactive`).
        let conn = liter::Connection::open(":memory:").unwrap();
        let mut out = Vec::new();
        let input = b"SELECT\n1\n;\n";
        assert!(run_session(&input[..], &mut out, &conn, false).is_ok());
        let output = String::from_utf8(out).unwrap();
        assert!(!output.contains("...>"));
    }

    #[test]
    fn test_run_cli_interactive_and_non_interactive() {
        let mut out = Vec::new();
        let args = vec!["liter-shell".to_string(), ":memory:".to_string()];
        let input = b"SELECT 42;\n";
        assert!(run_cli(&args, &input[..], &mut out, true).is_ok());
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("Liter-rs"));
        assert!(output.contains("Connected to: :memory:"));
        assert!(output.contains("42"));

        let mut out_non_interactive = Vec::new();
        let args_default = vec!["liter-shell".to_string()];
        let input_empty = b"";
        assert!(run_cli(
            &args_default,
            &input_empty[..],
            &mut out_non_interactive,
            false
        )
        .is_ok());
    }

    #[test]
    fn test_run_session_eof() {
        let conn = liter::Connection::open(":memory:").unwrap();
        let mut out = Vec::new();
        let input = b"";
        assert!(run_session(&input[..], &mut out, &conn, false).is_ok());
    }

    #[test]
    fn test_run_cli_open_err() {
        let mut out = Vec::new();
        let args = vec![
            "liter-shell".to_string(),
            "/nonexistent_directory/path/that/cannot/exist/db.sqlite".to_string(),
        ];
        let input = b"";
        assert!(run_cli(&args, &input[..], &mut out, false).is_err());
    }
}
