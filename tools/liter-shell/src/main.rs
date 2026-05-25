//! liter-shell — interactive command-line interface for liter-rs.
//!
//! Mirrors the `sqlite3` shell from the C SQLite distribution. Supports
//! interactive SQL entry, dot-commands, and scripting via stdin.
//!
//! ## Status
//! Scaffold only — prompts for input but does not yet execute queries.

use std::io::{self, BufRead, Write};

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

    println!(
        "Liter-rs v{} — targeting SQLite 3.53.x",
        env!("CARGO_PKG_VERSION")
    );
    println!("Connected to: {}", conn.path());
    println!("Enter SQL statements terminated by ';', or '.quit' to exit.");

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
            match conn.query(&sql, [] as [(); 0]) {
                Ok(rows) => {
                    for row in &rows {
                        let cols: Vec<String> = row.iter().map(|v| format!("{v:?}")).collect();
                        writeln!(out, "{}", cols.join("|")).ok();
                    }
                }
                Err(liter::SqliteError::NotImplemented) => {
                    writeln!(out, "-- query engine not yet implemented --").ok();
                }
                Err(e) => {
                    writeln!(out, "Error: {e}").ok();
                }
            }
        } else {
            print!("   ...> ");
            out.flush().ok();
        }
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
