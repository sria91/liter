//! JSON1 extension for Liter-rs.
//!
//! Mirrors `json.c`. Provides `json()`, `json_extract()`, `json_object()`,
//! `json_array()`, `json_patch()`, and related functions.
//!
//! ## Status
//! Phase 3 — not yet implemented.

use liter_vdbe::Mem;
use serde_json::Value as JsonValue;
use std::sync::Arc;

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
        "json_array" => Ok(func_json_array(args)),
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
        Mem::Real(f) => JsonValue::Number(
            serde_json::Number::from_f64(*f).unwrap_or(serde_json::Number::from(0)),
        ),
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
        let parts = path
            .trim_start_matches('$')
            .split('.')
            .filter(|s| !s.is_empty());
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
            JsonValue::Number(n) => match n.as_i64() {
                Some(i) => Ok(Mem::Int(i)),
                None => Ok(Mem::Real(n.as_f64().unwrap_or(0.0))),
            },
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
            Mem::Null => {
                return Err(JsonError::WrongArgCount(
                    "json_object label cannot be null".into(),
                ))
            }
            m => mem_to_string(m),
        };
        let val = mem_to_json(&args[i + 1]);
        map.insert(key, val);
    }
    let obj = JsonValue::Object(map);
    Ok(Mem::Text(Arc::from(obj.to_string().as_str())))
}

pub fn func_json_array(args: &[Mem]) -> Mem {
    let mut vec = Vec::new();
    for arg in args {
        vec.push(mem_to_json(arg));
    }
    let arr = JsonValue::Array(vec);
    Mem::Text(Arc::from(arr.to_string().as_str()))
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
        assert_eq!(
            func_json_extract(&[json.clone(), path1]).unwrap(),
            Mem::Int(42)
        );

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
            func_json_array(&[val1, val2, val3]),
            Mem::Text(Arc::from("[1,\"hello\",null]"))
        );
    }

    #[test]
    fn test_dispatch_function_all_branches() {
        let valid_args = [Mem::Text(Arc::from("1"))];
        assert!(dispatch_function("JSON_VALID", &valid_args).is_ok());

        let extract_args = [
            Mem::Text(Arc::from("{\"a\":1}")),
            Mem::Text(Arc::from("$.a")),
        ];
        assert!(dispatch_function("json_extract", &extract_args).is_ok());
        assert!(dispatch_function("->>", &extract_args).is_ok());
        assert!(dispatch_function("->", &extract_args).is_ok());

        let object_args = [Mem::Text(Arc::from("k")), Mem::Int(1)];
        assert!(dispatch_function("json_object", &object_args).is_ok());

        assert!(dispatch_function("json_array", &[Mem::Int(1)]).is_ok());

        assert!(dispatch_function("json_unknown_fn", &[]).is_err());
    }

    #[test]
    fn test_json_error_debug() {
        let err = JsonError::WrongArgCount("test".into());
        assert_eq!(format!("{err:?}"), "WrongArgCount(\"test\")");
        assert_eq!(format!("{err}"), "wrong number of arguments to function test()");
    }

    #[test]
    fn test_dispatch_function_propagates_errors_as_strings() {
        let err = dispatch_function("json_valid", &[]).unwrap_err();
        assert!(err.contains("json_valid"));

        let err2 = dispatch_function("json_extract", &[]).unwrap_err();
        assert!(err2.contains("json_extract"));

        let err2_arrow = dispatch_function("->>", &[]).unwrap_err();
        assert!(err2_arrow.contains("json_extract"));

        let err2_single_arrow = dispatch_function("->", &[]).unwrap_err();
        assert!(err2_single_arrow.contains("json_extract"));

        let err3 = dispatch_function("json_object", &[Mem::Null, Mem::Int(1)]).unwrap_err();
        assert!(err3.contains("json_object"));
    }

    #[test]
    fn test_mem_to_string_all_branches() {
        assert_eq!(mem_to_string(&Mem::Text(Arc::from("x"))), "x");
        assert_eq!(mem_to_string(&Mem::Int(7)), "7");
        assert_eq!(mem_to_string(&Mem::Real(1.5)), "1.5");
        assert_eq!(mem_to_string(&Mem::Null), "");
        assert_eq!(mem_to_string(&Mem::Blob(Arc::from(b"x".as_slice()))), "");
    }

    #[test]
    fn test_mem_to_string_non_text_key_in_json_object() {
        // Non-text, non-null keys go through `mem_to_string` (int/real branches).
        let args = [
            Mem::Int(1),
            Mem::Text(Arc::from("a")),
            Mem::Real(2.5),
            Mem::Text(Arc::from("b")),
        ];
        let result = func_json_object(&args).unwrap();
        assert_eq!(result, Mem::Text(Arc::from("{\"1\":\"a\",\"2.5\":\"b\"}")));
    }

    #[test]
    fn test_mem_to_json_real_and_blob_and_other() {
        let args = [
            Mem::Real(3.5),
            Mem::Blob(Arc::from(b"hi".as_slice())),
            Mem::Text(Arc::from("42")), // parses as JSON number
            Mem::ZeroBlob(4),           // falls into the catch-all -> JsonValue::Null
        ];
        let result = func_json_array(&args);
        assert_eq!(result, Mem::Text(Arc::from("[3.5,\"hi\",42,null]")));
    }

    #[test]
    fn test_json_extract_result_variants() {
        let json = Mem::Text(Arc::from(
            "{\"n\": null, \"t\": true, \"false_val\": false, \"f\": 1.5, \"s\": \"hi\", \"o\": {\"x\": 1}, \"arr\": [1, 2]}",
        ));

        assert_eq!(
            func_json_extract(&[json.clone(), Mem::Text(Arc::from("$.n"))]).unwrap(),
            Mem::Null
        );
        assert_eq!(
            func_json_extract(&[json.clone(), Mem::Text(Arc::from("$.t"))]).unwrap(),
            Mem::Int(1)
        );
        assert_eq!(
            func_json_extract(&[json.clone(), Mem::Text(Arc::from("$.false_val"))]).unwrap(),
            Mem::Int(0)
        );
        assert_eq!(
            func_json_extract(&[json.clone(), Mem::Text(Arc::from("$.f"))]).unwrap(),
            Mem::Real(1.5)
        );
        assert_eq!(
            func_json_extract(&[json.clone(), Mem::Text(Arc::from("$.s"))]).unwrap(),
            Mem::Text(Arc::from("hi"))
        );
        assert_eq!(
            func_json_extract(&[json.clone(), Mem::Text(Arc::from("$.o"))]).unwrap(),
            Mem::Text(Arc::from("{\"x\":1}"))
        );
        assert_eq!(
            func_json_extract(&[json, Mem::Text(Arc::from("$.arr"))]).unwrap(),
            Mem::Text(Arc::from("[1,2]"))
        );
    }

    #[test]
    fn test_json_valid_empty_args_errors() {
        let err = func_json_valid(&[]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "wrong number of arguments to function json_valid()"
        );
    }

    #[test]
    fn test_json_valid_null_and_other_types() {
        assert_eq!(func_json_valid(&[Mem::Null]).unwrap(), Mem::Null);
        assert_eq!(func_json_valid(&[Mem::Int(5)]).unwrap(), Mem::Int(0));
    }

    #[test]
    fn test_json_extract_wrong_arg_count() {
        let json = Mem::Text(Arc::from("{}"));
        let err = func_json_extract(&[json]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "wrong number of arguments to function json_extract()"
        );
    }

    #[test]
    fn test_json_extract_null_and_non_text_first_arg() {
        let path = Mem::Text(Arc::from("$.a"));
        assert_eq!(
            func_json_extract(&[Mem::Null, path.clone()]).unwrap(),
            Mem::Null
        );
        assert_eq!(func_json_extract(&[Mem::Int(1), path]).unwrap(), Mem::Null);
    }

    #[test]
    fn test_json_extract_non_text_path() {
        let json = Mem::Text(Arc::from("{\"a\":1}"));
        assert_eq!(func_json_extract(&[json, Mem::Int(1)]).unwrap(), Mem::Null);
    }

    #[test]
    fn test_json_extract_invalid_json_returns_null() {
        let json = Mem::Text(Arc::from("not json"));
        let path = Mem::Text(Arc::from("$.a"));
        assert_eq!(func_json_extract(&[json, path]).unwrap(), Mem::Null);
    }

    #[test]
    fn test_json_extract_array_out_of_range_and_non_numeric_index() {
        let json = Mem::Text(Arc::from("{\"c\": [1, 2]}"));
        let out_of_range = Mem::Text(Arc::from("$.c.10"));
        assert_eq!(
            func_json_extract(&[json.clone(), out_of_range]).unwrap(),
            Mem::Null
        );
        let non_numeric = Mem::Text(Arc::from("$.c.foo"));
        assert_eq!(func_json_extract(&[json, non_numeric]).unwrap(), Mem::Null);
    }

    #[test]
    fn test_json_extract_path_into_scalar_returns_null() {
        // "a" resolves to a number; indexing further into it hits the catch-all.
        let json = Mem::Text(Arc::from("{\"a\": 42}"));
        let path = Mem::Text(Arc::from("$.a.b"));
        assert_eq!(func_json_extract(&[json, path]).unwrap(), Mem::Null);
    }

    #[test]
    fn test_json_object_odd_args_errors() {
        let err = func_json_object(&[Mem::Text(Arc::from("a"))]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "wrong number of arguments to function json_object()"
        );
    }

    #[test]
    fn test_json_object_null_key_errors() {
        let err = func_json_object(&[Mem::Null, Mem::Int(1)]).unwrap_err();
        assert_eq!(err.to_string(), "wrong number of arguments to function json_object label cannot be null()");
    }
}
