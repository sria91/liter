//! JSON1 extension for SQLite3-rs.
//!
//! Mirrors `json.c`. Provides `json()`, `json_extract()`, `json_object()`,
//! `json_array()`, `json_patch()`, and related functions.
//!
//! ## Status
//! Phase 3 — not yet implemented.

use sqlite3_vdbe::Mem;
use std::sync::Arc;
use serde_json::Value as JsonValue;

/// Error from a JSON function call.
#[derive(Debug, thiserror::Error)]
pub enum JsonError {
    #[error("wrong number of arguments to function {0}()")]
    WrongArgCount(String),
}

pub fn func_json_valid(args: &[Mem]) -> Result<Mem, JsonError> {
    if args.is_empty() {
        return Err(JsonError::WrongArgCount("json_valid".into()));
    }
    match &args[0] {
        Mem::Text(s) => {
            let is_valid = serde_json::from_str::<JsonValue>(s).is_ok();
            Ok(Mem::Int(if is_valid { 1 } else { 0 }))
        }
        Mem::Null => Ok(Mem::Null),
        _ => Ok(Mem::Int(0)),
    }
}

pub fn func_json_extract(args: &[Mem]) -> Result<Mem, JsonError> {
    if args.len() < 2 {
        return Err(JsonError::WrongArgCount("json_extract".into()));
    }
    
    let json_str = match &args[0] {
        Mem::Text(s) => s.as_ref(),
        Mem::Null => return Ok(Mem::Null),
        _ => return Ok(Mem::Null),
    };
    
    let path = match &args[1] {
        Mem::Text(s) => s.as_ref(),
        _ => return Ok(Mem::Null),
    };

    // Simplified extraction. sqlite handles `$.foo.bar` paths.
    // Here we just map `$.key` to `json_obj["key"]`
    let parsed = serde_json::from_str::<JsonValue>(json_str).ok();
    if let Some(mut val) = parsed {
        let parts = path.trim_start_matches('$').split('.').filter(|s| !s.is_empty());
        for part in parts {
            val = match val {
                JsonValue::Object(mut map) => map.remove(part).unwrap_or(JsonValue::Null),
                JsonValue::Array(vec) => {
                    if let Ok(idx) = part.parse::<usize>() {
                        if idx < vec.len() {
                            vec.into_iter().nth(idx).unwrap_or(JsonValue::Null)
                        } else {
                            JsonValue::Null
                        }
                    } else {
                        JsonValue::Null
                    }
                }
                _ => JsonValue::Null,
            }
        }
        
        match val {
            JsonValue::Null => Ok(Mem::Null),
            JsonValue::Bool(b) => Ok(Mem::Int(if b { 1 } else { 0 })),
            JsonValue::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Ok(Mem::Int(i))
                } else if let Some(f) = n.as_f64() {
                    Ok(Mem::Real(f))
                } else {
                    Ok(Mem::Null)
                }
            }
            JsonValue::String(s) => Ok(Mem::Text(Arc::from(s.as_str()))),
            JsonValue::Array(_) | JsonValue::Object(_) => {
                Ok(Mem::Text(Arc::from(val.to_string().as_str())))
            }
        }
    } else {
        Ok(Mem::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_valid() {
        let args = [Mem::Text(Arc::from("{\"a\": 1}"))];
        assert_eq!(func_json_valid(&args).unwrap(), Mem::Int(1));

        let args_invalid = [Mem::Text(Arc::from("{bad json"))];
        assert_eq!(func_json_valid(&args_invalid).unwrap(), Mem::Int(0));
    }

    #[test]
    fn test_json_extract() {
        let json = Mem::Text(Arc::from("{\"a\": {\"b\": 42}, \"c\": [10, 20]}"));
        let path1 = Mem::Text(Arc::from("$.a.b"));
        assert_eq!(func_json_extract(&[json.clone(), path1]).unwrap(), Mem::Int(42));
        
        let path2 = Mem::Text(Arc::from("$.c.1"));
        assert_eq!(func_json_extract(&[json, path2]).unwrap(), Mem::Int(20));
    }
}
