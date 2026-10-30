//! Small strict JSON parser / serializer (RFC 8259) with explicit size and depth limits.
//!
//! Strictness: no comments, no trailing commas, no leading zeros, no raw control characters in strings,
//! duplicate object keys are rejected (a parser differential is a classic smuggling vector), numbers must be
//! finite. A lone surrogate escape (`\ud800`) is replaced by U+FFFD because a Rust `String` cannot hold it.

use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    /// Insertion-ordered object (keys are unique).
    Obj(Vec<(String, Json)>),
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_depth: usize,
}

impl Limits {
    pub const DEFAULT: Limits = Limits {
        max_bytes: 1 << 20,
        max_depth: 64,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonErrorKind {
    TooLarge,
    TooDeep,
    Syntax,
    DuplicateKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonError {
    pub kind: JsonErrorKind,
    pub pos: usize,
}

impl std::fmt::Display for JsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} at byte {}", self.kind, self.pos)
    }
}

impl std::error::Error for JsonError {}

pub fn parse(input: &str, limits: Limits) -> Result<Json, JsonError> {
    if input.len() > limits.max_bytes {
        return Err(JsonError {
            kind: JsonErrorKind::TooLarge,
            pos: 0,
        });
    }
    let mut p = Parser {
        b: input.as_bytes(),
        s: input,
        i: 0,
        limits,
    };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.err(JsonErrorKind::Syntax));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    s: &'a str,
    i: usize,
    limits: Limits,
}

impl Parser<'_> {
    fn err(&self, kind: JsonErrorKind) -> JsonError {
        JsonError { kind, pos: self.i }
    }

    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.b[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if self.i >= self.b.len() {
            return Err(self.err(JsonErrorKind::Syntax));
        }
        let c = self.b[self.i];
        match c {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => Ok(Json::Str(self.string()?)),
            b't' | b'f' | b'n' => {
                if self.eat("true") {
                    Ok(Json::Bool(true))
                } else if self.eat("false") {
                    Ok(Json::Bool(false))
                } else if self.eat("null") {
                    Ok(Json::Null)
                } else {
                    Err(self.err(JsonErrorKind::Syntax))
                }
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(self.err(JsonErrorKind::Syntax)),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth + 1 > self.limits.max_depth {
            return Err(self.err(JsonErrorKind::TooDeep));
        }
        self.i += 1;
        let mut out: Vec<(String, Json)> = Vec::new();
        self.ws();
        if self.i < self.b.len() && self.b[self.i] == b'}' {
            self.i += 1;
            return Ok(Json::Obj(out));
        }
        loop {
            self.ws();
            if self.i >= self.b.len() || self.b[self.i] != b'"' {
                return Err(self.err(JsonErrorKind::Syntax));
            }
            let key = self.string()?;
            if out.iter().any(|(k, _)| *k == key) {
                return Err(self.err(JsonErrorKind::DuplicateKey));
            }
            self.ws();
            if self.i >= self.b.len() || self.b[self.i] != b':' {
                return Err(self.err(JsonErrorKind::Syntax));
            }
            self.i += 1;
            self.ws();
            let v = self.value(depth + 1)?;
            out.push((key, v));
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Json::Obj(out));
                }
                _ => return Err(self.err(JsonErrorKind::Syntax)),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth + 1 > self.limits.max_depth {
            return Err(self.err(JsonErrorKind::TooDeep));
        }
        self.i += 1;
        let mut out = Vec::new();
        self.ws();
        if self.i < self.b.len() && self.b[self.i] == b']' {
            self.i += 1;
            return Ok(Json::Arr(out));
        }
        loop {
            self.ws();
            out.push(self.value(depth + 1)?);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Arr(out));
                }
                _ => return Err(self.err(JsonErrorKind::Syntax)),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        if self.i + 4 > self.b.len() {
            return Err(self.err(JsonErrorKind::Syntax));
        }
        let mut v = 0u32;
        for k in 0..4 {
            let d = (self.b[self.i + k] as char)
                .to_digit(16)
                .ok_or_else(|| self.err(JsonErrorKind::Syntax))?;
            v = v * 16 + d;
        }
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.i += 1; // opening quote
        let mut out = String::new();
        loop {
            let Some(&c) = self.b.get(self.i) else {
                return Err(self.err(JsonErrorKind::Syntax));
            };
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(out);
                }
                0..=0x1f => return Err(self.err(JsonErrorKind::Syntax)),
                b'\\' => {
                    self.i += 1;
                    let Some(&e) = self.b.get(self.i) else {
                        return Err(self.err(JsonErrorKind::Syntax));
                    };
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            if (0xD800..0xDC00).contains(&hi) {
                                if self.b[self.i..].starts_with(b"\\u") {
                                    let save = self.i;
                                    self.i += 2;
                                    let lo = self.hex4()?;
                                    if (0xDC00..0xE000).contains(&lo) {
                                        let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                                        out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                                    } else {
                                        // high surrogate without partner; re-read the second escape normally
                                        out.push('\u{fffd}');
                                        self.i = save;
                                    }
                                } else {
                                    out.push('\u{fffd}');
                                }
                            } else if (0xDC00..0xE000).contains(&hi) {
                                out.push('\u{fffd}');
                            } else {
                                out.push(char::from_u32(hi).unwrap_or('\u{fffd}'));
                            }
                        }
                        _ => return Err(self.err(JsonErrorKind::Syntax)),
                    }
                }
                _ => {
                    // copy one UTF-8 scalar (the source is a valid &str)
                    let start = self.i;
                    let ch = self.s[start..]
                        .chars()
                        .next()
                        .ok_or_else(|| self.err(JsonErrorKind::Syntax))?;
                    out.push(ch);
                    self.i += ch.len_utf8();
                }
            }
        }
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.i;
        if self.b[self.i] == b'-' {
            self.i += 1;
        }
        match self.b.get(self.i) {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                    self.i += 1;
                }
            }
            _ => return Err(self.err(JsonErrorKind::Syntax)),
        }
        if self.b.get(self.i) == Some(&b'.') {
            self.i += 1;
            let d = self.i;
            while matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                self.i += 1;
            }
            if self.i == d {
                return Err(self.err(JsonErrorKind::Syntax));
            }
        }
        if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            let d = self.i;
            while matches!(self.b.get(self.i), Some(b'0'..=b'9')) {
                self.i += 1;
            }
            if self.i == d {
                return Err(self.err(JsonErrorKind::Syntax));
            }
        }
        let v: f64 = self.s[start..self.i]
            .parse()
            .map_err(|_| self.err(JsonErrorKind::Syntax))?;
        if !v.is_finite() {
            return Err(self.err(JsonErrorKind::Syntax));
        }
        Ok(Json::Num(v))
    }
}

impl Json {
    pub fn obj(pairs: Vec<(&str, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn str(s: &str) -> Json {
        Json::Str(s.to_string())
    }

    pub fn int(n: i64) -> Json {
        Json::Num(n as f64)
    }

    pub fn strs<S: AsRef<str>>(items: &[S]) -> Json {
        Json::Arr(
            items
                .iter()
                .map(|s| Json::Str(s.as_ref().to_string()))
                .collect(),
        )
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(o) => o.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Vec<(String, Json)>> {
        match self {
            Json::Obj(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// `Number.isInteger` semantics (5.0 is an integer).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0 => {
                Some(*n as i64)
            }
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    pub fn to_json_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if !n.is_finite() {
                    out.push_str("null");
                } else if n.fract() == 0.0 && n.abs() < 1e15 {
                    let _ = write!(out, "{}", *n as i64);
                } else {
                    let _ = write!(out, "{n}");
                }
            }
            Json::Str(s) => write_str(s, out),
            Json::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Json::Obj(o) => {
                out.push('{');
                for (i, (k, v)) in o.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Json, JsonError> {
        parse(s, Limits::DEFAULT)
    }

    #[test]
    fn parses_and_round_trips() {
        let v = p(r#"{"a":[1,2.5,-3e2,true,null,"x\n\u00e9\ud83d\ude00"],"b":{}}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_array().unwrap().len(), 6);
        assert_eq!(p(&v.to_json_string()).unwrap(), v);
        assert_eq!(
            v.get("a").unwrap().as_array().unwrap()[5].as_str().unwrap(),
            "x\n\u{e9}\u{1F600}"
        );
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\":1,}",
            "01",
            "1.",
            "+1",
            "NaN",
            "'a'",
            "{\"a\":1}{",
            "\"\u{1}\"",
            "{\"a\" 1}",
            "[1 2]",
            "\"\\x\"",
            "{\"a\":1,\"a\":2}",
            "// c\n1",
            "1e999",
        ] {
            assert!(p(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn enforces_limits() {
        let deep = format!("{}{}", "[".repeat(70), "]".repeat(70));
        assert_eq!(p(&deep).unwrap_err().kind, JsonErrorKind::TooDeep);
        let ok = format!("{}{}", "[".repeat(60), "]".repeat(60));
        assert!(p(&ok).is_ok());
        let big = Limits {
            max_bytes: 8,
            max_depth: 4,
        };
        assert_eq!(
            parse("[1,2,3,4,5]", big).unwrap_err().kind,
            JsonErrorKind::TooLarge
        );
    }

    #[test]
    fn lone_surrogate_becomes_replacement_character() {
        assert_eq!(p(r#""\ud800""#).unwrap(), Json::Str("\u{fffd}".into()));
        assert_eq!(p(r#""\udc00x""#).unwrap(), Json::Str("\u{fffd}x".into()));
    }

    #[test]
    fn serializer_escapes_control_characters() {
        assert_eq!(
            Json::str("a\"\\\n\u{1}").to_json_string(),
            r#""a\"\\\n\u0001""#
        );
        assert_eq!(Json::int(5).to_json_string(), "5");
        assert_eq!(Json::Num(0.5).to_json_string(), "0.5");
    }
}
