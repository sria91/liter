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

pub fn dispatch_function(name: &str, args: &[Mem]) -> Result<Mem, String> {
    match name.to_ascii_lowercase().as_str() {
        "json_valid" => func_json_valid(args).map_err(|e| e.to_string()),
        "json_extract" | "->>" | "->" => func_json_extract(args).map_err(|e| e.to_string()),
        "json_object" => func_json_object(args).map_err(|e| e.to_string()),
        "json_array" => func_json_array(args).map_err(|e| e.to_string()),
        _ => Err(format!("Not implemented: {}", name)),
    }
}

fn mem_to_string(m: &Mem) -> String {
    match m {
        Mem::Text(t) => t.to_string(),
        Mem::Int(i) => i.to_string(),
        Mem::Real(f) => f.to_string(),
        _ => String::new(),
    }
}

fn mem_to_json(m: &Mem) -> JsonValue {
    match m {
        Mem::Null => JsonValue::Null,
        Mem::Int(i) => JsonValue::Number((*i).into()),
        Mem::Real(f) => JsonValue::Number(serde_json::Number::from_f64(*f).unwrap_or(serde_json::Number::from(0))),
        Mem::Text(t) => {
            // In a full implementation we'd check for JSON subtype.
            // For now, if it parses as JSON, we treat it as JSON (like json(x)).
            if let Ok(v) = serde_json::from_str(t) {
                v
            } else {
                JsonValue::String(t.to_string())
            }
        }
        Mem::Blob(b) => JsonValue::String(String::from_utf8_lossy(b).to_string()),
        _ => JsonValue::Null,
    }
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

pub fn func_json_object(args: &[Mem]) -> Result<Mem, JsonError> {
    if args.len() & 1 != 0 {
        return Err(JsonError::WrongArgCount("json_object".into()));
    }
    let mut map = serde_json::Map::new();
    for i in (0..args.len()).step_by(2) {
        let key = match &args[i] {
            Mem::Text(t) => t.to_string(),
            Mem::Null => return Err(JsonError::WrongArgCount("json_object label cannot be null".into())),
            m => mem_to_string(m),
        };
        let val = mem_to_json(&args[i+1]);
        map.insert(key, val);
    }
    let obj = JsonValue::Object(map);
    Ok(Mem::Text(Arc::from(obj.to_string().as_str())))
}

pub fn func_json_array(args: &[Mem]) -> Result<Mem, JsonError> {
    let mut vec = Vec::new();
    for arg in args {
        vec.push(mem_to_json(arg));
    }
    let arr = JsonValue::Array(vec);
    Ok(Mem::Text(Arc::from(arr.to_string().as_str())))
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

    #[test]
    fn test_json_object() {
        let key1 = Mem::Text(Arc::from("a"));
        let val1 = Mem::Int(1);
        let key2 = Mem::Text(Arc::from("b"));
        let val2 = Mem::Text(Arc::from("hello"));
        assert_eq!(
            func_json_object(&[key1, val1, key2, val2]).unwrap(),
            Mem::Text(Arc::from("{\"a\":1,\"b\":\"hello\"}"))
        );
    }

    #[test]
    fn test_json_array() {
        let val1 = Mem::Int(1);
        let val2 = Mem::Text(Arc::from("hello"));
        let val3 = Mem::Null;
        assert_eq!(
            func_json_array(&[val1, val2, val3]).unwrap(),
            Mem::Text(Arc::from("[1,\"hello\",null]"))
        );
    }
}
