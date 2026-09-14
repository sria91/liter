//! Built-in SQL functions for Liter-rs.
//!
//! Mirrors `func.c` and `math.c`. Provides scalar and aggregate functions
//! callable from SQL expressions in the VDBE.
//!
//! ## Status
//! Phase 4 — scalar function stubs; aggregate stubs.

use liter_vdbe::Mem;

/// Error from a built-in function call.
#[derive(Debug, thiserror::Error)]
pub enum FuncError {
    #[error("wrong number of arguments to function {0}()")]
    WrongArgCount(String),
    #[error("not yet implemented: {0}()")]
    NotImplemented(String),
}

pub type FuncResult<T> = Result<T, FuncError>;

// ── Aggregate functions ───────────────────────────────────────────────────────

pub fn dispatch_aggregate(name: &str) -> Result<Box<dyn liter_vdbe::AggregateState>, String> {
    match name.to_ascii_lowercase().as_str() {
        "count" => Ok(Box::new(CountState { count: 0 })),
        "sum" => Ok(Box::new(SumState { sum: None })),
        "avg" => Ok(Box::new(AvgState { sum: 0.0, count: 0 })),
        "min" => Ok(Box::new(MinState { min: None })),
        "max" => Ok(Box::new(MaxState { max: None })),
        _ => Err(format!("Not implemented: aggregate {}()", name)),
    }
}

#[derive(Debug)]
struct CountState {
    count: i64,
}
impl liter_vdbe::AggregateState for CountState {
    fn step(&mut self, args: &[Mem]) -> Result<(), String> {
        if args.is_empty() || !args[0].is_null() {
            self.count += 1;
        }
        Ok(())
    }
    fn finalize(&mut self) -> Result<Mem, String> {
        Ok(Mem::Int(self.count))
    }
}

#[derive(Debug)]
struct SumState {
    sum: Option<Mem>,
}
impl liter_vdbe::AggregateState for SumState {
    fn step(&mut self, args: &[Mem]) -> Result<(), String> {
        if let Some(val) = args.first() {
            if val.is_null() {
                return Ok(());
            }
            match &self.sum {
                None => {
                    self.sum = match val {
                        Mem::Int(i) => Some(Mem::Int(*i)),
                        Mem::Real(f) => Some(Mem::Real(*f)),
                        v => Some(Mem::Real(v.to_real().unwrap_or(0.0))),
                    };
                }
                Some(Mem::Int(acc)) => match val {
                    Mem::Int(i) => {
                        if let Some(new_acc) = acc.checked_add(*i) {
                            self.sum = Some(Mem::Int(new_acc));
                        } else {
                            self.sum = Some(Mem::Real(*acc as f64 + *i as f64));
                        }
                    }
                    Mem::Real(f) => {
                        self.sum = Some(Mem::Real(*acc as f64 + f));
                    }
                    v => {
                        let f = v.to_real().unwrap_or(0.0);
                        self.sum = Some(Mem::Real(*acc as f64 + f));
                    }
                },
                Some(Mem::Real(acc)) => {
                    self.sum = Some(Mem::Real(*acc + val.to_real().unwrap_or(0.0)));
                }
                // Accumulator is only ever None / Int / Real (see above), so
                // this arm is unreachable in practice — but required for
                // exhaustiveness.
                Some(_) => {}
            }
        }
        Ok(())
    }
    fn finalize(&mut self) -> Result<Mem, String> {
        Ok(self.sum.take().unwrap_or(Mem::Null))
    }
}

#[derive(Debug)]
struct AvgState {
    sum: f64,
    count: i64,
}
impl liter_vdbe::AggregateState for AvgState {
    fn step(&mut self, args: &[Mem]) -> Result<(), String> {
        if let Some(val) = args.first() {
            if let Some(num) = val.to_real() {
                self.sum += num;
                self.count += 1;
            }
        }
        Ok(())
    }
    fn finalize(&mut self) -> Result<Mem, String> {
        if self.count == 0 {
            Ok(Mem::Null)
        } else {
            Ok(Mem::Real(self.sum / self.count as f64))
        }
    }
}

#[derive(Debug)]
struct MinState {
    min: Option<Mem>,
}
impl liter_vdbe::AggregateState for MinState {
    fn step(&mut self, args: &[Mem]) -> Result<(), String> {
        if let Some(val) = args.first() {
            if !val.is_null() {
                if let Some(m) = &self.min {
                    if val.cmp(m) == std::cmp::Ordering::Less {
                        self.min = Some(val.clone());
                    }
                } else {
                    self.min = Some(val.clone());
                }
            }
        }
        Ok(())
    }
    fn finalize(&mut self) -> Result<Mem, String> {
        Ok(self.min.take().unwrap_or(Mem::Null))
    }
}

#[derive(Debug)]
struct MaxState {
    max: Option<Mem>,
}
impl liter_vdbe::AggregateState for MaxState {
    fn step(&mut self, args: &[Mem]) -> Result<(), String> {
        if let Some(val) = args.first() {
            if !val.is_null() {
                if let Some(m) = &self.max {
                    if val.cmp(m) == std::cmp::Ordering::Greater {
                        self.max = Some(val.clone());
                    }
                } else {
                    self.max = Some(val.clone());
                }
            }
        }
        Ok(())
    }
    fn finalize(&mut self) -> Result<Mem, String> {
        Ok(self.max.take().unwrap_or(Mem::Null))
    }
}

// ── Scalar functions ──────────────────────────────────────────────────────────

/// Dynamically dispatch a scalar function call by name.
pub fn dispatch_function(name: &str, args: &[Mem]) -> FuncResult<Mem> {
    match name.to_ascii_lowercase().as_str() {
        "abs" => func_abs(args),
        "length" => func_length(args),
        "typeof" => func_typeof(args),
        "upper" => func_upper(args),
        "lower" => func_lower(args),
        "coalesce" => func_coalesce(args),
        "ifnull" => func_ifnull(args),
        "max" => func_max_scalar(args),
        "min" => func_min_scalar(args),
        "round" => func_round(args),
        "sign" => func_sign(args),
        "substr" | "substring" => func_substr(args),
        "instr" => func_instr(args),
        "replace" => func_replace(args),
        "trim" => func_trim(args),
        "zeroblob" => func_zeroblob(args),
        "date" => func_date(args),
        "time" => func_time(args),
        "datetime" => func_datetime(args),
        "julianday" => func_julianday(args),
        "strftime" => func_strftime(args),
        _ => Err(FuncError::NotImplemented(name.to_string())),
    }
}

pub fn func_abs(args: &[Mem]) -> FuncResult<Mem> {
    match args.first() {
        Some(Mem::Int(i)) => Ok(Mem::Int(i.checked_abs().unwrap_or(i64::MAX))),
        Some(Mem::Real(f)) => Ok(Mem::Real(f.abs())),
        Some(Mem::Null) | None => Ok(Mem::Null),
        _ => Ok(Mem::Null),
    }
}

pub fn func_length(args: &[Mem]) -> FuncResult<Mem> {
    match args.first() {
        Some(Mem::Text(s)) => Ok(Mem::Int(s.chars().count() as i64)),
        Some(Mem::Blob(b)) => Ok(Mem::Int(b.len() as i64)),
        Some(Mem::ZeroBlob(n)) => Ok(Mem::Int(*n as i64)),
        Some(Mem::Null) | None => Ok(Mem::Null),
        _ => Ok(Mem::Null),
    }
}

pub fn func_zeroblob(args: &[Mem]) -> FuncResult<Mem> {
    if args.len() != 1 {
        return Err(FuncError::WrongArgCount("zeroblob".into()));
    }
    match args.first() {
        Some(Mem::Int(n)) if *n >= 0 => {
            println!("zeroblob called with Int({})", n);
            Ok(Mem::ZeroBlob(*n as i64))
        }
        Some(Mem::Real(f)) if *f >= 0.0 => {
            println!("zeroblob called with Real({})", f);
            Ok(Mem::ZeroBlob(*f as i64))
        }
        Some(Mem::Null) => {
            println!("zeroblob called with Null");
            Ok(Mem::Null)
        }
        _ => {
            println!("zeroblob called with unsupported arg");
            Ok(Mem::Null)
        }
    }
}

pub fn func_typeof(args: &[Mem]) -> FuncResult<Mem> {
    let t = match args.first() {
        Some(Mem::Null) => "null",
        Some(Mem::Int(_)) => "integer",
        Some(Mem::Real(_)) => "real",
        Some(Mem::Text(_)) => "text",
        Some(Mem::Blob(_)) | Some(Mem::ZeroBlob(_)) => "blob",
        None => "null",
        Some(Mem::Agg(_)) => "blob", // Treat aggregators as blobs from SQL perspective
    };
    Ok(Mem::Text(std::sync::Arc::from(t)))
}

pub fn func_upper(args: &[Mem]) -> FuncResult<Mem> {
    match args.first() {
        Some(Mem::Text(s)) => Ok(Mem::Text(std::sync::Arc::from(s.to_uppercase().as_str()))),
        Some(Mem::Null) | None => Ok(Mem::Null),
        _ => Ok(Mem::Null),
    }
}

pub fn func_lower(args: &[Mem]) -> FuncResult<Mem> {
    match args.first() {
        Some(Mem::Text(s)) => Ok(Mem::Text(std::sync::Arc::from(s.to_lowercase().as_str()))),
        Some(Mem::Null) | None => Ok(Mem::Null),
        _ => Ok(Mem::Null),
    }
}

pub fn func_coalesce(args: &[Mem]) -> FuncResult<Mem> {
    for a in args {
        if !matches!(a, Mem::Null) {
            return Ok(a.clone());
        }
    }
    Ok(Mem::Null)
}

pub fn func_ifnull(args: &[Mem]) -> FuncResult<Mem> {
    if args.len() != 2 {
        return Err(FuncError::WrongArgCount("ifnull".into()));
    }
    func_coalesce(args)
}

fn mem_to_string(m: &Mem) -> String {
    match m {
        Mem::Text(t) => t.to_string(),
        Mem::Int(i) => i.to_string(),
        Mem::Real(f) => f.to_string(),
        _ => String::new(),
    }
}

pub fn func_substr(args: &[Mem]) -> FuncResult<Mem> {
    if args.len() < 2 || args.len() > 3 {
        return Err(FuncError::WrongArgCount("substr".into()));
    }
    let s = match args.first() {
        Some(Mem::Text(t)) => t.to_string(),
        Some(Mem::Null) | None => return Ok(Mem::Null),
        Some(m) => mem_to_string(m),
    };

    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;

    let start = match &args[1] {
        Mem::Int(i) => *i,
        Mem::Real(f) => *f as i64,
        Mem::Text(t) => t.parse().unwrap_or(0),
        _ => return Ok(Mem::Null),
    };

    let start_idx = if start > 0 {
        start - 1
    } else if start < 0 {
        (len + start).max(0)
    } else {
        0
    };

    let length = if args.len() == 3 {
        match &args[2] {
            Mem::Int(i) => Some(*i),
            Mem::Real(f) => Some(*f as i64),
            Mem::Text(t) => Some(t.parse().unwrap_or(0)),
            _ => None,
        }
    } else {
        None
    };

    if let Some(l) = length {
        if l < 0 {
            let l_abs = l.abs();
            let new_start = (start_idx - l_abs).max(0);
            let end_idx = start_idx.min(len).max(0);
            let sub = chars[new_start as usize..end_idx as usize]
                .iter()
                .collect::<String>();
            return Ok(Mem::Text(std::sync::Arc::from(sub.as_str())));
        }
    }

    let actual_len = length.unwrap_or(len - start_idx);
    let end_idx = (start_idx + actual_len).min(len).max(0);
    let start_idx = start_idx.min(len).max(0);

    let sub = chars[start_idx as usize..end_idx as usize]
        .iter()
        .collect::<String>();
    Ok(Mem::Text(std::sync::Arc::from(sub.as_str())))
}

pub fn func_instr(args: &[Mem]) -> FuncResult<Mem> {
    if args.len() != 2 {
        return Err(FuncError::WrongArgCount("instr".into()));
    }
    let haystack = match &args[0] {
        Mem::Text(t) => t.to_string(),
        Mem::Null => return Ok(Mem::Null),
        m => mem_to_string(m),
    };
    let needle = match &args[1] {
        Mem::Text(t) => t.to_string(),
        Mem::Null => return Ok(Mem::Null),
        m => mem_to_string(m),
    };

    if let Some(pos) = haystack.find(&needle) {
        let char_pos = haystack[..pos].chars().count() + 1;
        Ok(Mem::Int(char_pos as i64))
    } else {
        Ok(Mem::Int(0))
    }
}

pub fn func_replace(args: &[Mem]) -> FuncResult<Mem> {
    if args.len() != 3 {
        return Err(FuncError::WrongArgCount("replace".into()));
    }
    let haystack = match &args[0] {
        Mem::Text(t) => t.to_string(),
        Mem::Null => return Ok(Mem::Null),
        m => mem_to_string(m),
    };
    let pattern = match &args[1] {
        Mem::Text(t) => t.to_string(),
        Mem::Null => return Ok(Mem::Null),
        m => mem_to_string(m),
    };
    let replacement = match &args[2] {
        Mem::Text(t) => t.to_string(),
        Mem::Null => return Ok(Mem::Null),
        m => mem_to_string(m),
    };

    let result = haystack.replace(&pattern, &replacement);
    Ok(Mem::Text(std::sync::Arc::from(result.as_str())))
}

pub fn func_trim(args: &[Mem]) -> FuncResult<Mem> {
    if args.is_empty() || args.len() > 2 {
        return Err(FuncError::WrongArgCount("trim".into()));
    }
    let s = match &args[0] {
        Mem::Text(t) => t.to_string(),
        Mem::Null => return Ok(Mem::Null),
        m => mem_to_string(m),
    };

    let chars_to_trim = if args.len() == 2 {
        match &args[1] {
            Mem::Text(t) => t.to_string(),
            _ => " ".to_string(),
        }
    } else {
        " ".to_string()
    };

    let trimmed = s.trim_matches(|c| chars_to_trim.contains(c));
    Ok(Mem::Text(std::sync::Arc::from(trimmed)))
}

pub fn func_date(args: &[Mem]) -> FuncResult<Mem> {
    func_strftime_impl("%Y-%m-%d", args)
}

pub fn func_time(args: &[Mem]) -> FuncResult<Mem> {
    func_strftime_impl("%H:%M:%S", args)
}

pub fn func_datetime(args: &[Mem]) -> FuncResult<Mem> {
    func_strftime_impl("%Y-%m-%d %H:%M:%S", args)
}

pub fn func_julianday(args: &[Mem]) -> FuncResult<Mem> {
    let dt = match parse_datetime(args) {
        Some(dt) => dt,
        None => return Ok(Mem::Null),
    };
    let jd = (dt.timestamp_millis() as f64) / 86400000.0 + 2440587.5;
    Ok(Mem::Real(jd))
}

pub fn func_strftime(args: &[Mem]) -> FuncResult<Mem> {
    if args.is_empty() {
        return Err(FuncError::WrongArgCount("strftime".into()));
    }
    let format = match args.first() {
        Some(Mem::Text(t)) => t.to_string(),
        _ => return Ok(Mem::Null),
    };
    func_strftime_impl(&format, &args[1..])
}

fn parse_datetime(args: &[Mem]) -> Option<chrono::DateTime<chrono::Utc>> {
    if args.is_empty() {
        return None;
    }
    let time_val = match &args[0] {
        Mem::Text(t) => t.to_string(),
        Mem::Int(i) => return chrono::DateTime::from_timestamp(*i, 0),
        Mem::Real(f) => {
            let millis = ((*f - 2440587.5) * 86400000.0) as i64;
            return chrono::DateTime::from_timestamp_millis(millis);
        }
        Mem::Null => return None,
        _ => return None,
    };

    if time_val.eq_ignore_ascii_case("now") {
        Some(chrono::Utc::now())
    } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&time_val, "%Y-%m-%d %H:%M:%S") {
        Some(dt.and_utc())
    } else if let Ok(d) = chrono::NaiveDate::parse_from_str(&time_val, "%Y-%m-%d") {
        Some(d.and_hms_opt(0, 0, 0).unwrap_or_default().and_utc())
    } else {
        None
    }
}

fn func_strftime_impl(format: &str, args: &[Mem]) -> FuncResult<Mem> {
    let dt = match parse_datetime(args) {
        Some(dt) => dt,
        None => return Ok(Mem::Null),
    };
    let formatted = dt.format(format).to_string();
    Ok(Mem::Text(std::sync::Arc::from(formatted.as_str())))
}

fn compare_mem(a: &Mem, b: &Mem) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Mem::Null, Mem::Null) => Ordering::Equal,
        (Mem::Null, _) => Ordering::Less,
        (_, Mem::Null) => Ordering::Greater,
        (Mem::Int(i1), Mem::Int(i2)) => i1.cmp(i2),
        (Mem::Real(f1), Mem::Real(f2)) => f1.partial_cmp(f2).unwrap_or(Ordering::Equal),
        (Mem::Int(i), Mem::Real(f)) => (*i as f64).partial_cmp(f).unwrap_or(Ordering::Equal),
        (Mem::Real(f), Mem::Int(i)) => f.partial_cmp(&(*i as f64)).unwrap_or(Ordering::Equal),
        (Mem::Text(s1), Mem::Text(s2)) => s1.cmp(s2),
        (Mem::Int(_) | Mem::Real(_), Mem::Text(_)) => Ordering::Less,
        (Mem::Text(_), Mem::Int(_) | Mem::Real(_)) => Ordering::Greater,
        _ => Ordering::Equal, // simplified fallback
    }
}

pub fn func_max_scalar(args: &[Mem]) -> FuncResult<Mem> {
    if args.is_empty() {
        return Ok(Mem::Null);
    }
    let mut max_val = &args[0];
    for val in args.iter().skip(1) {
        if compare_mem(val, max_val) == std::cmp::Ordering::Greater {
            max_val = val;
        }
    }
    Ok(max_val.clone())
}

pub fn func_min_scalar(args: &[Mem]) -> FuncResult<Mem> {
    if args.is_empty() {
        return Ok(Mem::Null);
    }
    let mut min_val = &args[0];
    for val in args.iter().skip(1) {
        // In min(), NULL is less than everything, but typical SQL min ignores nulls?
        // Actually SQLite scalar min() treats NULL as smaller than everything else.
        if compare_mem(val, min_val) == std::cmp::Ordering::Less {
            min_val = val;
        }
    }
    Ok(min_val.clone())
}

pub fn func_round(args: &[Mem]) -> FuncResult<Mem> {
    if args.is_empty() {
        return Ok(Mem::Null);
    }
    let val = match &args[0] {
        Mem::Null => return Ok(Mem::Null),
        Mem::Int(i) => *i as f64,
        Mem::Real(f) => *f,
        Mem::Text(s) => s.parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    };

    let digits = if args.len() > 1 {
        match &args[1] {
            Mem::Int(i) => *i,
            Mem::Real(f) => *f as i64,
            _ => 0,
        }
    } else {
        0
    };

    if digits == 0 {
        Ok(Mem::Real(val.round()))
    } else {
        let multiplier = 10.0_f64.powi(digits as i32);
        Ok(Mem::Real((val * multiplier).round() / multiplier))
    }
}

pub fn func_sign(args: &[Mem]) -> FuncResult<Mem> {
    match args.first() {
        Some(Mem::Int(i)) => Ok(Mem::Int(i.signum())),
        Some(Mem::Real(f)) => Ok(Mem::Int(if *f > 0.0 {
            1
        } else if *f < 0.0 {
            -1
        } else {
            0
        })),
        Some(Mem::Null) | None => Ok(Mem::Null),
        _ => Ok(Mem::Int(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abs_int() {
        assert_eq!(func_abs(&[Mem::Int(-5)]).unwrap(), Mem::Int(5));
    }

    #[test]
    fn abs_null() {
        assert_eq!(func_abs(&[Mem::Null]).unwrap(), Mem::Null);
    }

    #[test]
    fn length_text() {
        let m = Mem::Text(std::sync::Arc::from("hello"));
        assert_eq!(func_length(&[m]).unwrap(), Mem::Int(5));
    }

    #[test]
    fn typeof_values() {
        assert_eq!(
            func_typeof(&[Mem::Int(1)]).unwrap(),
            Mem::Text(std::sync::Arc::from("integer"))
        );
        assert_eq!(
            func_typeof(&[Mem::Null]).unwrap(),
            Mem::Text(std::sync::Arc::from("null"))
        );
    }

    #[test]
    fn coalesce_first_non_null() {
        let v = func_coalesce(&[Mem::Null, Mem::Int(42), Mem::Int(99)]).unwrap();
        assert_eq!(v, Mem::Int(42));
    }

    #[test]
    fn test_max_scalar() {
        let args = [Mem::Int(10), Mem::Int(42), Mem::Int(-5)];
        assert_eq!(func_max_scalar(&args).unwrap(), Mem::Int(42));

        let mixed = [Mem::Int(10), Mem::Real(15.5)];
        assert_eq!(func_max_scalar(&mixed).unwrap(), Mem::Real(15.5));
    }

    #[test]
    fn test_min_scalar() {
        let args = [Mem::Int(10), Mem::Int(42), Mem::Int(-5)];
        assert_eq!(func_min_scalar(&args).unwrap(), Mem::Int(-5));

        let with_null = [Mem::Int(10), Mem::Null, Mem::Int(-5)];
        assert_eq!(func_min_scalar(&with_null).unwrap(), Mem::Null);
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_round() {
        assert_eq!(
            func_round(&[Mem::Real(std::f64::consts::PI), Mem::Int(2)]).unwrap(),
            Mem::Real(3.14)
        );
        assert_eq!(
            func_round(&[Mem::Real(std::f64::consts::PI)]).unwrap(),
            Mem::Real(3.0)
        );
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_sign() {
        assert_eq!(func_sign(&[Mem::Int(-42)]).unwrap(), Mem::Int(-1));
        assert_eq!(
            func_sign(&[Mem::Real(std::f64::consts::PI)]).unwrap(),
            Mem::Int(1)
        );
        assert_eq!(func_sign(&[Mem::Int(0)]).unwrap(), Mem::Int(0));
    }

    #[test]
    fn test_substr() {
        let text = Mem::Text(std::sync::Arc::from("hello world"));

        // substr('hello world', 1, 5) -> 'hello'
        assert_eq!(
            func_substr(&[text.clone(), Mem::Int(1), Mem::Int(5)]).unwrap(),
            Mem::Text(std::sync::Arc::from("hello"))
        );

        // substr('hello world', 7) -> 'world'
        assert_eq!(
            func_substr(&[text.clone(), Mem::Int(7)]).unwrap(),
            Mem::Text(std::sync::Arc::from("world"))
        );

        // substr('hello world', -5, 3) -> 'wor'
        assert_eq!(
            func_substr(&[text.clone(), Mem::Int(-5), Mem::Int(3)]).unwrap(),
            Mem::Text(std::sync::Arc::from("wor"))
        );

        // substr('hello world', 7, -3) -> 'lo '
        assert_eq!(
            func_substr(&[text.clone(), Mem::Int(7), Mem::Int(-3)]).unwrap(),
            Mem::Text(std::sync::Arc::from("lo "))
        );
    }

    #[test]
    fn test_instr() {
        let text = Mem::Text(std::sync::Arc::from("hello world"));
        assert_eq!(
            func_instr(&[text.clone(), Mem::Text(std::sync::Arc::from("world"))]).unwrap(),
            Mem::Int(7)
        );
        assert_eq!(
            func_instr(&[text.clone(), Mem::Text(std::sync::Arc::from("x"))]).unwrap(),
            Mem::Int(0)
        );
    }

    #[test]
    fn test_replace() {
        let text = Mem::Text(std::sync::Arc::from("hello world"));
        assert_eq!(
            func_replace(&[
                text.clone(),
                Mem::Text(std::sync::Arc::from("world")),
                Mem::Text(std::sync::Arc::from("rust"))
            ])
            .unwrap(),
            Mem::Text(std::sync::Arc::from("hello rust"))
        );
    }

    #[test]
    fn test_trim() {
        let text = Mem::Text(std::sync::Arc::from("  hello  "));
        assert_eq!(
            func_trim(std::slice::from_ref(&text)).unwrap(),
            Mem::Text(std::sync::Arc::from("hello"))
        );

        let custom_text = Mem::Text(std::sync::Arc::from("xxhelloxx"));
        assert_eq!(
            func_trim(&[custom_text, Mem::Text(std::sync::Arc::from("x"))]).unwrap(),
            Mem::Text(std::sync::Arc::from("hello"))
        );
    }

    #[test]
    fn test_datetime_parsing() {
        // Test parsing unix timestamp
        let dt = func_datetime(&[Mem::Int(1672531200)]).unwrap();
        assert_eq!(dt, Mem::Text(std::sync::Arc::from("2023-01-01 00:00:00")));

        // Test parsing YYYY-MM-DD
        let d = func_date(&[Mem::Text(std::sync::Arc::from("2023-01-01"))]).unwrap();
        assert_eq!(d, Mem::Text(std::sync::Arc::from("2023-01-01")));

        // Test julianday — result must be a Real within expected range.
        let jd = func_julianday(&[Mem::Text(std::sync::Arc::from("2023-01-01"))]).unwrap();
        let jd_f = match jd {
            Mem::Real(f) => f,
            other => panic!("Expected Real, got {other:?}"),
        };
        assert!((jd_f - 2459945.5).abs() < 0.0001);
    }

    // ── Aggregate dispatch and state machines ──────────────────────────────

    #[test]
    fn dispatch_aggregate_unknown() {
        assert!(dispatch_aggregate("bogus").is_err());
    }

    #[test]
    fn count_aggregate_star_and_column() {
        // COUNT(*) style: called with no args, every step counts.
        let mut star = dispatch_aggregate("count").unwrap();
        star.step(&[]).unwrap();
        star.step(&[]).unwrap();
        assert_eq!(star.finalize().unwrap(), Mem::Int(2));

        // COUNT(col): NULLs are not counted.
        let mut col = dispatch_aggregate("count").unwrap();
        col.step(&[Mem::Int(1)]).unwrap();
        col.step(&[Mem::Null]).unwrap();
        col.step(&[Mem::Int(3)]).unwrap();
        assert_eq!(col.finalize().unwrap(), Mem::Int(2));
    }

    #[test]
    fn sum_aggregate_all_paths() {
        // Never stepped -> NULL.
        let mut empty = dispatch_aggregate("sum").unwrap();
        assert_eq!(empty.finalize().unwrap(), Mem::Null);

        // All-NULL input -> NULL.
        let mut all_null = dispatch_aggregate("sum").unwrap();
        all_null.step(&[Mem::Null]).unwrap();
        assert_eq!(all_null.finalize().unwrap(), Mem::Null);

        // Integer accumulation without overflow.
        let mut ints = dispatch_aggregate("sum").unwrap();
        ints.step(&[Mem::Int(2)]).unwrap();
        ints.step(&[Mem::Int(3)]).unwrap();
        assert_eq!(ints.finalize().unwrap(), Mem::Int(5));

        // Integer overflow promotes the accumulator to Real.
        let mut overflow = dispatch_aggregate("sum").unwrap();
        overflow.step(&[Mem::Int(i64::MAX)]).unwrap();
        overflow.step(&[Mem::Int(1)]).unwrap();
        assert_eq!(
            overflow.finalize().unwrap(),
            Mem::Real(i64::MAX as f64 + 1.0)
        );

        // Int accumulator + Real value promotes to Real.
        let mut int_then_real = dispatch_aggregate("sum").unwrap();
        int_then_real.step(&[Mem::Int(2)]).unwrap();
        int_then_real.step(&[Mem::Real(1.5)]).unwrap();
        assert_eq!(int_then_real.finalize().unwrap(), Mem::Real(3.5));

        // Int accumulator + non-numeric value falls back to 0.0 via to_real().
        let mut int_then_text = dispatch_aggregate("sum").unwrap();
        int_then_text.step(&[Mem::Int(2)]).unwrap();
        int_then_text
            .step(&[Mem::Text(std::sync::Arc::from("abc"))])
            .unwrap();
        assert_eq!(int_then_text.finalize().unwrap(), Mem::Real(2.0));

        // First value is Real -> accumulator starts as Real.
        let mut real_start = dispatch_aggregate("sum").unwrap();
        real_start.step(&[Mem::Real(1.5)]).unwrap();
        real_start.step(&[Mem::Real(2.5)]).unwrap();
        assert_eq!(real_start.finalize().unwrap(), Mem::Real(4.0));

        // First value non-numeric -> starts as Real via to_real() fallback.
        let mut text_start = dispatch_aggregate("sum").unwrap();
        text_start
            .step(&[Mem::Text(std::sync::Arc::from("xyz"))])
            .unwrap();
        assert_eq!(text_start.finalize().unwrap(), Mem::Real(0.0));

        // Real accumulator + Int value.
        let mut real_then_int = dispatch_aggregate("sum").unwrap();
        real_then_int.step(&[Mem::Real(1.0)]).unwrap();
        real_then_int.step(&[Mem::Int(2)]).unwrap();
        assert_eq!(real_then_int.finalize().unwrap(), Mem::Real(3.0));

        // Force coverage of the unreachable Some(_) catch-all arm: directly
        // construct a SumState whose accumulator is a variant that normal
        // usage can never produce.
        use liter_vdbe::AggregateState;
        let mut weird = SumState {
            sum: Some(Mem::Text(std::sync::Arc::from("oops"))),
        };
        // The Some(_) arm does nothing, so the accumulator stays unchanged.
        weird.step(&[Mem::Int(1)]).unwrap();
        assert_eq!(
            weird.finalize().unwrap(),
            Mem::Text(std::sync::Arc::from("oops"))
        );
    }

    #[test]
    fn min_aggregate_paths() {
        let mut empty = dispatch_aggregate("min").unwrap();
        assert_eq!(empty.finalize().unwrap(), Mem::Null);

        let mut m = dispatch_aggregate("min").unwrap();
        m.step(&[]).unwrap(); // no args at all (defensive no-op path)
        m.step(&[Mem::Null]).unwrap(); // ignored
        m.step(&[Mem::Int(5)]).unwrap(); // first real value
        m.step(&[Mem::Int(2)]).unwrap(); // smaller -> replaces
        m.step(&[Mem::Int(9)]).unwrap(); // larger -> ignored
        assert_eq!(m.finalize().unwrap(), Mem::Int(2));
    }

    #[test]
    fn max_aggregate_paths() {
        let mut empty = dispatch_aggregate("max").unwrap();
        assert_eq!(empty.finalize().unwrap(), Mem::Null);

        let mut m = dispatch_aggregate("max").unwrap();
        m.step(&[]).unwrap(); // no args at all (defensive no-op path)
        m.step(&[Mem::Null]).unwrap(); // ignored
        m.step(&[Mem::Int(5)]).unwrap();
        m.step(&[Mem::Int(9)]).unwrap(); // larger -> replaces
        m.step(&[Mem::Int(2)]).unwrap(); // smaller -> ignored
        assert_eq!(m.finalize().unwrap(), Mem::Int(9));
    }

    #[test]
    fn avg_aggregate_paths() {
        let mut empty = dispatch_aggregate("avg").unwrap();
        assert_eq!(empty.finalize().unwrap(), Mem::Null);

        let mut a = dispatch_aggregate("avg").unwrap();
        a.step(&[]).unwrap(); // no args at all (defensive no-op path)
        a.step(&[Mem::Int(2)]).unwrap();
        a.step(&[Mem::Int(4)]).unwrap();
        // Non-numeric input (to_real() == None) is ignored, not counted.
        a.step(&[Mem::Text(std::sync::Arc::from("nan"))]).unwrap();
        assert_eq!(a.finalize().unwrap(), Mem::Real(3.0));
    }

    // ── Scalar dispatch ─────────────────────────────────────────────────────

    #[test]
    fn dispatch_function_routes_known_names() {
        let one = [Mem::Int(1)];
        let txt = |s: &str| Mem::Text(std::sync::Arc::from(s));

        assert!(dispatch_function("abs", &one).is_ok());
        assert!(dispatch_function("length", &[txt("hi")]).is_ok());
        assert!(dispatch_function("typeof", &one).is_ok());
        assert!(dispatch_function("upper", &[txt("a")]).is_ok());
        assert!(dispatch_function("lower", &[txt("A")]).is_ok());
        assert!(dispatch_function("coalesce", &[Mem::Null, Mem::Int(1)]).is_ok());
        assert!(dispatch_function("ifnull", &[Mem::Null, Mem::Int(1)]).is_ok());
        assert!(dispatch_function("max", &one).is_ok());
        assert!(dispatch_function("min", &one).is_ok());
        assert!(dispatch_function("round", &[Mem::Real(1.2)]).is_ok());
        assert!(dispatch_function("sign", &one).is_ok());
        assert!(dispatch_function("substr", &[txt("hi"), Mem::Int(1)]).is_ok());
        assert!(dispatch_function("substring", &[txt("hi"), Mem::Int(1)]).is_ok());
        assert!(dispatch_function("instr", &[txt("hi"), txt("h")]).is_ok());
        assert!(dispatch_function("replace", &[txt("hi"), txt("h"), txt("y")]).is_ok());
        assert!(dispatch_function("trim", &[txt(" hi ")]).is_ok());
        assert!(dispatch_function("date", &[txt("2023-01-01")]).is_ok());
        assert!(dispatch_function("time", &[txt("2023-01-01")]).is_ok());
        assert!(dispatch_function("datetime", &[txt("2023-01-01")]).is_ok());
        assert!(dispatch_function("julianday", &[txt("2023-01-01")]).is_ok());
        assert!(dispatch_function("strftime", &[txt("%Y"), txt("2023-01-01")]).is_ok());

        assert!(matches!(
            dispatch_function("nope", &[]),
            Err(FuncError::NotImplemented(name)) if name == "nope"
        ));
    }

    // ── length / typeof / upper / lower ────────────────────────────────────

    #[test]
    fn length_blob_null_and_unsupported() {
        assert_eq!(
            func_length(&[Mem::Blob(std::sync::Arc::from(vec![1u8, 2, 3]))]).unwrap(),
            Mem::Int(3)
        );
        assert_eq!(func_length(&[Mem::Null]).unwrap(), Mem::Null);
        assert_eq!(func_length(&[]).unwrap(), Mem::Null);
        assert_eq!(func_length(&[Mem::Agg(0)]).unwrap(), Mem::Null);
    }

    #[test]
    fn typeof_all_variants() {
        assert_eq!(
            func_typeof(&[Mem::Real(1.5)]).unwrap(),
            Mem::Text(std::sync::Arc::from("real"))
        );
        assert_eq!(
            func_typeof(&[Mem::Text(std::sync::Arc::from("x"))]).unwrap(),
            Mem::Text(std::sync::Arc::from("text"))
        );
        assert_eq!(
            func_typeof(&[Mem::Blob(std::sync::Arc::from(vec![1u8]))]).unwrap(),
            Mem::Text(std::sync::Arc::from("blob"))
        );
        assert_eq!(
            func_typeof(&[Mem::ZeroBlob(4)]).unwrap(),
            Mem::Text(std::sync::Arc::from("blob"))
        );
        assert_eq!(
            func_typeof(&[]).unwrap(),
            Mem::Text(std::sync::Arc::from("null"))
        );
        // Aggregate handles are surfaced to SQL as blobs.
        assert_eq!(
            func_typeof(&[Mem::Agg(0)]).unwrap(),
            Mem::Text(std::sync::Arc::from("blob"))
        );
    }

    #[test]
    fn upper_lower_null_and_non_text() {
        assert_eq!(func_upper(&[Mem::Null]).unwrap(), Mem::Null);
        assert_eq!(func_upper(&[]).unwrap(), Mem::Null);
        assert_eq!(func_upper(&[Mem::Int(5)]).unwrap(), Mem::Null);
        assert_eq!(func_lower(&[Mem::Null]).unwrap(), Mem::Null);
        assert_eq!(func_lower(&[]).unwrap(), Mem::Null);
        assert_eq!(func_lower(&[Mem::Int(5)]).unwrap(), Mem::Null);
        assert_eq!(
            func_upper(&[Mem::Text(std::sync::Arc::from("hello"))]).unwrap(),
            Mem::Text(std::sync::Arc::from("HELLO"))
        );
        assert_eq!(
            func_lower(&[Mem::Text(std::sync::Arc::from("HELLO"))]).unwrap(),
            Mem::Text(std::sync::Arc::from("hello"))
        );
    }

    #[test]
    fn abs_real_and_unsupported() {
        assert_eq!(func_abs(&[Mem::Real(-3.5)]).unwrap(), Mem::Real(3.5));
        assert_eq!(
            func_abs(&[Mem::Text(std::sync::Arc::from("x"))]).unwrap(),
            Mem::Null
        );
        assert_eq!(func_abs(&[]).unwrap(), Mem::Null);
        // i64::MIN has no positive counterpart; checked_abs saturates to MAX.
        assert_eq!(func_abs(&[Mem::Int(i64::MIN)]).unwrap(), Mem::Int(i64::MAX));
    }

    // ── coalesce / ifnull ───────────────────────────────────────────────────

    #[test]
    fn coalesce_all_null() {
        assert_eq!(func_coalesce(&[Mem::Null, Mem::Null]).unwrap(), Mem::Null);
        assert_eq!(func_coalesce(&[]).unwrap(), Mem::Null);
    }

    #[test]
    fn ifnull_wrong_arg_count_and_ok() {
        assert!(func_ifnull(&[Mem::Null]).is_err());
        assert!(func_ifnull(&[Mem::Null, Mem::Int(1), Mem::Int(2)]).is_err());
        assert_eq!(func_ifnull(&[Mem::Null, Mem::Int(7)]).unwrap(), Mem::Int(7));
        assert_eq!(func_ifnull(&[Mem::Int(3), Mem::Int(7)]).unwrap(), Mem::Int(3));
    }

    // ── substr ──────────────────────────────────────────────────────────────

    #[test]
    fn substr_wrong_arg_count() {
        assert!(func_substr(&[Mem::Text(std::sync::Arc::from("x"))]).is_err());
        assert!(func_substr(&[
            Mem::Text(std::sync::Arc::from("x")),
            Mem::Int(1),
            Mem::Int(1),
            Mem::Int(1)
        ])
        .is_err());
    }

    #[test]
    fn substr_null_and_non_text_first_arg() {
        assert_eq!(func_substr(&[Mem::Null, Mem::Int(1)]).unwrap(), Mem::Null);
        assert_eq!(
            func_substr(&[Mem::Int(12345), Mem::Int(2), Mem::Int(2)]).unwrap(),
            Mem::Text(std::sync::Arc::from("23"))
        );
    }

    #[test]
    fn substr_start_variants() {
        let text = Mem::Text(std::sync::Arc::from("abcdef"));
        // start == 0 behaves like start == 1.
        assert_eq!(
            func_substr(&[text.clone(), Mem::Int(0), Mem::Int(3)]).unwrap(),
            Mem::Text(std::sync::Arc::from("abc"))
        );
        // start from a Real.
        assert_eq!(
            func_substr(&[text.clone(), Mem::Real(2.0), Mem::Int(2)]).unwrap(),
            Mem::Text(std::sync::Arc::from("bc"))
        );
        // start from a parseable Text.
        assert_eq!(
            func_substr(&[
                text.clone(),
                Mem::Text(std::sync::Arc::from("3")),
                Mem::Int(2)
            ])
            .unwrap(),
            Mem::Text(std::sync::Arc::from("cd"))
        );
        // start from an unparseable Text defaults to 0.
        assert_eq!(
            func_substr(&[
                text.clone(),
                Mem::Text(std::sync::Arc::from("nope")),
                Mem::Int(2)
            ])
            .unwrap(),
            Mem::Text(std::sync::Arc::from("ab"))
        );
        // start of an unsupported type yields NULL.
        assert_eq!(func_substr(&[text.clone(), Mem::Null]).unwrap(), Mem::Null);
        // negative start far beyond the string length clamps to the beginning.
        assert_eq!(
            func_substr(&[text.clone(), Mem::Int(-100), Mem::Int(2)]).unwrap(),
            Mem::Text(std::sync::Arc::from("ab"))
        );
    }

    #[test]
    fn substr_length_variants() {
        let text = Mem::Text(std::sync::Arc::from("abcdef"));
        // length from a Real.
        assert_eq!(
            func_substr(&[text.clone(), Mem::Int(1), Mem::Real(3.0)]).unwrap(),
            Mem::Text(std::sync::Arc::from("abc"))
        );
        // length from a parseable Text.
        assert_eq!(
            func_substr(&[
                text.clone(),
                Mem::Int(1),
                Mem::Text(std::sync::Arc::from("2"))
            ])
            .unwrap(),
            Mem::Text(std::sync::Arc::from("ab"))
        );
        // length from an unparseable Text defaults to 0.
        assert_eq!(
            func_substr(&[
                text.clone(),
                Mem::Int(1),
                Mem::Text(std::sync::Arc::from("bogus"))
            ])
            .unwrap(),
            Mem::Text(std::sync::Arc::from(""))
        );
        // length of an unsupported type is treated as "no length limit".
        assert_eq!(
            func_substr(&[text.clone(), Mem::Int(4), Mem::Null]).unwrap(),
            Mem::Text(std::sync::Arc::from("def"))
        );
        // length larger than the remaining string clamps to the end.
        assert_eq!(
            func_substr(&[text.clone(), Mem::Int(4), Mem::Int(100)]).unwrap(),
            Mem::Text(std::sync::Arc::from("def"))
        );
    }

    // ── instr / replace / trim ──────────────────────────────────────────────

    #[test]
    fn instr_wrong_arg_count_nulls_and_coercion() {
        assert!(func_instr(&[Mem::Text(std::sync::Arc::from("x"))]).is_err());
        assert_eq!(
            func_instr(&[Mem::Null, Mem::Text(std::sync::Arc::from("x"))]).unwrap(),
            Mem::Null
        );
        assert_eq!(
            func_instr(&[Mem::Text(std::sync::Arc::from("x")), Mem::Null]).unwrap(),
            Mem::Null
        );
        // Non-text arguments are coerced through mem_to_string.
        assert_eq!(
            func_instr(&[Mem::Int(12345), Mem::Int(23)]).unwrap(),
            Mem::Int(2)
        );
        // Real arguments are also coerced through mem_to_string.
        assert_eq!(
            func_instr(&[Mem::Real(3.14), Mem::Text(std::sync::Arc::from("14"))]).unwrap(),
            Mem::Int(3)
        );
    }

    #[test]
    fn replace_wrong_arg_count_nulls_and_coercion() {
        assert!(func_replace(&[Mem::Text(std::sync::Arc::from("x"))]).is_err());
        assert_eq!(
            func_replace(&[
                Mem::Null,
                Mem::Text(std::sync::Arc::from("a")),
                Mem::Text(std::sync::Arc::from("b"))
            ])
            .unwrap(),
            Mem::Null
        );
        assert_eq!(
            func_replace(&[
                Mem::Text(std::sync::Arc::from("abc")),
                Mem::Null,
                Mem::Text(std::sync::Arc::from("b"))
            ])
            .unwrap(),
            Mem::Null
        );
        assert_eq!(
            func_replace(&[
                Mem::Text(std::sync::Arc::from("abc")),
                Mem::Text(std::sync::Arc::from("a")),
                Mem::Null
            ])
            .unwrap(),
            Mem::Null
        );
        // Non-text arguments are coerced through mem_to_string.
        assert_eq!(
            func_replace(&[
                Mem::Int(1231),
                Mem::Int(23),
                Mem::Text(std::sync::Arc::from("X"))
            ])
            .unwrap(),
            Mem::Text(std::sync::Arc::from("1X1"))
        );
        // Non-text replacement argument is also coerced through mem_to_string.
        assert_eq!(
            func_replace(&[
                Mem::Text(std::sync::Arc::from("ab")),
                Mem::Text(std::sync::Arc::from("a")),
                Mem::Int(5)
            ])
            .unwrap(),
            Mem::Text(std::sync::Arc::from("5b"))
        );
    }

    #[test]
    fn trim_wrong_arg_count_null_and_coercion() {
        assert!(func_trim(&[]).is_err());
        assert!(func_trim(&[
            Mem::Text(std::sync::Arc::from("x")),
            Mem::Text(std::sync::Arc::from("y")),
            Mem::Text(std::sync::Arc::from("z"))
        ])
        .is_err());
        assert_eq!(func_trim(&[Mem::Null]).unwrap(), Mem::Null);
        // Non-text first argument is coerced through mem_to_string.
        assert_eq!(
            func_trim(&[Mem::Int(42)]).unwrap(),
            Mem::Text(std::sync::Arc::from("42"))
        );
        // Non-text second argument falls back to trimming spaces.
        assert_eq!(
            func_trim(&[Mem::Text(std::sync::Arc::from("  hi  ")), Mem::Int(1)]).unwrap(),
            Mem::Text(std::sync::Arc::from("hi"))
        );
        // A Real first argument is coerced through mem_to_string's Real arm.
        assert_eq!(
            func_trim(&[Mem::Real(3.5)]).unwrap(),
            Mem::Text(std::sync::Arc::from("3.5"))
        );
        // A Blob first argument has no textual representation, so it
        // coerces to the empty string via mem_to_string's fallback arm.
        assert_eq!(
            func_trim(&[Mem::Blob(std::sync::Arc::from(vec![1u8, 2]))]).unwrap(),
            Mem::Text(std::sync::Arc::from(""))
        );
    }

    // ── date / time / datetime / julianday / strftime ───────────────────────

    #[test]
    fn date_time_functions() {
        assert_eq!(
            func_time(&[Mem::Text(std::sync::Arc::from("2023-01-01 13:45:30"))]).unwrap(),
            Mem::Text(std::sync::Arc::from("13:45:30"))
        );
        assert_eq!(
            func_date(&[Mem::Text(std::sync::Arc::from("2023-01-01 13:45:30"))]).unwrap(),
            Mem::Text(std::sync::Arc::from("2023-01-01"))
        );
        assert_eq!(
            func_datetime(&[Mem::Text(std::sync::Arc::from("2023-06-15 10:30:00"))]).unwrap(),
            Mem::Text(std::sync::Arc::from("2023-06-15 10:30:00"))
        );
    }

    #[test]
    fn date_functions_invalid_and_missing_input() {
        // No arguments -> NULL.
        assert_eq!(func_date(&[]).unwrap(), Mem::Null);
        assert_eq!(func_julianday(&[]).unwrap(), Mem::Null);
        // Unparseable text -> NULL.
        assert_eq!(
            func_date(&[Mem::Text(std::sync::Arc::from("not-a-date"))]).unwrap(),
            Mem::Null
        );
        // Unsupported argument type -> NULL.
        assert_eq!(
            func_date(&[Mem::Blob(std::sync::Arc::from(vec![1u8]))]).unwrap(),
            Mem::Null
        );
        assert_eq!(func_date(&[Mem::Null]).unwrap(), Mem::Null);
    }

    #[test]
    fn date_functions_now_and_julian_real_roundtrip() {
        // "now" (case-insensitively) resolves to the current time rather than NULL.
        let now_date = func_date(&[Mem::Text(std::sync::Arc::from("NOW"))]).unwrap();
        let now_text = match now_date {
            Mem::Text(t) => t,
            other => panic!("expected text, got {other:?}"),
        };
        assert_eq!(now_text.len(), 10);

        // A Julian day (Real) round-trips back to the same calendar date/time.
        assert_eq!(
            func_datetime(&[Mem::Real(2459945.5)]).unwrap(),
            Mem::Text(std::sync::Arc::from("2023-01-01 00:00:00"))
        );
    }

    #[test]
    fn strftime_function() {
        assert!(func_strftime(&[]).is_err());
        // Non-text format -> NULL.
        assert_eq!(func_strftime(&[Mem::Int(1)]).unwrap(), Mem::Null);
        assert_eq!(
            func_strftime(&[
                Mem::Text(std::sync::Arc::from("%Y/%m/%d")),
                Mem::Text(std::sync::Arc::from("2023-01-01"))
            ])
            .unwrap(),
            Mem::Text(std::sync::Arc::from("2023/01/01"))
        );
    }

    // ── max()/min() scalar and compare_mem ──────────────────────────────────

    #[test]
    fn max_min_scalar_empty_and_null_equal() {
        assert_eq!(func_max_scalar(&[]).unwrap(), Mem::Null);
        assert_eq!(func_min_scalar(&[]).unwrap(), Mem::Null);
        // Two NULLs compare equal; neither replaces the other.
        assert_eq!(func_max_scalar(&[Mem::Null, Mem::Null]).unwrap(), Mem::Null);
    }

    #[test]
    fn max_min_scalar_mixed_types() {
        // Int vs Real, both orderings.
        assert_eq!(
            func_max_scalar(&[Mem::Int(5), Mem::Real(1.0)]).unwrap(),
            Mem::Int(5)
        );
        assert_eq!(
            func_max_scalar(&[Mem::Real(1.0), Mem::Int(5)]).unwrap(),
            Mem::Int(5)
        );

        // Text vs Text.
        assert_eq!(
            func_max_scalar(&[
                Mem::Text(std::sync::Arc::from("a")),
                Mem::Text(std::sync::Arc::from("b"))
            ])
            .unwrap(),
            Mem::Text(std::sync::Arc::from("b"))
        );

        // Numeric vs Text: text sorts higher than numbers in both directions.
        assert_eq!(
            func_max_scalar(&[Mem::Int(5), Mem::Text(std::sync::Arc::from("abc"))]).unwrap(),
            Mem::Text(std::sync::Arc::from("abc"))
        );
        assert_eq!(
            func_min_scalar(&[Mem::Text(std::sync::Arc::from("abc")), Mem::Int(5)]).unwrap(),
            Mem::Int(5)
        );

        // Unhandled type combination (e.g. two blobs) falls back to "equal",
        // so the first value seen is kept.
        let blobs = [
            Mem::Blob(std::sync::Arc::from(vec![1u8])),
            Mem::Blob(std::sync::Arc::from(vec![2u8])),
        ];
        assert_eq!(func_max_scalar(&blobs).unwrap(), blobs[0].clone());

        // Real vs Real.
        assert_eq!(
            func_max_scalar(&[Mem::Real(1.5), Mem::Real(2.5)]).unwrap(),
            Mem::Real(2.5)
        );
        assert_eq!(
            func_min_scalar(&[Mem::Real(1.5), Mem::Real(2.5)]).unwrap(),
            Mem::Real(1.5)
        );
    }

    // ── round() ───────────────────────────────────────────────────────────

    #[test]
    fn round_edge_cases() {
        assert_eq!(func_round(&[]).unwrap(), Mem::Null);
        assert_eq!(func_round(&[Mem::Null]).unwrap(), Mem::Null);
        assert_eq!(
            func_round(&[Mem::Text(std::sync::Arc::from("2.5"))]).unwrap(),
            Mem::Real(3.0)
        );
        assert_eq!(
            func_round(&[Mem::Text(std::sync::Arc::from("bogus"))]).unwrap(),
            Mem::Real(0.0)
        );
        // Unsupported value type defaults to 0.0.
        assert_eq!(
            func_round(&[Mem::Blob(std::sync::Arc::from(vec![1u8]))]).unwrap(),
            Mem::Real(0.0)
        );
        // Digits from a Real argument.
        assert_eq!(
            func_round(&[Mem::Real(1.2345), Mem::Real(2.0)]).unwrap(),
            Mem::Real(1.23)
        );
        // Unsupported digits type defaults to 0 digits.
        assert_eq!(
            func_round(&[Mem::Real(1.6), Mem::Null]).unwrap(),
            Mem::Real(2.0)
        );
        // Integer value is coerced to f64 then rounded.
        assert_eq!(func_round(&[Mem::Int(5)]).unwrap(), Mem::Real(5.0));
    }

    // ── sign() ───────────────────────────────────────────────────────────

    #[test]
    fn sign_extra_cases() {
        assert_eq!(func_sign(&[Mem::Real(-3.5)]).unwrap(), Mem::Int(-1));
        assert_eq!(func_sign(&[Mem::Real(0.0)]).unwrap(), Mem::Int(0));
        assert_eq!(func_sign(&[Mem::Null]).unwrap(), Mem::Null);
        assert_eq!(func_sign(&[]).unwrap(), Mem::Null);
        // Unsupported type defaults to 0.
        assert_eq!(
            func_sign(&[Mem::Text(std::sync::Arc::from("x"))]).unwrap(),
            Mem::Int(0)
        );
    }

    #[test]
    fn test_zeroblob() {
        assert_eq!(
            dispatch_function("zeroblob", &[Mem::Int(5)]).unwrap(),
            Mem::ZeroBlob(5)
        );
        assert_eq!(
            dispatch_function("zeroblob", &[Mem::Real(3.0)]).unwrap(),
            Mem::ZeroBlob(3)
        );
        assert_eq!(
            dispatch_function("zeroblob", &[Mem::Null]).unwrap(),
            Mem::Null
        );
        assert_eq!(
            dispatch_function("zeroblob", &[Mem::Int(-1)]).unwrap(),
            Mem::Null
        );
        assert!(dispatch_function("zeroblob", &[]).is_err());
        assert_eq!(
            func_length(&[Mem::ZeroBlob(10)]).unwrap(),
            Mem::Int(10)
        );
    }
}
