//! liter-shell — interactive command-line interface for liter-rs.
//!
//! Mirrors the `sqlite3` shell from the C SQLite distribution. Supports
//! interactive SQL entry, dot-commands, and scripting via stdin (piped or
//! interactive).  When stdin is not a TTY the banner and continuation prompts
//! are suppressed so the shell can be driven like the C `sqlite3` binary.

use std::io::{self, BufRead, IsTerminal, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let db_path = args.get(1).map(|s| s.as_str()).unwrap_or(":memory:");
    let conn = match liter::Connection::open(db_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error opening database '{}': {}", db_path, e);
            std::process::exit(1);
        }
    };

    let interactive = io::stdin().is_terminal();

    if interactive {
        println!(
            "Liter-rs v{} — targeting SQLite 3.53.x",
            env!("CARGO_PKG_VERSION")
        );
        println!("Connected to: {}", conn.path());
        println!("Enter SQL statements terminated by ';', or '.quit' to exit.");
    }

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    let mut sql_buf = String::new();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Input error: {e}");
                break;
            }
        };

        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case(".quit") || trimmed.eq_ignore_ascii_case(".exit") {
            break;
        }

        if trimmed.starts_with('.') {
            handle_dot_command(trimmed, &conn);
            continue;
        }

        sql_buf.push_str(&line);
        sql_buf.push('\n');

        if trimmed.ends_with(';') {
            let sql = sql_buf.trim().to_owned();
            sql_buf.clear();
            exec_sql(&sql, &conn, &mut out, interactive);
        } else if interactive {
            print!("   ...> ");
            out.flush().ok();
        }
    }
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

fn exec_sql(sql: &str, conn: &liter::Connection, out: &mut impl Write, interactive: bool) {
    if is_returning_sql(sql) {
        match conn.query(sql, [] as [(); 0]) {
            Ok(rows) => {
                for row in &rows {
                    let cols: Vec<String> = row.iter().map(format_value).collect();
                    writeln!(out, "{}", cols.join("|")).ok();
                }
            }
            Err(liter::SqliteError::NotImplemented) => {
                if interactive {
                    writeln!(out, "-- not yet implemented --").ok();
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
            }
        }
    } else {
        match conn.execute(sql, [] as [(); 0]) {
            Ok(_) => {}
            Err(liter::SqliteError::NotImplemented) => {
                if interactive {
                    writeln!(out, "-- not yet implemented --").ok();
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
            }
        }
    }
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

fn handle_dot_command(cmd: &str, _conn: &liter::Connection) {
    match cmd {
        ".help" => {
            println!(".help      Show this help");
            println!(".tables    List tables");
            println!(".schema    Show schema");
            println!(".quit      Exit");
        }
        ".tables" => println!("-- .tables not yet implemented --"),
        ".schema" => println!("-- .schema not yet implemented --"),
        _ => eprintln!("Unknown dot-command: {cmd}"),
    }
}
