//! JavaScript value semantics that the ported bridge logic depends on: `String(value)` (so error messages and
//! interface strings read the same as the reference), truthiness, `Math.round`, property order of objects,
//! `Date.prototype.toISOString` and `JSON.stringify(value, null, 2)`.
//!
//! These are small, exact re-implementations; the parity fixtures exercise every one of them through the modules
//! that use them (`settings`, `i18n`, `diag`, `uimodel`).

use crate::json::Json;

/// ECMAScript `Number::toString(x)` (radix 10): shortest round-trip digits, exponent form from 1e21 and below 1e-6.
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".to_string();
    }
    if n == 0.0 {
        return "0".to_string();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let sign = if n < 0.0 { "-" } else { "" };
    // `{:e}` prints the shortest digits that round-trip, as `d.ddde<exp>`
    let sci = format!("{:e}", n.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    // value = 0.<digits> * 10^point
    let point = exponent.parse::<i32>().unwrap_or(0) + 1;
    let body = if k <= point && point <= 21 {
        format!("{digits}{}", "0".repeat((point - k) as usize))
    } else if 0 < point && point <= 21 {
        format!(
            "{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let e = point - 1;
        let tail = format!("e{}{}", if e < 0 { '-' } else { '+' }, e.abs());
        if k == 1 {
            format!("{digits}{tail}")
        } else {
            format!("{}.{}{tail}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

/// `String(value)` for a value that came out of JSON: arrays join their elements with `,` (null as the empty
/// string), objects are `[object Object]`.
pub fn to_js_string(v: &Json) -> String {
    match v {
        Json::Null => "null".to_string(),
        Json::Bool(b) => b.to_string(),
        Json::Num(n) => number_to_string(*n),
        Json::Str(s) => s.clone(),
        Json::Arr(a) => a
            .iter()
            .map(|x| match x {
                Json::Null => String::new(),
                other => to_js_string(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        Json::Obj(_) => "[object Object]".to_string(),
    }
}

/// JavaScript truthiness (`if (value)`, `filter(Boolean)`).
pub fn truthy(v: &Json) -> bool {
    match v {
        Json::Null => false,
        Json::Bool(b) => *b,
        Json::Num(n) => *n != 0.0 && !n.is_nan(),
        Json::Str(s) => !s.is_empty(),
        Json::Arr(_) | Json::Obj(_) => true,
    }
}

/// `Math.round`: halves round toward +infinity (`Math.round(-2.5)` is `-2`), unlike `f64::round`.
pub fn round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

/// The array-index value of a canonical numeric key (`"0"`, `"17"`), if it is one.
pub fn array_index(key: &str) -> Option<u64> {
    if key.is_empty() || key.len() > 10 || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    if !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<u64>().ok().filter(|n| *n < 4_294_967_295)
}

/// JavaScript property order: integer-like keys ascending first, then the other keys in insertion order.
pub fn js_order<V>(obj: Vec<(String, V)>) -> Vec<(String, V)> {
    if !obj.iter().any(|(k, _)| array_index(k).is_some()) {
        return obj;
    }
    let (mut ints, rest): (Vec<_>, Vec<_>) =
        obj.into_iter().partition(|(k, _)| array_index(k).is_some());
    ints.sort_by_key(|(k, _)| array_index(k));
    ints.extend(rest);
    ints
}

/// `new Date(ms).toISOString()`; `None` where JavaScript throws a `RangeError` (not finite, or beyond +-8.64e15).
pub fn iso_date(ms: f64) -> Option<String> {
    if !ms.is_finite() || ms.abs() > 8.64e15 {
        return None;
    }
    let ms = ms.trunc() as i64;
    let days = ms.div_euclid(86_400_000);
    let in_day = ms.rem_euclid(86_400_000);
    // civil date from the day count (proleptic Gregorian calendar)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let year_text = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else {
        format!("{}{:06}", if year < 0 { '-' } else { '+' }, year.abs())
    };
    Some(format!(
        "{year_text}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        in_day / 3_600_000,
        in_day / 60_000 % 60,
        in_day / 1000 % 60,
        in_day % 1000
    ))
}

/// `JSON.stringify(value)`: compact, with numbers printed like JavaScript (the plain `Json::to_json_string` prints
/// exponent-sized numbers in positional form).
pub fn to_compact_string(v: &Json) -> String {
    let mut out = String::new();
    stringify(v, None, 0, &mut out);
    out
}

/// `JSON.stringify(value, null, 2)`: two-space indentation, `"key": value`, empty containers as `{}` / `[]`.
pub fn to_pretty_string(v: &Json) -> String {
    let mut out = String::new();
    stringify(v, Some(2), 0, &mut out);
    out
}

fn newline(indent: Option<usize>, depth: usize, out: &mut String) {
    if let Some(width) = indent {
        out.push('\n');
        out.push_str(&" ".repeat(width * depth));
    }
}

fn stringify(v: &Json, indent: Option<usize>, depth: usize, out: &mut String) {
    match v {
        Json::Num(n) => out.push_str(&if n.is_finite() {
            number_to_string(*n)
        } else {
            "null".to_string()
        }),
        Json::Arr(a) if !a.is_empty() => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(indent, depth + 1, out);
                stringify(x, indent, depth + 1, out);
            }
            newline(indent, depth, out);
            out.push(']');
        }
        Json::Obj(o) if !o.is_empty() => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(indent, depth + 1, out);
                out.push_str(&Json::str(k).to_json_string());
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                stringify(x, indent, depth + 1, out);
            }
            newline(indent, depth, out);
            out.push('}');
        }
        other => out.push_str(&other.to_json_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Limits, parse};

    #[test]
    fn numbers_print_like_javascript() {
        for (n, want) in [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.5, "-1.5"),
            (100.0, "100"),
            (123_456_789.0, "123456789"),
            (0.1, "0.1"),
            (0.000_001, "0.000001"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (1e21, "1e+21"),
            (1.234_567_890_123_456_8e20, "123456789012345680000"),
            (1e20, "100000000000000000000"),
            (1.5e300, "1.5e+300"),
            (9_007_199_254_740_992.0, "9007199254740992"),
            (f64::MAX, "1.7976931348623157e+308"),
            (5e-324, "5e-324"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (f64::NAN, "NaN"),
        ] {
            assert_eq!(number_to_string(n), want, "{n:?}");
        }
    }

    #[test]
    fn values_stringify_truthify_and_round_like_javascript() {
        let j = |s: &str| parse(s, Limits::DEFAULT).unwrap();
        for (text, want) in [
            ("null", "null"),
            ("true", "true"),
            ("1e21", "1e+21"),
            ("\"x\"", "x"),
            ("[1,[2,null],\"a\"]", "1,2,,a"),
            ("[]", ""),
            ("{\"a\":1}", "[object Object]"),
        ] {
            assert_eq!(to_js_string(&j(text)), want, "{text}");
        }
        for (text, want) in [
            ("null", false),
            ("0", false),
            ("0.0", false),
            ("\"\"", false),
            ("false", false),
            ("1", true),
            ("\"0\"", true),
            ("[]", true),
            ("{}", true),
        ] {
            assert_eq!(truthy(&j(text)), want, "{text}");
        }
        for (x, want) in [
            (2.5, 3.0),
            (-2.5, -2.0),
            (-0.5, 0.0),
            (0.5, 1.0),
            (0.49999999999999994, 0.0),
            (-0.49999999999999994, 0.0),
            (1.4, 1.0),
            (-1.6, -2.0),
            (4_503_599_627_370_497.0, 4_503_599_627_370_497.0),
        ] {
            assert_eq!(round(x), want, "{x:?}");
        }
    }

    #[test]
    fn integer_like_keys_come_first_in_ascending_order() {
        let obj = vec![
            ("b".to_string(), Json::Null),
            ("10".to_string(), Json::Null),
            ("01".to_string(), Json::Null),
            ("2".to_string(), Json::Null),
            ("4294967295".to_string(), Json::Null),
        ];
        let keys: Vec<String> = js_order(obj).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["2", "10", "b", "01", "4294967295"]);
    }

    #[test]
    fn dates_format_like_to_iso_string() {
        for (ms, want) in [
            (0.0, "1970-01-01T00:00:00.000Z"),
            (-1.0, "1969-12-31T23:59:59.999Z"),
            (1_793_318_400_000.0, "2026-10-30T00:00:00.000Z"),
            (1_709_251_199_999.0, "2024-02-29T23:59:59.999Z"),
            (4_107_542_400_000.0, "2100-03-01T00:00:00.000Z"),
            (253_402_300_799_999.0, "9999-12-31T23:59:59.999Z"),
            (253_402_300_800_000.0, "+010000-01-01T00:00:00.000Z"),
            (-62_198_755_200_000.0, "-000001-01-01T00:00:00.000Z"),
            (-62_167_219_200_000.0, "0000-01-01T00:00:00.000Z"),
            (8.64e15, "+275760-09-13T00:00:00.000Z"),
            (-8.64e15, "-271821-04-20T00:00:00.000Z"),
            (1.9, "1970-01-01T00:00:00.001Z"),
        ] {
            assert_eq!(iso_date(ms).as_deref(), Some(want), "{ms}");
        }
        assert_eq!(iso_date(8.64e15 + 1.0), None);
        assert_eq!(iso_date(f64::NAN), None);
    }

    #[test]
    fn pretty_printing_matches_json_stringify_with_two_spaces() {
        let v = parse(
            r#"{"a":[],"b":{},"c":[1,{"d":"x\ny","e":null}],"f":1e-7,"g":-0.5,"h":"é\u0001"}"#,
            Limits::DEFAULT,
        )
        .unwrap();
        assert_eq!(
            to_pretty_string(&v),
            "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1,\n    {\n      \"d\": \"x\\ny\",\n      \"e\": null\n    }\n  ],\n  \"f\": 1e-7,\n  \"g\": -0.5,\n  \"h\": \"é\\u0001\"\n}"
        );
        assert_eq!(to_pretty_string(&Json::Null), "null");
        assert_eq!(
            to_compact_string(&v),
            "{\"a\":[],\"b\":{},\"c\":[1,{\"d\":\"x\\ny\",\"e\":null}],\"f\":1e-7,\"g\":-0.5,\"h\":\"é\\u0001\"}"
        );
    }
}
