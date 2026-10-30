//! Outbound sensitivity screening (FR-049, DD section 7.3), hand-written port of `secrets.js`.
//! Findings carry kinds, severities and offsets/lines only - never the matched value - so they are safe to log.
//! The scanner cannot find every secret; it reduces accidental exposure.
//!
//! Every rule is a position matcher that reproduces the backtracking semantics of the original regular
//! expression (greedy runs, `\b` after a run, leftmost match, non-overlapping per rule).

use crate::util::{boundary, head_utf16, is_js_space, is_word, skip_ws, starts_at, starts_at_ci};

pub const MAX_SCAN: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub kind: &'static str,
    pub severity: &'static str,
    /// Char offsets of the sensitive span (never expose the text itself).
    pub start: usize,
    pub end: usize,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanResult {
    pub findings: Vec<Finding>,
    pub blocked: bool,
    pub sensitive_file: bool,
    pub highest: &'static str,
    pub truncated: bool,
}

/// `SENSITIVE_FILE`: the last path component names a credential-like file.
pub fn is_sensitive_file_name(name: &str) -> bool {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let b = base.to_ascii_lowercase();
    if b == ".env" {
        return true;
    }
    if let Some(rest) = b.strip_prefix(".env.")
        && !rest.is_empty()
        && rest.chars().all(|c| is_word(c) || c == '.' || c == '-')
    {
        return true;
    }
    for ext in ["pem", "key", "pfx", "p12", "jks", "keystore"] {
        if b.ends_with(&format!(".{ext}")) {
            return true;
        }
    }
    matches!(
        b.as_str(),
        "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
            | "credentials"
            | "credentials.json"
            | ".npmrc"
            | ".netrc"
            | ".pypirc"
    ) || {
        let stem_ext = b.split_once('.');
        matches!(
            stem_ext,
            Some(("secret" | "secrets", "json" | "yaml" | "yml" | "toml"))
        )
    }
}

/// `PLACEHOLDER_VALUE` (case-insensitive): obvious dummy values are not findings.
fn is_placeholder_value(v: &str) -> bool {
    let l = v.to_lowercase();
    (l.chars().count() >= 4 && l.chars().all(|c| c == 'x'))
        || (l.chars().count() >= 3 && l.chars().all(|c| c == '*'))
        || (l.chars().count() >= 3 && l.chars().all(|c| c == '.'))
        || l.starts_with("your_")
        || l.starts_with("your-")
        || l == "changeme"
        || l == "change-me"
        || l.starts_with("example")
        || l.starts_with("placeholder")
        || (l.chars().count() >= 2 && l.starts_with('<') && l.ends_with('>'))
        || matches!(
            l.as_str(),
            "todo" | "none" | "null" | "undefined" | "true" | "false"
        )
}

enum Step {
    Hit {
        end: usize,
        cap: (usize, usize),
    },
    /// No match at this position; the scan may resume at the given position.
    Miss(usize),
}

fn miss(p: usize) -> Step {
    Step::Miss(p + 1)
}

fn hit(end: usize) -> Step {
    Step::Hit { end, cap: (0, 0) }
}

fn run_end(t: &[char], start: usize, class: impl Fn(char) -> bool) -> usize {
    let mut e = start;
    while e < t.len() && class(t[e]) {
        e += 1;
    }
    e
}

/// Greedy `class{min,}` followed by `\b`, with backtracking over the run length.
fn greedy_bounded(
    t: &[char],
    start: usize,
    class: impl Fn(char) -> bool,
    min: usize,
) -> Option<usize> {
    let m = run_end(t, start, class) - start;
    if m < min {
        return None;
    }
    (min..=m).rev().map(|l| start + l).find(|&e| boundary(t, e))
}

fn alnum(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

fn private_key(t: &[char], p: usize) -> Step {
    if !starts_at(t, p, "-----BEGIN ") {
        return miss(p);
    }
    let mut q = p + 11;
    let mut positions = vec![q];
    loop {
        let r = run_end(t, q, |c| c.is_ascii_uppercase() || c.is_ascii_digit());
        if r > q && r < t.len() && t[r] == ' ' {
            q = r + 1;
            positions.push(q);
        } else {
            break;
        }
    }
    for &s in positions.iter().rev() {
        if starts_at(t, s, "PRIVATE KEY") {
            let e = s + 11;
            if starts_at(t, e, " BLOCK") && starts_at(t, e + 6, "-----") {
                return hit(e + 11);
            }
            if starts_at(t, e, "-----") {
                return hit(e + 5);
            }
        }
    }
    miss(p)
}

fn aws_key(t: &[char], p: usize) -> Step {
    if !boundary(t, p) || !(starts_at(t, p, "AKIA") || starts_at(t, p, "ASIA")) {
        return miss(p);
    }
    let end = p + 20;
    if end <= t.len()
        && t[p + 4..end]
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && boundary(t, end)
    {
        hit(end)
    } else {
        miss(p)
    }
}

fn github_token(t: &[char], p: usize) -> Step {
    if !boundary(t, p) {
        return miss(p);
    }
    if starts_at(t, p, "gh")
        && p + 3 < t.len()
        && matches!(t[p + 2], 'p' | 'o' | 'u' | 's' | 'r')
        && t[p + 3] == '_'
        && let Some(e) = greedy_bounded(t, p + 4, alnum, 36)
    {
        return hit(e);
    }
    if starts_at(t, p, "github_pat_")
        && let Some(e) = greedy_bounded(t, p + 11, |c| alnum(c) || c == '_', 40)
    {
        return hit(e);
    }
    miss(p)
}

fn api_key(t: &[char], p: usize) -> Step {
    if !boundary(t, p) || !starts_at(t, p, "sk-") {
        return miss(p);
    }
    let class = |c: char| alnum(c) || c == '_' || c == '-';
    for prefix in ["proj-", "live-", "test-", "ant-"] {
        if starts_at(t, p + 3, prefix)
            && let Some(e) = greedy_bounded(t, p + 3 + prefix.len(), class, 20)
        {
            return hit(e);
        }
    }
    match greedy_bounded(t, p + 3, class, 20) {
        Some(e) => hit(e),
        None => miss(p),
    }
}

fn slack_token(t: &[char], p: usize) -> Step {
    if !boundary(t, p)
        || !starts_at(t, p, "xox")
        || p + 4 >= t.len()
        || !matches!(t[p + 3], 'a' | 'b' | 'p' | 'r' | 's')
        || t[p + 4] != '-'
    {
        return miss(p);
    }
    match greedy_bounded(t, p + 5, |c| alnum(c) || c == '-', 10) {
        Some(e) => hit(e),
        None => miss(p),
    }
}

fn google_key(t: &[char], p: usize) -> Step {
    if !boundary(t, p) || !starts_at(t, p, "AIza") {
        return miss(p);
    }
    let end = p + 39;
    if end <= t.len()
        && t[p + 4..end]
            .iter()
            .all(|&c| alnum(c) || c == '_' || c == '-')
        && boundary(t, end)
    {
        hit(end)
    } else {
        miss(p)
    }
}

fn bearer(t: &[char], p: usize) -> Step {
    if !boundary(t, p) || !starts_at(t, p, "Bearer") {
        return miss(p);
    }
    let q = skip_ws(t, p + 6);
    if q == p + 6 {
        return miss(p);
    }
    let class = |c: char| alnum(c) || matches!(c, '.' | '_' | '~' | '+' | '/' | '-');
    let e = run_end(t, q, class);
    if e - q < 20 {
        return miss(p);
    }
    hit(run_end(t, e, |c| c == '='))
}

fn jwt(t: &[char], p: usize) -> Step {
    if !boundary(t, p) || !starts_at(t, p, "eyJ") {
        return miss(p);
    }
    let class = |c: char| alnum(c) || c == '_' || c == '-';
    let r1 = run_end(t, p + 3, class);
    if r1 - (p + 3) < 8 || !starts_at(t, r1, ".eyJ") {
        return miss(p);
    }
    let s2 = r1 + 4;
    let r2 = run_end(t, s2, class);
    if r2 - s2 < 8 || r2 >= t.len() || t[r2] != '.' {
        return miss(p);
    }
    match greedy_bounded(t, r2 + 1, class, 8) {
        Some(e) => hit(e),
        None => miss(p),
    }
}

fn url_credentials(t: &[char], p: usize) -> Step {
    if !boundary(t, p) || !t[p].is_ascii_alphabetic() {
        return miss(p);
    }
    let e = run_end(t, p + 1, |c| alnum(c) || matches!(c, '+' | '.' | '-'));
    // every later start inside the same scheme run shares this tail, so a miss can skip the whole run
    let skip = Step::Miss(e.max(p + 1));
    if !starts_at(t, e, "://") {
        return skip;
    }
    let u0 = e + 3;
    let u1 = run_end(t, u0, |c| {
        !is_js_space(c) && !matches!(c, '/' | ':' | '@' | '"' | '\'')
    });
    if u1 == u0 || u1 >= t.len() || t[u1] != ':' {
        return skip;
    }
    let p0 = u1 + 1;
    let p1 = run_end(t, p0, |c| {
        !is_js_space(c) && !matches!(c, '/' | '@' | '"' | '\'')
    });
    if p1 == p0 || p1 >= t.len() || t[p1] != '@' {
        return skip;
    }
    hit(p1 + 1)
}

fn has_keyword(run: &[char]) -> bool {
    let s: String = run.iter().map(|c| c.to_ascii_lowercase()).collect();
    const SEP: [&str; 3] = ["", "_", "-"];
    for k in ["secret", "token", "passwd", "password", "pwd"] {
        if s.contains(k) {
            return true;
        }
    }
    for (a, b) in [
        ("api", "key"),
        ("private", "key"),
        ("access", "key"),
        ("client", "secret"),
    ] {
        for sep in SEP {
            if s.contains(&format!("{a}{sep}{b}")) {
                return true;
            }
        }
    }
    false
}

fn credential_assignment(t: &[char], p: usize) -> Step {
    if !boundary(t, p) {
        return miss(p);
    }
    let e = run_end(t, p, |c| is_word(c) || c == '.' || c == '-');
    if e == p {
        return miss(p);
    }
    // the run end and everything after it are shared by all later starts in this run
    let skip = Step::Miss(e);
    if !has_keyword(&t[p..e]) {
        return skip;
    }
    let q = skip_ws(t, e);
    if q >= t.len() || (t[q] != ':' && t[q] != '=') {
        return skip;
    }
    let mut r = skip_ws(t, q + 1);
    if r < t.len() && (t[r] == '"' || t[r] == '\'') {
        r += 1;
    }
    for blocked in [
        "${",
        "<",
        "{{",
        "process.env",
        "os.environ",
        "getenv",
        "env(",
    ] {
        if starts_at_ci(t, r, blocked) {
            return skip;
        }
    }
    let c = run_end(t, r, |ch| {
        !is_js_space(ch) && !matches!(ch, '"' | '\'' | '`' | ';' | ',' | '#')
    });
    let units: usize = t[r..c].iter().map(|ch| ch.len_utf16()).sum();
    if units < 8 {
        return skip;
    }
    Step::Hit {
        end: c,
        cap: (r, c),
    }
}

fn private_path(t: &[char], p: usize) -> Step {
    if boundary(t, p) && t[p].is_ascii_alphabetic() && starts_at(t, p + 1, ":\\Users\\") {
        let s = p + 9;
        let e = run_end(t, s, |c| {
            !is_js_space(c) && !matches!(c, '\\' | '"' | '\'' | '<' | '>' | '|')
        });
        if e > s {
            return hit(e);
        }
    }
    if t[p] == '/' {
        for dir in ["Users", "home"] {
            if starts_at(t, p + 1, dir) && starts_at(t, p + 1 + dir.len(), "/") {
                let s = p + 2 + dir.len();
                let e = run_end(t, s, |c| {
                    !is_js_space(c) && !matches!(c, '/' | '"' | '\'' | '<' | '>')
                });
                if e > s {
                    return hit(e);
                }
            }
        }
    }
    miss(p)
}

type Matcher = fn(&[char], usize) -> Step;

const RULES: [(&str, &str, Matcher); 11] = [
    ("private-key", "high", private_key),
    ("aws-access-key", "high", aws_key),
    ("github-token", "high", github_token),
    ("api-key", "high", api_key),
    ("slack-token", "high", slack_token),
    ("google-api-key", "high", google_key),
    ("bearer-token", "high", bearer),
    ("jwt", "medium", jwt),
    ("url-credentials", "medium", url_credentials),
    ("credential-assignment", "medium", credential_assignment),
    ("private-path", "medium", private_path),
];

/// Scans `text`; `file_name` only feeds the sensitive-file check.
pub fn scan(text: &str, file_name: &str) -> ScanResult {
    let sensitive_file = is_sensitive_file_name(file_name);
    let sample = head_utf16(text, MAX_SCAN);
    let truncated = sample.len() != text.len();
    let t: Vec<char> = sample.chars().collect();
    let newlines: Vec<usize> = t
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == '\n')
        .map(|(i, _)| i)
        .collect();
    let line_at = |offset: usize| newlines.partition_point(|&n| n < offset) + 1;
    let mut findings: Vec<Finding> = Vec::new();
    for (kind, severity, matcher) in RULES {
        let mut p = 0usize;
        while p < t.len() {
            match matcher(&t, p) {
                Step::Miss(next) => p = next.max(p + 1),
                Step::Hit { end, cap } => {
                    let (start, stop) = if cap.1 > cap.0 { cap } else { (p, end) };
                    let placeholder = kind == "credential-assignment"
                        && is_placeholder_value(&t[start..stop].iter().collect::<String>());
                    if !placeholder {
                        findings.push(Finding {
                            kind,
                            severity,
                            start,
                            end: stop,
                            line: line_at(start),
                        });
                    }
                    p = end.max(p + 1);
                }
            }
        }
    }
    findings.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let highest = if findings.iter().any(|f| f.severity == "high") || sensitive_file {
        "high"
    } else if findings.is_empty() {
        "none"
    } else {
        "medium"
    };
    ScanResult {
        findings,
        blocked: highest == "high",
        sensitive_file,
        highest,
        truncated,
    }
}

/// What may leave the machine (`assessOutbound`): `allow`, `redact` (medium findings), and for high-risk content
/// `block` in the realtime scope (never sent) or `confirm` (the user must decide explicitly).
pub fn assess_outbound(text: &str, file_name: &str, scope: &str) -> (&'static str, ScanResult) {
    let scanned = scan(text, file_name);
    let action = match scanned.highest {
        "none" => "allow",
        "medium" => "redact",
        _ if scope == "realtime" => "block",
        _ => "confirm",
    };
    (action, scanned)
}

/// `summarize`: number of findings per kind, in the order the kinds first occur.
pub fn summarize(findings: &[Finding]) -> Vec<(&'static str, usize)> {
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    for f in findings {
        match counts.iter_mut().find(|(k, _)| *k == f.kind) {
            Some(slot) => slot.1 += 1,
            None => counts.push((f.kind, 1)),
        }
    }
    counts
}

/// One redacted value; the map of a redaction lets a model reply be restored locally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redacted {
    pub marker: String,
    pub original: String,
    pub kind: &'static str,
}

/// Replaces the findings with `PIME_REDACTED_n` markers (numbered from 1 in this call). Overlapping findings are
/// covered by the first one.
pub fn redact(text: &str, findings: &[Finding]) -> (String, Vec<Redacted>) {
    let t: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut map = Vec::new();
    let mut cursor = 0usize;
    for f in findings {
        if f.start < cursor {
            continue; // overlapping finding already covered
        }
        let marker = format!("PIME_REDACTED_{}", map.len() + 1);
        out.extend(&t[cursor..f.start]);
        out.push_str(&marker);
        map.push(Redacted {
            marker,
            original: t[f.start..f.end].iter().collect(),
            kind: f.kind,
        });
        cursor = f.end;
    }
    out.extend(&t[cursor..]);
    (out, map)
}

/// Puts the originals back; a marker that is not in the map stays and is counted as unresolved. When two entries
/// share a marker the later one wins (a `Map` built from the list, as in the reference).
pub fn restore(text: &str, map: &[Redacted]) -> (String, usize) {
    const PREFIX: &str = "PIME_REDACTED_";
    let t: Vec<char> = text.chars().collect();
    let prefix: Vec<char> = PREFIX.chars().collect();
    let mut out = String::new();
    let mut unresolved = 0usize;
    let mut i = 0usize;
    while i < t.len() {
        if t[i..].starts_with(&prefix) {
            let digits = t[i + prefix.len()..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
            if digits > 0 {
                let end = i + prefix.len() + digits;
                let marker: String = t[i..end].iter().collect();
                match map.iter().rev().find(|r| r.marker == marker) {
                    Some(r) => out.push_str(&r.original),
                    None => {
                        unresolved += 1;
                        out.push_str(&marker);
                    }
                }
                i = end;
                continue;
            }
        }
        out.push(t[i]);
        i += 1;
    }
    (out, unresolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(&'static str, usize)> {
        scan(text, "")
            .findings
            .iter()
            .map(|f| (f.kind, f.line))
            .collect()
    }

    #[test]
    fn ut_062_mixed_secret_content_is_blocked_without_echoing_values() {
        let text = "const k = \"sk-abcdefghijklmnopqrstuvwxyz0123\";\napi_key = \"mock-key-test-123\"\n-----BEGIN RSA PRIVATE KEY-----\nAKIAABCDEFGHIJKLMNOP\n";
        let r = scan(text, "");
        assert_eq!(r.highest, "high");
        assert!(r.blocked);
        let k: Vec<_> = r
            .findings
            .iter()
            .map(|f| (f.kind, f.severity, f.line))
            .collect();
        assert_eq!(
            k,
            vec![
                ("api-key", "high", 1),
                ("credential-assignment", "medium", 2),
                ("private-key", "high", 3),
                ("aws-access-key", "high", 4)
            ]
        );
        // Debug output of a finding carries no secret value
        let shown = format!("{:?}", r.findings);
        assert!(!shown.contains("abcdefghijklmnop") && !shown.contains("mock-key"));
    }

    #[test]
    fn ut_062_env_files_are_sensitive_by_name() {
        for n in [
            "config/.env",
            ".env.local",
            "id_rsa",
            "a\\b\\server.pem",
            "secrets.yaml",
            ".npmrc",
        ] {
            assert!(scan("x", n).sensitive_file, "{n}");
        }
        assert!(!scan("x", "src/main.rs").sensitive_file);
        assert_eq!(scan("x", "config/.env").highest, "high");
    }

    #[test]
    fn placeholders_and_env_lookups_are_not_findings() {
        assert!(kinds("password = process.env.PW").is_empty());
        assert!(
            kinds("secret = \"${SECRET}\"\napi_key = \"your_api_key_here\"\npassword: <password>")
                .is_empty()
        );
        assert!(kinds("fn main() {}").is_empty());
    }

    #[test]
    fn token_shapes() {
        assert_eq!(
            kinds(&format!("token = \"ghp_{}\"", "a".repeat(36))),
            vec![("github-token", 1), ("credential-assignment", 1)]
        );
        assert_eq!(
            kinds("Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345"),
            vec![("bearer-token", 1)]
        );
        assert_eq!(
            kinds("url = \"https://user:pass@example.com/x\""),
            vec![("url-credentials", 1)]
        );
        assert_eq!(
            kinds("path = \"/home/bob/project\""),
            vec![("private-path", 1)]
        );
    }
}
