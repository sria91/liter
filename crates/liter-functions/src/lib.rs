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
            if val.is_null() { return Ok(()); }
            match &self.sum {
                None => {
                    self.sum = match val {
                        Mem::Int(i) => Some(Mem::Int(*i)),
                        Mem::Real(f) => Some(Mem::Real(*f)),
                        v => Some(Mem::Real(v.to_real().unwrap_or(0.0))),
                    };
                }
                Some(Mem::Int(acc)) => {
                    match val {
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
                    }
                }
                Some(Mem::Real(acc)) => {
                    self.sum = Some(Mem::Real(*acc + val.to_real().unwrap_or(0.0)));
                }
                _ => {}
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
        Some(Mem::Null) | None => Ok(Mem::Null),
        _ => Ok(Mem::Null),
    }
}

pub fn func_typeof(args: &[Mem]) -> FuncResult<Mem> {
    let t = match args.first() {
        Some(Mem::Null)     => "null",
        Some(Mem::Int(_))   => "integer",
        Some(Mem::Real(_))  => "real",
        Some(Mem::Text(_))  => "text",
        Some(Mem::Blob(_)) | Some(Mem::ZeroBlob(_)) => "blob",
        None                => "null",
        Some(Mem::Agg(_))   => "blob", // Treat aggregators as blobs from SQL perspective
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
            let sub = chars[new_start as usize..end_idx as usize].iter().collect::<String>();
            return Ok(Mem::Text(std::sync::Arc::from(sub.as_str())));
        }
    }
    
    let actual_len = length.unwrap_or(len - start_idx);
    let end_idx = (start_idx + actual_len).min(len).max(0);
    let start_idx = start_idx.min(len).max(0);
    
    let sub = chars[start_idx as usize..end_idx as usize].iter().collect::<String>();
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
    let dt = match parse_datetime(args)? {
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

fn parse_datetime(args: &[Mem]) -> FuncResult<Option<chrono::DateTime<chrono::Utc>>> {
    if args.is_empty() {
        return Ok(None);
    }
    let time_val = match &args[0] {
        Mem::Text(t) => t.to_string(),
        Mem::Int(i) => return Ok(chrono::DateTime::from_timestamp(*i, 0)),
        Mem::Real(f) => {
            let millis = ((*f - 2440587.5) * 86400000.0) as i64;
            return Ok(chrono::DateTime::from_timestamp_millis(millis));
        }
        Mem::Null => return Ok(None),
        _ => return Ok(None),
    };
    
    if time_val.eq_ignore_ascii_case("now") {
        Ok(Some(chrono::Utc::now()))
    } else {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(&time_val, "%Y-%m-%d %H:%M:%S") {
            Ok(Some(dt.and_utc()))
        } else if let Ok(d) = chrono::NaiveDate::parse_from_str(&time_val, "%Y-%m-%d") {
            Ok(Some(d.and_hms_opt(0, 0, 0).unwrap_or_default().and_utc()))
        } else {
            Ok(None)
        }
    }
}

fn func_strftime_impl(format: &str, args: &[Mem]) -> FuncResult<Mem> {
    let dt = match parse_datetime(args)? {
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
        Some(Mem::Real(f)) => Ok(Mem::Int(if *f > 0.0 { 1 } else if *f < 0.0 { -1 } else { 0 })),
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
        assert_eq!(func_typeof(&[Mem::Int(1)]).unwrap(), Mem::Text(std::sync::Arc::from("integer")));
        assert_eq!(func_typeof(&[Mem::Null]).unwrap(), Mem::Text(std::sync::Arc::from("null")));
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
        assert_eq!(func_round(&[Mem::Real(std::f64::consts::PI), Mem::Int(2)]).unwrap(), Mem::Real(3.14));
        assert_eq!(func_round(&[Mem::Real(std::f64::consts::PI)]).unwrap(), Mem::Real(3.0));
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_sign() {
        assert_eq!(func_sign(&[Mem::Int(-42)]).unwrap(), Mem::Int(-1));
        assert_eq!(func_sign(&[Mem::Real(std::f64::consts::PI)]).unwrap(), Mem::Int(1));
        assert_eq!(func_sign(&[Mem::Int(0)]).unwrap(), Mem::Int(0));
    }

    #[test]
    fn test_substr() {
        let text = Mem::Text(std::sync::Arc::from("hello world"));
        
        // substr('hello world', 1, 5) -> 'hello'
        assert_eq!(func_substr(&[text.clone(), Mem::Int(1), Mem::Int(5)]).unwrap(), Mem::Text(std::sync::Arc::from("hello")));
        
        // substr('hello world', 7) -> 'world'
        assert_eq!(func_substr(&[text.clone(), Mem::Int(7)]).unwrap(), Mem::Text(std::sync::Arc::from("world")));
        
        // substr('hello world', -5, 3) -> 'wor'
        assert_eq!(func_substr(&[text.clone(), Mem::Int(-5), Mem::Int(3)]).unwrap(), Mem::Text(std::sync::Arc::from("wor")));
        
        // substr('hello world', 7, -3) -> 'lo '
        assert_eq!(func_substr(&[text.clone(), Mem::Int(7), Mem::Int(-3)]).unwrap(), Mem::Text(std::sync::Arc::from("lo ")));
    }

    #[test]
    fn test_instr() {
        let text = Mem::Text(std::sync::Arc::from("hello world"));
        assert_eq!(func_instr(&[text.clone(), Mem::Text(std::sync::Arc::from("world"))]).unwrap(), Mem::Int(7));
        assert_eq!(func_instr(&[text.clone(), Mem::Text(std::sync::Arc::from("x"))]).unwrap(), Mem::Int(0));
    }

    #[test]
    fn test_replace() {
        let text = Mem::Text(std::sync::Arc::from("hello world"));
        assert_eq!(func_replace(&[text.clone(), Mem::Text(std::sync::Arc::from("world")), Mem::Text(std::sync::Arc::from("rust"))]).unwrap(), Mem::Text(std::sync::Arc::from("hello rust")));
    }

    #[test]
    fn test_trim() {
        let text = Mem::Text(std::sync::Arc::from("  hello  "));
        assert_eq!(func_trim(std::slice::from_ref(&text)).unwrap(), Mem::Text(std::sync::Arc::from("hello")));
        
        let custom_text = Mem::Text(std::sync::Arc::from("xxhelloxx"));
        assert_eq!(func_trim(&[custom_text, Mem::Text(std::sync::Arc::from("x"))]).unwrap(), Mem::Text(std::sync::Arc::from("hello")));
    }

    #[test]
    fn test_datetime_parsing() {
        // Test parsing unix timestamp
        let dt = func_datetime(&[Mem::Int(1672531200)]).unwrap();
        assert_eq!(dt, Mem::Text(std::sync::Arc::from("2023-01-01 00:00:00")));

        // Test parsing YYYY-MM-DD
        let d = func_date(&[Mem::Text(std::sync::Arc::from("2023-01-01"))]).unwrap();
        assert_eq!(d, Mem::Text(std::sync::Arc::from("2023-01-01")));

        // Test julianday
        let jd = func_julianday(&[Mem::Text(std::sync::Arc::from("2023-01-01"))]).unwrap();
        if let Mem::Real(f) = jd {
            assert!((f - 2459945.5).abs() < 0.0001);
        } else {
            panic!("Expected Real");
        }
    }
}
