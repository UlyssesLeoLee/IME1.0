//! Shared helpers of the protocol 0.3 integration tests: a core with an injected clock that is driven over the real
//! JSON Lines dispatch (`Core::handle_line`).
#![allow(dead_code)]

use pime_core::clock::ManualClock;
use pime_core::json::{parse, Json, Limits};
use pime_core::protocol::Core;

pub const ENTROPY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

pub fn entropy(seed: u8) -> String {
    (0..32u8)
        .map(|i| format!("{:02x}", i.wrapping_mul(5).wrapping_add(seed)))
        .collect()
}

pub struct Harness {
    pub core: Core,
    pub clock: ManualClock,
    pub session: String,
    next_id: i64,
}

impl Harness {
    pub fn without_session() -> Harness {
        let clock = ManualClock::new(1_000_000);
        Harness {
            core: Core::with_clock(Box::new(clock.clone())),
            clock,
            session: String::new(),
            next_id: 1,
        }
    }

    pub fn open() -> Harness {
        let mut h = Harness::without_session();
        let r = h.call(&format!(
            r#"{{"op":"session.open","locale":"en-US","entropy":"{ENTROPY}","settings":{{}}}}"#
        ));
        assert_eq!(r.get("ok"), Some(&Json::Bool(true)), "{r:?}");
        h.session = r.get("session").and_then(Json::as_str).unwrap().to_string();
        h
    }

    /// A second session on the same core.
    pub fn open_another(&mut self, seed: u8) -> String {
        let r = self.call(&format!(
            r#"{{"op":"session.open","locale":"en-US","entropy":"{}"}}"#,
            entropy(seed)
        ));
        s(&r, "session").to_string()
    }

    /// Sends one request (the `id` and, when a session is open, the `session` are added) and parses the answer.
    pub fn call(&mut self, body: &str) -> Json {
        let mut req = parse(body, big_limits()).expect("request JSON");
        if let Json::Obj(o) = &mut req {
            let id = self.next_id;
            self.next_id += 1;
            o.push(("id".to_string(), Json::int(id)));
            if !self.session.is_empty() && !o.iter().any(|(k, _)| k == "session") {
                o.push(("session".to_string(), Json::str(&self.session)));
            }
        }
        let reply = self.core.handle_line(&req.to_json_string());
        let resp =
            parse(&reply, big_limits()).unwrap_or_else(|e| panic!("bad response {e}: {reply}"));
        assert_eq!(
            resp.get("id").and_then(Json::as_i64),
            Some(self.next_id - 1),
            "the id is echoed: {}",
            &reply[..reply.len().min(200)]
        );
        resp
    }
}

pub fn big_limits() -> Limits {
    Limits {
        max_bytes: 64 << 20,
        max_depth: 64,
    }
}

pub fn s<'a>(v: &'a Json, key: &str) -> &'a str {
    v.get(key).and_then(Json::as_str).unwrap_or("<missing>")
}

pub fn is_ok(v: &Json) -> bool {
    v.get("ok") == Some(&Json::Bool(true))
}

pub fn err_code(v: &Json) -> &str {
    assert_eq!(v.get("ok"), Some(&Json::Bool(false)), "expected a failure");
    s(v, "error")
}

/// A JSON string literal for `s`.
pub fn q(s: &str) -> String {
    Json::str(s).to_json_string()
}
