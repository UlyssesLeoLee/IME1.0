//! Privacy-preserving diagnostics (FR-050, NFR-301, DD 12.2), port of `core/diagnostics.js`. Only whitelisted
//! metadata is recorded; every value must be a short token-like string or a number, so source code, prompts, paths
//! and keys cannot slip in. The events live in a bounded ring; the anonymised session id is the first 12 hex digits of
//! the SHA-256 of a seed, which cannot be reversed.
//!
//! Differences from the reference, all on the safe side: the `timestamp` must be a number (the reference lets ANY
//! value through unfiltered, so a caller passing a path as a timestamp would have it recorded; here anything that is
//! not a number is replaced by the clock), and keys such as `__proto__` or `constructor` are ordinary names in the
//! summary (the reference mangles or drops them because it counts in plain objects).

use crate::json::Json;
use crate::jsval::{js_order, round, to_compact_string, to_js_string, truthy};
use crate::sha256::sha256_hex;
use std::collections::VecDeque;

/// The recorded fields, in output order. (The design's whitelist, DD 12.2, lists seven of these; `error_detail` and
/// `provider_id` are the two additions of the reference implementation.)
pub const ALLOWED: [&str; 9] = [
    "timestamp",
    "component",
    "error_code",
    "error_detail",
    "elapsed_ms",
    "request_type",
    "anonymized_session",
    "provider_id",
    "counters",
];

/// The ring size of the reference.
pub const DEFAULT_MAX: usize = 500;

/// `/^[A-Za-z0-9_.:-]{0,64}$/`
fn is_token(s: &str) -> bool {
    s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
}

fn finite(v: &Json) -> bool {
    matches!(v, Json::Num(n) if n.is_finite())
}

/// `Object.entries(value || {})` as far as counters care: objects by key, arrays by index, everything else is empty
/// (a string yields one-character strings, which are not numbers and are dropped anyway).
fn counter_entries(v: &Json) -> Vec<(String, &Json)> {
    match v {
        Json::Obj(o) => o.iter().map(|(k, x)| (k.clone(), x)).collect(),
        Json::Arr(a) => a
            .iter()
            .enumerate()
            .map(|(i, x)| (i.to_string(), x))
            .collect(),
        _ => Vec::new(),
    }
}

/// Keeps the whitelisted fields of `event` and filters their values. `session` is the anonymised session id and
/// `now_ms` the clock for events without a numeric timestamp.
pub fn sanitize(event: &Json, session: &str, now_ms: f64) -> Json {
    let mut out: Vec<(String, Json)> = Vec::new();
    for key in ALLOWED {
        let present = event.get(key);
        let value = match key {
            "anonymized_session" => Json::str(session),
            "timestamp" => match present {
                Some(Json::Num(n)) if n.is_finite() => Json::Num(*n),
                _ => Json::Num(now_ms),
            },
            _ if present.is_none() => continue,
            "counters" => {
                let kept: Vec<(String, Json)> = present
                    .map(counter_entries)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(k, v)| is_token(k) && finite(v))
                    .map(|(k, v)| (k, v.clone()))
                    .collect();
                Json::Obj(js_order(kept))
            }
            _ => match present {
                Some(Json::Num(n)) => Json::Num(if n.is_finite() { round(*n) } else { 0.0 }),
                Some(Json::Str(s)) if is_token(s) => Json::str(s),
                _ => Json::str("[filtered]"),
            },
        };
        out.push((key.to_string(), value));
    }
    Json::Obj(out)
}

pub struct Diagnostics {
    session: String,
    max: usize,
    events: VecDeque<Json>,
}

impl Diagnostics {
    /// `seed` is hashed; the session id is not reversible.
    pub fn new(seed: &str, max: usize) -> Diagnostics {
        Diagnostics {
            session: sha256_hex(seed)[..12].to_string(),
            max,
            events: VecDeque::new(),
        }
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn events(&self) -> impl Iterator<Item = &Json> {
        self.events.iter()
    }

    /// Records the sanitised event and returns it. The oldest events fall out when the ring is full.
    pub fn record(&mut self, event: &Json, now_ms: f64) -> Json {
        let clean = sanitize(event, &self.session, now_ms);
        self.events.push_back(clean.clone());
        while self.events.len() > self.max {
            self.events.pop_front();
        }
        clean
    }

    /// `{total, byCode, lastByComponent}`.
    pub fn summary(&self) -> Json {
        let mut by_code: Vec<(String, i64)> = Vec::new();
        let mut last: Vec<(String, Json)> = Vec::new();
        for event in &self.events {
            let code = match event.get("error_code") {
                Some(c) if truthy(c) => c.clone(),
                _ => Json::str("OK"),
            };
            let code_key = to_js_string(&code);
            match by_code.iter_mut().find(|(k, _)| *k == code_key) {
                Some(slot) => slot.1 += 1,
                None => by_code.push((code_key, 1)),
            }
            let component = match event.get("component") {
                Some(c) if truthy(c) => to_js_string(c),
                _ => "unknown".to_string(),
            };
            let elapsed = match event.get("elapsed_ms") {
                None | Some(Json::Null) => Json::Null,
                Some(v) => v.clone(),
            };
            let entry = Json::obj(vec![("error_code", code), ("elapsed_ms", elapsed)]);
            match last.iter_mut().find(|(k, _)| *k == component) {
                Some(slot) => slot.1 = entry,
                None => last.push((component, entry)),
            }
        }
        Json::obj(vec![
            ("total", Json::int(self.events.len() as i64)),
            (
                "byCode",
                Json::Obj(js_order(
                    by_code
                        .into_iter()
                        .map(|(k, n)| (k, Json::int(n)))
                        .collect(),
                )),
            ),
            ("lastByComponent", Json::Obj(js_order(last))),
        ])
    }

    /// One JSON line per event, oldest first.
    pub fn to_text(&self) -> String {
        self.events
            .iter()
            .map(to_compact_string)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Limits, parse};

    fn j(s: &str) -> Json {
        parse(s, Limits::DEFAULT).unwrap()
    }

    #[test]
    fn ut_071_only_whitelisted_token_like_metadata_is_recorded() {
        let mut d = Diagnostics::new("seed", DEFAULT_MAX);
        let clean = d.record(
            &j(r#"{"component":"ai","error_code":"AI_TIMEOUT","elapsed_ms":12.6,"request_type":"f1Full","provider_id":"openai:api.openai.com",
                   "counters":{"requests":2,"bad key":3,"s":"x"},"input_text":"fn secret() {}","api_key":"sk-live-123","prompt":"p","document_uri":"file:///x"}"#),
            5000.0,
        );
        assert_eq!(
            clean.to_json_string(),
            format!(
                r#"{{"timestamp":5000,"component":"ai","error_code":"AI_TIMEOUT","elapsed_ms":13,"request_type":"f1Full","anonymized_session":"{}","provider_id":"openai:api.openai.com","counters":{{"requests":2}}}}"#,
                d.session()
            )
        );
        let text = d.to_text();
        for leak in [
            "secret",
            "sk-live",
            "file:///",
            "input_text",
            "api_key",
            "bad key",
        ] {
            assert!(!text.contains(leak), "{leak}");
        }
        // values that are not short tokens are filtered, never copied
        let clean = d.record(
            &j(r#"{"component":"C:\\Users\\me\\x.rs","error_code":"a b","error_detail":"é","request_type":null,"provider_id":["x"],"elapsed_ms":"too slow"}"#),
            1.0,
        );
        for key in [
            "component",
            "error_code",
            "error_detail",
            "request_type",
            "provider_id",
            "elapsed_ms",
        ] {
            assert_eq!(
                clean.get(key).and_then(Json::as_str),
                Some("[filtered]"),
                "{key}"
            );
        }
        assert!(is_token(&"a".repeat(64)) && !is_token(&"a".repeat(65)) && is_token(""));
        assert!(
            !is_token("a\n"),
            "the end anchor does not match before a trailing newline"
        );
    }

    #[test]
    fn a_timestamp_that_is_not_a_number_never_reaches_the_record() {
        let mut d = Diagnostics::new("s", 10);
        let clean = d.record(
            &j(r#"{"timestamp":"/home/me/secret.rs","component":"a"}"#),
            77.0,
        );
        assert_eq!(clean.get("timestamp"), Some(&Json::Num(77.0)));
        let clean = d.record(&j(r#"{"timestamp":null}"#), 78.0);
        assert_eq!(clean.get("timestamp"), Some(&Json::Num(78.0)));
        let clean = d.record(&j(r#"{"timestamp":0}"#), 79.0);
        assert_eq!(
            clean.get("timestamp"),
            Some(&Json::Num(0.0)),
            "zero is a timestamp"
        );
        assert!(!d.to_text().contains("secret"));
    }

    #[test]
    fn the_session_id_is_an_anonymised_hash_of_the_seed() {
        let a = Diagnostics::new("seed-1", 5);
        assert_eq!(a.session().len(), 12);
        assert_eq!(a.session(), &sha256_hex("seed-1")[..12]);
        assert_ne!(a.session(), Diagnostics::new("seed-2", 5).session());
        let mut d = Diagnostics::new("seed-1", 5);
        let clean = d.record(&j(r#"{"anonymized_session":"evil"}"#), 1.0);
        assert_eq!(
            clean.get("anonymized_session").and_then(Json::as_str),
            Some(a.session())
        );
    }

    #[test]
    fn the_ring_keeps_the_newest_events_and_the_summary_counts_what_is_left() {
        let mut d = Diagnostics::new("s", 3);
        for i in 0..10 {
            d.record(
                &j(&format!(r#"{{"component":"x","error_code":"C{}"}}"#, i % 2)),
                f64::from(i),
            );
        }
        assert_eq!(d.events().count(), 3);
        assert_eq!(d.to_text().lines().count(), 3);
        let s = d.summary();
        assert_eq!(s.get("total").and_then(Json::as_i64), Some(3));
        assert_eq!(
            s.get("byCode").unwrap().to_json_string(),
            r#"{"C1":2,"C0":1}"#
        );
        let mut none = Diagnostics::new("s", 0);
        none.record(&j("{}"), 1.0);
        assert_eq!(none.events().count(), 0, "a ring of zero keeps nothing");
    }

    #[test]
    fn the_summary_follows_the_reference_rules() {
        let mut d = Diagnostics::new("s", DEFAULT_MAX);
        for e in [
            r#"{"component":"ai","error_code":"AI_TIMEOUT","elapsed_ms":10}"#,
            r#"{"component":"ai","elapsed_ms":7}"#,
            r#"{"error_code":404}"#,
            r#"{"component":"","error_code":""}"#,
            r#"{"component":"edit","error_code":0,"elapsed_ms":"too slow"}"#,
            r#"{"component":"__proto__","error_code":"constructor"}"#,
        ] {
            d.record(&j(e), 1.0);
        }
        let s = d.summary();
        assert_eq!(
            s.get("byCode").unwrap().to_json_string(),
            r#"{"404":1,"AI_TIMEOUT":1,"OK":3,"constructor":1}"#,
            "integer-like keys first; an empty or zero code counts as OK"
        );
        assert_eq!(
            s.get("lastByComponent").unwrap().to_json_string(),
            r#"{"ai":{"error_code":"OK","elapsed_ms":7},"unknown":{"error_code":"OK","elapsed_ms":null},"edit":{"error_code":"OK","elapsed_ms":"[filtered]"},"__proto__":{"error_code":"constructor","elapsed_ms":null}}"#
        );
    }

    #[test]
    fn counters_read_like_object_entries() {
        let s = |text: &str| {
            sanitize(&j(text), "sess", 1.0)
                .get("counters")
                .map(Json::to_json_string)
        };
        assert_eq!(
            s(r#"{"counters":[1,"x",null,4]}"#).as_deref(),
            Some(r#"{"0":1,"3":4}"#)
        );
        assert_eq!(
            s(r#"{"counters":{"10":1,"2":2,"a":3}}"#).as_deref(),
            Some(r#"{"2":2,"10":1,"a":3}"#)
        );
        for ignored in [
            "\"abc\"",
            "5",
            "null",
            "true",
            "{\"a\":{\"b\":1}}",
            "{\"a\":null}",
        ] {
            assert_eq!(
                s(&format!(r#"{{"counters":{ignored}}}"#)).as_deref(),
                Some("{}"),
                "{ignored}"
            );
        }
        assert_eq!(s("{}"), None, "no counters, no field");
    }
}
