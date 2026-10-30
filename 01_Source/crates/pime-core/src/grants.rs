//! Permission broker (FR-016, DD section 15), port of `core/grants.js`: grants are bound to
//! (provider+host, workspace, scope, expiry) and are revocable. Per-request grants are consumed on use. Nothing
//! here stores file contents or keys.
//!
//! Persistence stays in the JavaScript bridge for now: [`PermissionBroker::export`] returns the reusable grants
//! in the JSON shape the JS broker stored (`pime.grants.v1`) and [`PermissionBroker::import`] loads them back, so
//! switching between the engines keeps the user's authorisations.

use crate::error::{Result, fail};
use crate::ids::IdGen;
use crate::json::Json;
use crate::sha256::sha256_hex;

pub const SCOPES: [&str; 4] = ["L1", "L2", "L3", "L4"];
pub const DEFAULT_TTL_MS: u64 = 24 * 3600 * 1000;

/// `sha256(sorted uris joined by "\n")`, first 24 hex characters. The uris are sorted by UTF-16 code unit, as
/// JavaScript `Array.prototype.sort` does.
pub fn workspace_token(uris: &[String]) -> String {
    let mut sorted: Vec<&String> = uris.iter().collect();
    sorted.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    let joined = sorted
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    sha256_hex(&joined)[..24].to_string()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Grant {
    pub id: String,
    pub provider_id: String,
    pub workspace: String,
    pub scope: String,
    pub once: bool,
    pub used: bool,
    pub revoked: bool,
    pub created_at: u64,
    pub expires_at: u64,
}

impl Grant {
    /// The full object as the JS broker keeps (and persists) it.
    pub fn to_json(&self) -> Json {
        Json::obj(vec![
            ("id", Json::str(&self.id)),
            ("providerId", Json::str(&self.provider_id)),
            ("workspace", Json::str(&self.workspace)),
            ("scope", Json::str(&self.scope)),
            ("once", Json::Bool(self.once)),
            ("used", Json::Bool(self.used)),
            ("revoked", Json::Bool(self.revoked)),
            ("createdAt", Json::Num(self.created_at as f64)),
            ("expiresAt", Json::Num(self.expires_at as f64)),
        ])
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Criteria<'a> {
    pub provider_id: &'a str,
    pub workspace: &'a str,
    pub scope: &'a str,
}

#[derive(Clone, Debug)]
pub struct AuthorizeRequest<'a> {
    pub provider_id: &'a str,
    pub workspace: &'a str,
    pub scope: &'a str,
    pub ttl_ms: u64,
    pub once: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check {
    pub ok: bool,
    /// `ok`, `unknown-grant`, `revoked`, `expired`, `already-used`, `provider-mismatch`, `workspace-mismatch`,
    /// `scope-mismatch`. The reason never contains workspace paths or provider secrets.
    pub reason: &'static str,
}

/// What `list` shows about a grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub id: String,
    pub provider_id: String,
    pub scope: String,
    pub once: bool,
    pub expires_at: u64,
}

#[derive(Default)]
pub struct PermissionBroker {
    grants: Vec<Grant>,
}

impl PermissionBroker {
    pub fn new() -> PermissionBroker {
        PermissionBroker::default()
    }

    fn get(&self, id: &str) -> Option<&Grant> {
        self.grants.iter().find(|g| g.id == id)
    }

    pub fn authorize(
        &mut self,
        now_ms: u64,
        ids: &mut IdGen,
        req: AuthorizeRequest,
    ) -> Result<Grant> {
        if req.provider_id.is_empty() || req.workspace.is_empty() || !SCOPES.contains(&req.scope) {
            return fail("GRANT_DENIED", "invalid grant request");
        }
        let grant = Grant {
            id: ids.next("grant", 12),
            provider_id: req.provider_id.to_string(),
            workspace: req.workspace.to_string(),
            scope: req.scope.to_string(),
            once: req.once,
            used: false,
            revoked: false,
            created_at: now_ms,
            expires_at: now_ms.saturating_add(req.ttl_ms),
        };
        self.grants.push(grant.clone());
        Ok(grant)
    }

    /// Any mismatch is a denial.
    pub fn check(&self, now_ms: u64, grant_id: &str, c: Criteria) -> Check {
        let deny = |reason| Check { ok: false, reason };
        let Some(g) = self.get(grant_id) else {
            return deny("unknown-grant");
        };
        if g.revoked {
            return deny("revoked");
        }
        if g.expires_at <= now_ms {
            return deny("expired");
        }
        if g.once && g.used {
            return deny("already-used");
        }
        if g.provider_id != c.provider_id {
            return deny("provider-mismatch");
        }
        if g.workspace != c.workspace {
            return deny("workspace-mismatch");
        }
        if g.scope != c.scope {
            return deny("scope-mismatch");
        }
        Check {
            ok: true,
            reason: "ok",
        }
    }

    /// A reusable (workspace-wide) grant for the criteria, if there is one.
    pub fn find(&self, now_ms: u64, c: Criteria) -> Option<&Grant> {
        self.grants
            .iter()
            .find(|g| !g.once && self.check(now_ms, &g.id, c).ok)
    }

    pub fn require(&self, now_ms: u64, grant_id: &str, c: Criteria) -> Result<&Grant> {
        let r = self.check(now_ms, grant_id, c);
        if !r.ok {
            return fail(
                "GRANT_DENIED",
                format!("authorisation denied ({})", r.reason),
            );
        }
        match self.get(grant_id) {
            Some(g) => Ok(g),
            None => fail("GRANT_DENIED", "authorisation denied (unknown-grant)"),
        }
    }

    /// Still honoured at Apply time: a one-use grant is already "used" by then, but a revoked or expired one is not.
    pub fn still_valid(&self, now_ms: u64, grant_id: &str) -> bool {
        self.get(grant_id)
            .is_some_and(|g| !g.revoked && g.expires_at > now_ms)
    }

    pub fn consume(&mut self, grant_id: &str) {
        if let Some(g) = self.grants.iter_mut().find(|g| g.id == grant_id)
            && g.once
        {
            g.used = true;
        }
    }

    pub fn revoke(&mut self, grant_id: &str) -> bool {
        match self.grants.iter_mut().find(|g| g.id == grant_id) {
            Some(g) => {
                g.revoked = true;
                true
            }
            None => false,
        }
    }

    pub fn revoke_provider(&mut self, provider_id: &str) -> usize {
        self.revoke_where(|g| g.provider_id == provider_id)
    }

    /// Every grant whose provider id starts with `prefix` (`"<provider>:"` = every host of one provider), used
    /// when the provider's key is deleted: no authorisation survives its key.
    pub fn revoke_provider_prefix(&mut self, prefix: &str) -> usize {
        self.revoke_where(|g| g.provider_id.starts_with(prefix))
    }

    pub fn revoke_all(&mut self) -> usize {
        self.revoke_where(|_| true)
    }

    fn revoke_where(&mut self, f: impl Fn(&Grant) -> bool) -> usize {
        let mut n = 0;
        for g in self.grants.iter_mut().filter(|g| !g.revoked && f(g)) {
            g.revoked = true;
            n += 1;
        }
        n
    }

    pub fn list(&self, now_ms: u64) -> Vec<Listed> {
        self.grants
            .iter()
            .filter(|g| !g.revoked && g.expires_at > now_ms && !(g.once && g.used))
            .map(|g| Listed {
                id: g.id.clone(),
                provider_id: g.provider_id.clone(),
                scope: g.scope.clone(),
                once: g.once,
                expires_at: g.expires_at,
            })
            .collect()
    }

    /// The reusable, not revoked grants (what the bridge persists). One-request grants are never persisted.
    pub fn export(&self) -> Vec<Json> {
        self.grants
            .iter()
            .filter(|g| !g.once && !g.revoked)
            .map(Grant::to_json)
            .collect()
    }

    /// Loads persisted grants. Entries that are malformed (not an object, no string id, unknown scope, a
    /// one-request grant, non-string provider or workspace, non-numeric times) are skipped. Returns how many
    /// were loaded.
    pub fn import(&mut self, entries: &[Json]) -> usize {
        let mut loaded = 0;
        for e in entries {
            let Some(g) = parse_grant(e) else { continue };
            match self.grants.iter_mut().find(|x| x.id == g.id) {
                Some(slot) => *slot = g,
                None => self.grants.push(g),
            }
            loaded += 1;
        }
        loaded
    }
}

fn parse_grant(e: &Json) -> Option<Grant> {
    let id = e
        .get("id")?
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 64)?;
    let scope = e.get("scope")?.as_str().filter(|s| SCOPES.contains(s))?;
    if e.get("once").and_then(Json::as_bool).unwrap_or(false) {
        return None;
    }
    let time = |key: &str| {
        e.get(key)
            .and_then(Json::as_f64)
            .filter(|n| n.is_finite() && *n >= 0.0)
            .map(|n| n as u64)
    };
    Some(Grant {
        id: id.to_string(),
        provider_id: e
            .get("providerId")?
            .as_str()
            .filter(|s| !s.is_empty())?
            .to_string(),
        workspace: e
            .get("workspace")?
            .as_str()
            .filter(|s| !s.is_empty())?
            .to_string(),
        scope: scope.to_string(),
        once: false,
        used: false,
        revoked: e.get("revoked").and_then(Json::as_bool).unwrap_or(false),
        created_at: time("createdAt").unwrap_or(0),
        expires_at: time("expiresAt")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Limits, parse};

    fn ids() -> IdGen {
        let entropy: Vec<u8> = (0..32u8)
            .map(|i| i.wrapping_mul(13).wrapping_add(5))
            .collect();
        IdGen::new(&entropy).unwrap()
    }

    fn req<'a>(provider: &'a str, ws: &'a str, scope: &'a str) -> AuthorizeRequest<'a> {
        AuthorizeRequest {
            provider_id: provider,
            workspace: ws,
            scope,
            ttl_ms: DEFAULT_TTL_MS,
            once: false,
        }
    }

    fn crit<'a>(provider: &'a str, ws: &'a str, scope: &'a str) -> Criteria<'a> {
        Criteria {
            provider_id: provider,
            workspace: ws,
            scope,
        }
    }

    #[test]
    fn ut_029_it_015_grants_any_mismatch_is_denied_revoke_stops_new_requests() {
        let mut now = 1000u64;
        let mut ids = ids();
        let mut b = PermissionBroker::new();
        let ws = workspace_token(&["file:///w".into()]);
        let ws2 = workspace_token(&["file:///other".into()]);
        let g = b
            .authorize(
                now,
                &mut ids,
                AuthorizeRequest {
                    ttl_ms: 5000,
                    ..req("A:host", &ws, "L1")
                },
            )
            .unwrap();
        let ok = crit("A:host", &ws, "L1");
        assert!(b.check(now, &g.id, ok).ok);
        assert_eq!(
            b.check(now, &g.id, crit("B:host", &ws, "L1")).reason,
            "provider-mismatch"
        );
        assert_eq!(
            b.check(now, &g.id, crit("A:host", &ws2, "L1")).reason,
            "workspace-mismatch"
        );
        assert_eq!(
            b.check(now, &g.id, crit("A:host", &ws, "L3")).reason,
            "scope-mismatch",
            "L1 authorisation does not cover L3"
        );
        assert_eq!(b.check(now, "nope", ok).reason, "unknown-grant");
        assert_eq!(
            b.require(now, &g.id, crit("A:host", &ws, "L3"))
                .unwrap_err()
                .code,
            "GRANT_DENIED"
        );
        now += 6000;
        assert_eq!(b.check(now, &g.id, ok).reason, "expired");
        now = 1000;
        assert!(b.check(now, &g.id, ok).ok);
        assert!(b.revoke(&g.id));
        assert_eq!(b.check(now, &g.id, ok).reason, "revoked");
        assert!(b.find(now, ok).is_none());
        assert!(!b.still_valid(now, &g.id));
        let once = b
            .authorize(
                now,
                &mut ids,
                AuthorizeRequest {
                    once: true,
                    ..req("A:host", &ws, "L3")
                },
            )
            .unwrap();
        assert!(
            b.find(now, crit("A:host", &ws, "L3")).is_none(),
            "one-request grants are never reused"
        );
        b.consume(&once.id);
        assert_eq!(
            b.check(now, &once.id, crit("A:host", &ws, "L3")).reason,
            "already-used"
        );
        assert!(
            b.still_valid(now, &once.id),
            "a used one-request grant is still honoured by Apply until revoked"
        );
        assert_eq!(
            b.authorize(now, &mut ids, req("A", &ws, "L9"))
                .unwrap_err()
                .code,
            "GRANT_DENIED"
        );
        assert_eq!(
            b.authorize(now, &mut ids, req("", &ws, "L1"))
                .unwrap_err()
                .code,
            "GRANT_DENIED"
        );
        assert_eq!(
            b.authorize(now, &mut ids, req("A", "", "L1"))
                .unwrap_err()
                .code,
            "GRANT_DENIED"
        );
    }

    #[test]
    fn grant_ids_are_24_hex_characters_and_unique() {
        let mut ids = ids();
        let mut b = PermissionBroker::new();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..40 {
            let g = b.authorize(1, &mut ids, req("A:h", "w", "L1")).unwrap();
            assert_eq!(g.id.len(), 24);
            assert!(seen.insert(g.id));
        }
    }

    #[test]
    fn find_returns_the_reusable_grant_and_require_returns_the_grant() {
        let mut ids = ids();
        let mut b = PermissionBroker::new();
        let w = b.authorize(10, &mut ids, req("A:h", "ws", "L3")).unwrap();
        assert_eq!(
            b.find(10, crit("A:h", "ws", "L3")).map(|g| g.id.clone()),
            Some(w.id.clone())
        );
        assert!(b.find(10, crit("A:h", "ws", "L1")).is_none());
        assert_eq!(
            b.require(10, &w.id, crit("A:h", "ws", "L3")).unwrap().id,
            w.id
        );
    }

    #[test]
    fn revocation_variants_and_the_list_of_live_grants() {
        let mut ids = ids();
        let mut b = PermissionBroker::new();
        let a1 = b
            .authorize(10, &mut ids, req("mock:127.0.0.1:1", "ws", "L1"))
            .unwrap();
        let a2 = b
            .authorize(10, &mut ids, req("mock:127.0.0.1:1", "ws", "L3"))
            .unwrap();
        let o = b
            .authorize(
                10,
                &mut ids,
                AuthorizeRequest {
                    once: true,
                    ..req("mock:127.0.0.1:1", "ws", "L3")
                },
            )
            .unwrap();
        let other = b
            .authorize(10, &mut ids, req("other:h", "ws", "L1"))
            .unwrap();
        assert_eq!(b.list(10).len(), 4);
        b.consume(&o.id);
        assert_eq!(
            b.list(10).len(),
            3,
            "a used one-request grant is not listed"
        );
        assert_eq!(b.list(10)[0].provider_id, "mock:127.0.0.1:1");
        assert_eq!(
            b.revoke_provider("mock:127.0.0.1:1"),
            3,
            "includes the used one-request grant"
        );
        assert_eq!(b.revoke_provider("mock:127.0.0.1:1"), 0);
        assert!(b.still_valid(10, &other.id));
        assert!(
            !b.still_valid(10, &a1.id) && !b.still_valid(10, &a2.id) && !b.still_valid(10, &o.id)
        );
        assert_eq!(b.revoke_all(), 1);
        assert!(b.list(10).is_empty());
        assert!(!b.revoke("nope"));
        // prefix revoke reaches every host of one provider, used or expired grants included
        let mut b = PermissionBroker::new();
        let x = b
            .authorize(
                1,
                &mut ids,
                AuthorizeRequest {
                    once: true,
                    ..req("mockA:h1", "w", "L3")
                },
            )
            .unwrap();
        b.consume(&x.id);
        b.authorize(
            1,
            &mut ids,
            AuthorizeRequest {
                ttl_ms: 1,
                ..req("mockA:h2", "w", "L1")
            },
        )
        .unwrap();
        b.authorize(1, &mut ids, req("mockAB:h", "w", "L1"))
            .unwrap();
        assert_eq!(
            b.revoke_provider_prefix("mockA:"),
            2,
            "the used grant of h1 and the expired one of h2"
        );
        assert_eq!(b.revoke_provider_prefix("mockA:h2"), 0, "already revoked");
        assert_eq!(
            b.list(1).len(),
            1,
            "mockAB is not covered by the prefix \"mockA:\""
        );
    }

    #[test]
    fn workspace_tokens_hash_the_sorted_uris_and_never_contain_paths() {
        let a = workspace_token(&["file:///b".into(), "file:///a".into()]);
        let b = workspace_token(&["file:///a".into(), "file:///b".into()]);
        assert_eq!(a, b, "order does not matter");
        assert_eq!(a.len(), 24);
        assert_eq!(a, sha256_hex("file:///a\nfile:///b")[..24]);
        assert!(!a.contains("file"));
        assert_ne!(a, workspace_token(&["file:///a".into()]));
        // sorting is by UTF-16 code unit like JavaScript: U+FFFF (one unit 0xFFFF) sorts after U+1F600
        // (surrogate pair 0xD83D 0xDE00), although its UTF-8 / code point order is the other way round
        let t = workspace_token(&["\u{ffff}".into(), "\u{1F600}".into()]);
        assert_eq!(t, sha256_hex("\u{1F600}\n\u{ffff}")[..24]);
    }

    #[test]
    fn only_reusable_grants_are_exported_and_they_survive_a_round_trip() {
        let mut ids = ids();
        let mut b = PermissionBroker::new();
        let w = b
            .authorize(100, &mut ids, req("A:h", "wsToken", "L1"))
            .unwrap();
        b.authorize(
            100,
            &mut ids,
            AuthorizeRequest {
                once: true,
                ..req("A:h", "wsToken", "L3")
            },
        )
        .unwrap();
        let revoked = b
            .authorize(100, &mut ids, req("B:h", "wsToken", "L1"))
            .unwrap();
        b.revoke(&revoked.id);
        let exported = b.export();
        assert_eq!(exported.len(), 1, "only reusable grants persist");
        let text = Json::Arr(exported.clone()).to_json_string();
        assert!(
            !text.contains("file://"),
            "workspace paths are stored only as hashes"
        );
        // the exact JS shape
        assert_eq!(
            exported[0].to_json_string(),
            format!(
                r#"{{"id":"{}","providerId":"A:h","workspace":"wsToken","scope":"L1","once":false,"used":false,"revoked":false,"createdAt":100,"expiresAt":{}}}"#,
                w.id,
                100 + DEFAULT_TTL_MS
            )
        );
        let reloaded_json = parse(&text, Limits::DEFAULT).unwrap();
        let mut r = PermissionBroker::new();
        assert_eq!(r.import(reloaded_json.as_array().unwrap()), 1);
        let list = r.list(100);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, w.id);
        assert!(r.find(100, crit("A:h", "wsToken", "L1")).is_some());
        // importing the same id again replaces, not duplicates
        assert_eq!(r.import(reloaded_json.as_array().unwrap()), 1);
        assert_eq!(r.list(100).len(), 1);
    }

    #[test]
    fn import_skips_malformed_entries_instead_of_trusting_them() {
        let entries = parse(
            r#"[null, 5, {"id":3}, {"id":"ok1","providerId":"A","workspace":"w","scope":"L9","expiresAt":9},
                {"id":"ok2","providerId":"A","workspace":"w","scope":"L1","once":true,"expiresAt":9},
                {"id":"ok3","providerId":7,"workspace":"w","scope":"L1","expiresAt":9},
                {"id":"ok4","providerId":"A","workspace":"w","scope":"L1","expiresAt":"never"},
                {"id":"ok5","providerId":"A","workspace":"w","scope":"L1","expiresAt":500,"createdAt":1},
                {"id":"","providerId":"A","workspace":"w","scope":"L1","expiresAt":500}]"#,
            Limits::DEFAULT,
        )
        .unwrap();
        let mut b = PermissionBroker::new();
        assert_eq!(b.import(entries.as_array().unwrap()), 1);
        assert_eq!(b.list(100)[0].id, "ok5");
        assert!(b.list(600).is_empty(), "the imported expiry is honoured");
    }
}
