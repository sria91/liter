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

// ── Scalar functions ──────────────────────────────────────────────────────────

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

pub fn func_max_scalar(args: &[Mem]) -> FuncResult<Mem> {
    Err(FuncError::NotImplemented("max".into()))
}

pub fn func_min_scalar(args: &[Mem]) -> FuncResult<Mem> {
    Err(FuncError::NotImplemented("min".into()))
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
}
