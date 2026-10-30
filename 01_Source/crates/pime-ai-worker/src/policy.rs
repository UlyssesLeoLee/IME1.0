//! What the worker is willing to send and to whom: URL rules, the request-header allow-list, the credential
//! reference grammar and the authentication-header template.
//!
//! Everything here is a pure function over text. The rules are deliberately narrower than a general HTTP client: the
//! worker talks to AI providers over HTTPS and nothing else, so anything unusual (user info in a URL, a numeric host
//! such as `2130706433`, a fragment, a header the caller could use to smuggle a credential) is refused instead of
//! interpreted.
#![forbid(unsafe_code)]

use crate::error::{Code, Failure};
use crate::secret::Secret;

pub const MAX_URL_LEN: usize = 2048;
pub const MAX_PATH_LEN: usize = 2000;
pub const MAX_HEADERS: usize = 16;
pub const MAX_HEADER_VALUE_LEN: usize = 512;
pub const MAX_PROVIDER_LEN: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub https: bool,
    /// Lower-case ASCII host name or dotted IPv4 address.
    pub host: String,
    pub port: u16,
    /// Always starts with `/`; contains the query when there is one; never a fragment.
    pub path_and_query: String,
}

impl Target {
    /// `host:port` with the port always explicit: the form a credential is bound to.
    pub fn origin(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn is_loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }
}

pub fn is_loopback_host(host: &str) -> bool {
    host == "localhost" || (parse_ipv4(host).is_some_and(|ip| ip[0] == 127))
}

/// Strict dotted-quad IPv4: four decimal parts without leading zeros, each at most 255.
fn parse_ipv4(host: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = host.split('.');
    for slot in &mut out {
        let p = parts.next()?;
        if p.is_empty() || p.len() > 3 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if p.len() > 1 && p.starts_with('0') {
            return None;
        }
        *slot = p.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(out)
}

/// Host grammar: ASCII letters, digits and hyphens in dot-separated labels. A host whose last label is numeric must be a
/// strict IPv4 address (so `127.1`, `0x7f.0.0.1` and `2130706433`, which some parsers read as addresses, are refused).
fn valid_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    let last = host.rsplit('.').next().unwrap_or("");
    if !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) {
        return parse_ipv4(host).is_some();
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

fn parse_port(text: &str) -> Option<u16> {
    if text.is_empty() || text.len() > 5 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match text.parse::<u32>() {
        Ok(p) if (1..=65535).contains(&p) => Some(p as u16),
        _ => None,
    }
}

/// `host` or `host:port` -> lower-case host and port (default given by the caller).
fn split_authority(authority: &str, default_port: u16) -> Option<(String, u16)> {
    if authority.is_empty() || authority.contains(['@', '[', ']']) {
        return None;
    }
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, parse_port(p)?),
        None => (authority, default_port),
    };
    let host = host.to_ascii_lowercase();
    valid_host(&host).then_some((host, port))
}

/// Parses an origin as given to `cred.set`: `host` or `host:port` (443 when omitted). No scheme, no path.
pub fn parse_origin(text: &str) -> Option<String> {
    if text.len() > 262 || !text.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return None;
    }
    let (host, port) = split_authority(text, 443)?;
    Some(format!("{host}:{port}"))
}

fn valid_percent_escapes(text: &str) -> bool {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if i + 2 >= b.len() || !(b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit())
            {
                return false;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    true
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UrlPolicy {
    /// Test mode only: plain `http` is accepted for loopback hosts.
    pub allow_loopback_http: bool,
}

/// Validates the request URL. Only `https` is accepted; `http` only for a loopback host when the (test-only) policy
/// allows it. Every violation is `URL_NOT_ALLOWED`, malformed input is `INVALID_REQUEST` with the field `url`.
pub fn parse_url(url: &str, policy: UrlPolicy) -> Result<Target, Failure> {
    if url.is_empty() || url.len() > MAX_URL_LEN || !url.bytes().all(|b| (0x21..=0x7e).contains(&b))
    {
        return Err(Failure::invalid("url"));
    }
    let (https, rest) = if let Some(r) = strip_scheme(url, "https://") {
        (true, r)
    } else if let Some(r) = strip_scheme(url, "http://") {
        (false, r)
    } else {
        return Err(Failure::with_field(Code::UrlNotAllowed, "url"));
    };
    if url.contains(['#', '\\']) {
        return Err(Failure::with_field(Code::UrlNotAllowed, "url"));
    }
    let auth_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(auth_end);
    let default_port = if https { 443 } else { 80 };
    let Some((host, port)) = split_authority(authority, default_port) else {
        return Err(Failure::with_field(Code::UrlNotAllowed, "url"));
    };
    if !https && !(policy.allow_loopback_http && is_loopback_host(&host)) {
        return Err(Failure::with_field(Code::UrlNotAllowed, "url"));
    }
    let path_and_query = if tail.is_empty() {
        "/".to_string()
    } else if tail.starts_with('?') {
        format!("/{tail}")
    } else {
        tail.to_string()
    };
    if path_and_query.len() > MAX_PATH_LEN || !valid_percent_escapes(&path_and_query) {
        return Err(Failure::with_field(Code::UrlNotAllowed, "url"));
    }
    Ok(Target {
        https,
        host,
        port,
        path_and_query,
    })
}

fn strip_scheme<'a>(url: &'a str, scheme: &str) -> Option<&'a str> {
    let head = url.get(..scheme.len())?;
    head.eq_ignore_ascii_case(scheme)
        .then(|| &url[scheme.len()..])
}

// ---------------------------------------------------------------------------------------------------------------------
// headers

/// Headers the caller may set. Everything else is refused: in particular the credential headers (they are only ever
/// produced by the worker from the stored key), `Host`, `Content-Length`, `Cookie` and hop-by-hop headers.
const ALLOWED_REQUEST_HEADERS: [&str; 7] = [
    "content-type",
    "accept",
    "anthropic-version",
    "anthropic-beta",
    "openai-organization",
    "openai-project",
    "x-request-id",
];

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_header_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_HEADER_VALUE_LEN
        && value.bytes().all(|b| (0x20..=0x7e).contains(&b))
        && !value.starts_with(' ')
        && !value.ends_with(' ')
}

/// Validates the caller's headers against the allow-list. Duplicates (case-insensitive) are refused.
pub fn validate_request_headers(
    headers: &[(String, String)],
) -> Result<Vec<(String, String)>, Failure> {
    if headers.len() > MAX_HEADERS {
        return Err(Failure::invalid("headers"));
    }
    let mut seen: Vec<String> = Vec::new();
    for (name, value) in headers {
        if !valid_header_name(name) || !valid_header_value(value) {
            return Err(Failure::invalid("headers"));
        }
        let lower = name.to_ascii_lowercase();
        if !ALLOWED_REQUEST_HEADERS.contains(&lower.as_str()) {
            return Err(Failure::with_field(Code::HeaderNotAllowed, "headers"));
        }
        if seen.contains(&lower) {
            return Err(Failure::invalid("headers"));
        }
        seen.push(lower);
    }
    Ok(headers.to_vec())
}

/// Response headers that are passed to the caller (lower-case names). The list is short on purpose: what the product
/// reads (`retry-after` for the cool-down), what helps support (request ids) and the rate-limit family.
pub fn response_header_allowed(lower_name: &str) -> bool {
    matches!(
        lower_name,
        "content-type" | "retry-after" | "retry-after-ms" | "x-request-id" | "request-id"
    ) || lower_name.starts_with("x-ratelimit-")
        || lower_name.starts_with("anthropic-ratelimit-")
}

// ---------------------------------------------------------------------------------------------------------------------
// credential references and the authentication template

/// A provider id: the part of the Credential Manager target name after the namespace.
pub fn valid_provider_ref(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_PROVIDER_LEN
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `<Header-Name>: <prefix>{key}<suffix>`, for example `Authorization: Bearer {key}` or `x-api-key: {key}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthTemplate {
    /// The header name as written in the template.
    name: String,
    prefix: String,
    suffix: String,
}

const KEY_PLACEHOLDER: &str = "{key}";
/// The only headers a stored key can be injected into.
const AUTH_HEADER_NAMES: [&str; 2] = ["authorization", "x-api-key"];

fn valid_template_part(text: &str) -> bool {
    text.len() <= 32
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'-' | b'_' | b'.'))
}

impl AuthTemplate {
    pub fn parse(template: &str) -> Result<AuthTemplate, Failure> {
        let bad = || Failure::invalid("authHeader");
        let (name, value) = template.split_once(':').ok_or_else(bad)?;
        let name = name.trim();
        if !AUTH_HEADER_NAMES.contains(&name.to_ascii_lowercase().as_str()) {
            return Err(Failure::with_field(Code::HeaderNotAllowed, "authHeader"));
        }
        let value = value.trim_start();
        if value.matches(KEY_PLACEHOLDER).count() != 1 {
            return Err(bad());
        }
        let (prefix, suffix) = value.split_once(KEY_PLACEHOLDER).ok_or_else(bad)?;
        if !valid_template_part(prefix) || !valid_template_part(suffix) {
            return Err(bad());
        }
        Ok(AuthTemplate {
            name: name.to_string(),
            prefix: prefix.to_string(),
            suffix: suffix.to_string(),
        })
    }

    /// The complete header line `Name: <prefix><key><suffix>` (no line terminator) as a wipe-on-drop buffer, built with
    /// its exact capacity so no earlier, smaller copy of the key is left behind by a reallocation.
    pub fn render(&self, key: &Secret) -> Secret {
        let mut line = Vec::with_capacity(
            self.name.len() + 2 + self.prefix.len() + key.len() + self.suffix.len(),
        );
        line.extend_from_slice(self.name.as_bytes());
        line.extend_from_slice(b": ");
        line.extend_from_slice(self.prefix.as_bytes());
        line.extend_from_slice(key.expose());
        line.extend_from_slice(self.suffix.as_bytes());
        Secret::from_bytes(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn https_only() -> UrlPolicy {
        UrlPolicy::default()
    }

    fn test_http() -> UrlPolicy {
        UrlPolicy {
            allow_loopback_http: true,
        }
    }

    #[test]
    fn accepts_the_provider_endpoints_the_product_uses() {
        let t = parse_url("https://api.openai.com/v1/responses", https_only()).unwrap();
        assert_eq!(
            (t.https, t.host.as_str(), t.port, t.path_and_query.as_str()),
            (true, "api.openai.com", 443, "/v1/responses")
        );
        let t = parse_url("HTTPS://API.Anthropic.COM:443/v1/messages?beta=1", https_only()).unwrap();
        assert_eq!(t.host, "api.anthropic.com");
        assert_eq!(t.origin(), "api.anthropic.com:443");
        assert_eq!(t.path_and_query, "/v1/messages?beta=1");
        let t = parse_url("https://127.0.0.1:8443", https_only()).unwrap();
        assert_eq!(t.path_and_query, "/");
        assert!(t.is_loopback());
        let t = parse_url("https://host.example?x=1", https_only()).unwrap();
        assert_eq!(t.path_and_query, "/?x=1");
        let t = parse_url("https://a-b.c1.example/%7Euser/x%20y", https_only()).unwrap();
        assert_eq!(t.path_and_query, "/%7Euser/x%20y");
    }

    #[test]
    fn refuses_everything_that_is_not_plain_https_to_a_named_host() {
        for url in [
            "http://api.openai.com/v1",
            "ftp://api.openai.com/",
            "file:///C:/secret",
            "//api.openai.com/v1",
            "api.openai.com/v1",
            "https://user@api.openai.com/",
            "https://user:pw@api.openai.com/",
            "https://api.openai.com#frag",
            "https://api.openai.com/a#frag",
            "https://api.openai.com\\@evil.example/",
            "https://api.openai.com:0/",
            "https://api.openai.com:65536/",
            "https://api.openai.com:/",
            "https://api.openai.com:44:3/",
            "https://[::1]/",
            "https://2130706433/",
            "https://0x7f.0.0.1/",
            "https://127.1/",
            "https://127.000.000.001/",
            "https://256.1.1.1/",
            "https://-bad.example/",
            "https://bad-.example/",
            "https://a..b/",
            "https://api.openai.com./",
            "https://exa mple.com/",
            "https://caf\u{e9}.example/",
            "https:///path",
            "https://",
            "https://api.openai.com/%zz",
            "https://api.openai.com/%4",
            "https://api.openai.com/%",
        ] {
            assert!(parse_url(url, https_only()).is_err(), "{url}");
        }
        let long = format!("https://api.openai.com/{}", "a".repeat(MAX_URL_LEN));
        assert!(parse_url(&long, https_only()).is_err());
        assert_eq!(parse_url("", https_only()).unwrap_err().code, Code::InvalidRequest);
    }

    #[test]
    fn plain_http_needs_the_test_policy_and_a_loopback_host() {
        for url in ["http://127.0.0.1:9000/x", "http://localhost/x", "HTTP://127.5.5.5/"] {
            assert!(parse_url(url, https_only()).is_err(), "{url}");
            assert!(parse_url(url, test_http()).is_ok(), "{url}");
        }
        for url in ["http://api.openai.com/x", "http://192.168.1.5/x", "http://10.0.0.1/"] {
            let e = parse_url(url, test_http()).unwrap_err();
            assert_eq!(e.code, Code::UrlNotAllowed, "{url}");
        }
        assert_eq!(parse_url("http://127.0.0.1/", test_http()).unwrap().port, 80);
    }

    #[test]
    fn violations_are_url_not_allowed_and_malformed_input_is_invalid_request() {
        let e = parse_url("https://user@host.example/", https_only()).unwrap_err();
        assert_eq!((e.code, e.field.as_deref()), (Code::UrlNotAllowed, Some("url")));
        let e = parse_url("https://host.example/ x", https_only()).unwrap_err();
        assert_eq!((e.code, e.field.as_deref()), (Code::InvalidRequest, Some("url")));
    }

    #[test]
    fn loopback_detection_is_exact() {
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.200.3.4"));
        assert!(!is_loopback_host("128.0.0.1"));
        assert!(!is_loopback_host("localhost.example"));
        assert!(!is_loopback_host("127.0.0.1.example"));
        assert!(!is_loopback_host("0.0.0.0"));
    }

    #[test]
    fn origins_are_canonicalised_for_credential_binding() {
        assert_eq!(parse_origin("API.OpenAI.com").as_deref(), Some("api.openai.com:443"));
        assert_eq!(parse_origin("api.openai.com:443").as_deref(), Some("api.openai.com:443"));
        assert_eq!(parse_origin("127.0.0.1:8443").as_deref(), Some("127.0.0.1:8443"));
        for bad in [
            "",
            "https://api.openai.com",
            "api.openai.com/v1",
            "user@api.openai.com",
            "api.openai.com:",
            "api.openai.com:99999",
            "[::1]:443",
            "bad host",
        ] {
            assert_eq!(parse_origin(bad), None, "{bad}");
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn allow_listed_headers_pass() {
        let h = headers(&[
            ("Content-Type", "application/json"),
            ("anthropic-version", "2026-10-21"),
            ("Accept", "application/json"),
        ]);
        assert_eq!(validate_request_headers(&h).unwrap(), h);
    }

    #[test]
    fn credential_and_transport_headers_are_not_the_callers_to_set() {
        for name in [
            "Authorization",
            "authorization",
            "x-api-key",
            "api-key",
            "Proxy-Authorization",
            "Cookie",
            "Host",
            "Content-Length",
            "Transfer-Encoding",
            "Connection",
            "X-Forwarded-For",
            "Expect",
        ] {
            let e = validate_request_headers(&headers(&[(name, "x")])).unwrap_err();
            assert_eq!(e.code, Code::HeaderNotAllowed, "{name}");
        }
    }

    #[test]
    fn malformed_headers_are_invalid_requests() {
        for (name, value) in [
            ("Content-Type", "a\r\nX-Evil: 1"),
            ("Content-Type", "a\nb"),
            ("Content-Type", "a\0b"),
            ("Content-Type", ""),
            ("Content-Type", " padded"),
            ("Content-Type", "caf\u{e9}"),
            ("Content-Type\r\nX", "a"),
            ("Content Type", "a"),
            ("", "a"),
        ] {
            let e = validate_request_headers(&headers(&[(name, value)])).unwrap_err();
            assert_eq!(e.code, Code::InvalidRequest, "{name:?}={value:?}");
        }
        let dup = headers(&[("Content-Type", "a/b"), ("content-type", "a/b")]);
        assert_eq!(validate_request_headers(&dup).unwrap_err().code, Code::InvalidRequest);
        let many: Vec<(String, String)> = (0..=MAX_HEADERS)
            .map(|_| ("Accept".to_string(), "x".to_string()))
            .collect();
        assert!(validate_request_headers(&many).is_err());
        let value = "a".repeat(MAX_HEADER_VALUE_LEN + 1);
        let long = headers(&[("Content-Type", value.as_str())]);
        assert!(validate_request_headers(&long).is_err());
    }

    #[test]
    fn response_headers_are_an_allow_list() {
        for ok in [
            "content-type",
            "retry-after",
            "x-request-id",
            "request-id",
            "x-ratelimit-remaining-requests",
            "anthropic-ratelimit-requests-reset",
        ] {
            assert!(response_header_allowed(ok), "{ok}");
        }
        for no in [
            "set-cookie",
            "www-authenticate",
            "location",
            "authorization",
            "proxy-authenticate",
            "server",
            "x-powered-by",
        ] {
            assert!(!response_header_allowed(no), "{no}");
        }
    }

    #[test]
    fn provider_refs_are_plain_identifiers() {
        for ok in ["openai", "anthropic", "my-local.server_1", "A"] {
            assert!(valid_provider_ref(ok), "{ok}");
        }
        let long = "a".repeat(MAX_PROVIDER_LEN + 1);
        for bad in ["", "a/b", "a\\b", "a b", "a*", "a:b", "..\\x", "caf\u{e9}", long.as_str()] {
            assert!(!valid_provider_ref(bad), "{bad}");
        }
    }

    fn key() -> Secret {
        Secret::parse("mock-key-test-123".to_string()).unwrap()
    }

    #[test]
    fn templates_inject_the_key_into_the_two_credential_headers() {
        let t = AuthTemplate::parse("Authorization: Bearer {key}").unwrap();
        assert_eq!(t.render(&key()).expose(), b"Authorization: Bearer mock-key-test-123");
        let t = AuthTemplate::parse("x-api-key: {key}").unwrap();
        assert_eq!(t.render(&key()).expose(), b"x-api-key: mock-key-test-123");
        let t = AuthTemplate::parse("X-API-Key:{key}").unwrap();
        assert_eq!(t.render(&key()).expose(), b"X-API-Key: mock-key-test-123");
        let t = AuthTemplate::parse("authorization: Token {key}").unwrap();
        assert_eq!(t.render(&key()).expose(), b"authorization: Token mock-key-test-123");
    }

    #[test]
    fn templates_are_strict() {
        for bad in [
            "",
            "Authorization",
            "Authorization: Bearer",
            "Authorization: Bearer {key} {key}",
            "Authorization: {KEY}",
            "Authorization: Bearer {key}\r\nX-Evil: 1",
            "Authorization: Bearer {key}\nX-Evil: 1",
            "Authorization: Bearer {{key}}",
            "Authorization: Bearer {key} {other}",
            "Authorization: Bearer ${key}",
            "Authorization: a=b{key}",
            "Authorization: ",
        ] {
            assert!(AuthTemplate::parse(bad).is_err(), "{bad:?}");
        }
        // the key can only ever go into a credential header
        for name in ["Host", "Cookie", "Content-Type", "X-Custom", "Proxy-Authorization"] {
            let e = AuthTemplate::parse(&format!("{name}: {{key}}")).unwrap_err();
            assert_eq!(e.code, Code::HeaderNotAllowed, "{name}");
        }
        let long = format!("Authorization: {}{{key}}", "a".repeat(33));
        assert!(AuthTemplate::parse(&long).is_err());
    }

    #[test]
    fn a_rendered_line_has_exactly_the_capacity_it_needs() {
        let t = AuthTemplate::parse("Authorization: Bearer {key}").unwrap();
        let line = t.render(&key());
        assert_eq!(line.capacity(), line.len(), "no spare capacity, so no stale copy");
    }
}
