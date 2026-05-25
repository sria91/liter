//! Built-in SQL functions for SQLite3-rs.
//!
//! Mirrors `func.c` and `math.c`. Provides scalar and aggregate functions
//! callable from SQL expressions in the VDBE.
//!
//! ## Status
//! Phase 4 — scalar function stubs; aggregate stubs.

use sqlite3_vdbe::Mem;

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

pub fn dispatch_aggregate(name: &str) -> Result<Box<dyn sqlite3_vdbe::AggregateState>, String> {
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
impl sqlite3_vdbe::AggregateState for CountState {
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
    sum: Option<f64>,
}
impl sqlite3_vdbe::AggregateState for SumState {
    fn step(&mut self, args: &[Mem]) -> Result<(), String> {
        if let Some(val) = args.first() {
            if let Some(num) = val.to_real() {
                self.sum = Some(self.sum.unwrap_or(0.0) + num);
            }
        }
        Ok(())
    }
    fn finalize(&mut self) -> Result<Mem, String> {
        if let Some(s) = self.sum {
            Ok(Mem::Real(s))
        } else {
            Ok(Mem::Null)
        }
    }
}

#[derive(Debug)]
struct AvgState {
    sum: f64,
    count: i64,
}
impl sqlite3_vdbe::AggregateState for AvgState {
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
impl sqlite3_vdbe::AggregateState for MinState {
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
impl sqlite3_vdbe::AggregateState for MaxState {
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
    fn test_round() {
        assert_eq!(func_round(&[Mem::Real(3.14159), Mem::Int(2)]).unwrap(), Mem::Real(3.14));
        assert_eq!(func_round(&[Mem::Real(3.14159)]).unwrap(), Mem::Real(3.0));
    }

    #[test]
    fn test_sign() {
        assert_eq!(func_sign(&[Mem::Int(-42)]).unwrap(), Mem::Int(-1));
        assert_eq!(func_sign(&[Mem::Real(3.14)]).unwrap(), Mem::Int(1));
        assert_eq!(func_sign(&[Mem::Int(0)]).unwrap(), Mem::Int(0));
    }
}
