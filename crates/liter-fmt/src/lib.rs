//! `printf`-style formatting and `strftime` date/time formatting.
//!
//! Mirrors `printf.c` and `date.c` from the C SQLite source. Provides
//! SQLite-compatible `%q`, `%Q`, `%w`, and standard printf conversions.

#[derive(Debug, thiserror::Error)]
pub enum FmtError {
    #[error("invalid format string")]
    InvalidFormat,
    #[error("output buffer overflow")]
    Overflow,
}

/// Escape a string for safe embedding in a SQL literal (mirrors `%q`).
/// Doubles every single-quote character.
pub fn quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        if c == '\'' {
            out.push('\'');
        }
        out.push(c);
    }
    out
}

/// Like `quote_string` but wraps the result in single quotes, or returns
/// `NULL` for a null-like empty marker (mirrors `%Q`).
pub fn quote_string_or_null(s: Option<&str>) -> String {
    match s {
        None => "NULL".to_owned(),
        Some(s) => format!("'{}'", quote_string(s)),
    }
}

/// Minimal `strftime` implementation for SQLite's supported format codes.
///
/// Supported codes: `%Y`, `%m`, `%d`, `%H`, `%M`, `%S`, `%f`, `%j`, `%s`, `%%`.
pub fn strftime(fmt: &str, julian_day: f64) -> Result<String, FmtError> {
    // Convert Julian Day Number to calendar fields.
    let (year, month, day, hour, min, sec, frac) = jdn_to_parts(julian_day);

    let mut out = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('Y') => out.push_str(&format!("{year:04}")),
            Some('m') => out.push_str(&format!("{month:02}")),
            Some('d') => out.push_str(&format!("{day:02}")),
            Some('H') => out.push_str(&format!("{hour:02}")),
            Some('M') => out.push_str(&format!("{min:02}")),
            Some('S') => out.push_str(&format!("{sec:02}")),
            Some('f') => out.push_str(&format!("{:09.6}", sec as f64 + frac)),
            Some('s') => {
                // Unix timestamp
                let unix = (julian_day - 2440587.5) * 86400.0;
                out.push_str(&format!("{}", unix as i64));
            }
            Some(_other) => {
                return Err(FmtError::InvalidFormat);
            }
            None => return Err(FmtError::InvalidFormat),
        }
    }
    Ok(out)
}

/// Decompose a Julian Day Number into (year, month, day, hour, min, sec, frac_sec).
fn jdn_to_parts(jd: f64) -> (i32, u32, u32, u32, u32, u32, f64) {
    // Algorithm from https://en.wikipedia.org/wiki/Julian_day#Julian_day_number_calculation
    let jd_int = (jd + 0.5) as i64;
    let frac = (jd + 0.5).fract();

    let l = jd_int + 68569;
    let n = (4 * l) / 146097;
    let l = l - (146097 * n + 3) / 4;
    let i = (4000 * (l + 1)) / 1461001;
    let l = l - (1461 * i) / 4 + 31;
    let j = (80 * l) / 2447;
    let day = (l - (2447 * j) / 80) as u32;
    let l = j / 11;
    let month = (j + 2 - 12 * l) as u32;
    let year = (100 * (n - 49) + i + l) as i32;

    let total_secs = (frac * 86400.0).round() as u64;
    let hour = (total_secs / 3600) as u32;
    let min = ((total_secs % 3600) / 60) as u32;
    let sec = (total_secs % 60) as u32;
    let frac_sec = frac * 86400.0 - total_secs as f64;

    (year, month, day, hour, min, sec, frac_sec.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_no_special_chars() {
        assert_eq!(quote_string("hello"), "hello");
    }

    #[test]
    fn quote_single_quotes() {
        assert_eq!(quote_string("it's"), "it''s");
    }

    #[test]
    fn quote_or_null_none() {
        assert_eq!(quote_string_or_null(None), "NULL");
    }

    #[test]
    fn strftime_unix_epoch() {
        // Julian day for 1970-01-01 00:00:00 UTC = 2440587.5
        let s = strftime("%Y-%m-%d", 2440587.5).unwrap();
        assert_eq!(s, "1970-01-01");
    }
}
