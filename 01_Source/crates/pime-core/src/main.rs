use pime_core::{detect_language, encode_x86_64, predict_local, validate_hex, LANGUAGES};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn dispatch(v: &Value) -> Value {
    match v.get("op").and_then(Value::as_str).unwrap_or("") {
        "health" => json!({"ok":true,"name":"pime-core","version":"0.1.0","languages":LANGUAGES}),
        "predict" => {
            let language = v.get("language").and_then(Value::as_str).unwrap_or("");
            let prefix = v.get("prefix").and_then(Value::as_str).unwrap_or("");
            let limit = v.get("limit").and_then(Value::as_u64).unwrap_or(5) as usize;
            json!({"ok":true,"seq":v.get("seq"),"candidates":predict_local(language,prefix,limit)})
        },
        "detect" => json!({"ok":true,"decision": detect_language(
            v.get("filename").and_then(Value::as_str).unwrap_or(""),
            v.get("text").and_then(Value::as_str).unwrap_or(""),
            v.get("locked").and_then(Value::as_str))}),
        "encode" => match encode_x86_64(v.get("instruction").and_then(Value::as_str).unwrap_or("")) {
            Ok(hex) => json!({"ok":true,"hex":hex}),
            Err(e) => json!({"ok":false,"error":e})
        },
        "validate_hex" => match validate_hex(v.get("hex").and_then(Value::as_str).unwrap_or("")) {
            Ok(bytes) => json!({"ok":true,"byte_count":bytes.len()}),
            Err(e) => json!({"ok":false,"error":e})
        },
        _ => json!({"ok":false,"error":"UNKNOWN_OPERATION"}),
    }
}

fn main() {
    for line in io::stdin().lock().lines() {
        let line = match line { Ok(x) => x, Err(_) => break };
        let answer = if line.len() > 65536 {json!({"ok":false,"error":"PAYLOAD_TOO_LARGE"})}
            else { match serde_json::from_str::<Value>(&line) {
                Ok(input) => dispatch(&input),
                Err(_) => json!({"ok":false,"error":"INVALID_JSON"}),
            }};
        let mut out = io::stdout().lock();
        if writeln!(out, "{}", answer).is_err() || out.flush().is_err() { break; }
    }
}
