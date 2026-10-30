//! The `PIME-IPC/1` handshake: protocol version negotiation, mutual challenge-response with HMAC-SHA-256, and the
//! per-connection capability token. Generic over [`Transport`], so it is tested without Windows.
//!
//! ```text
//! client                                                   server
//!   hello     {major, minor, role, caps, cnonce}  ----->   version check, role lookup, replay pre-check
//!             <-----  challenge {major, minor, snonce, proof = HMAC(Krole, "server-proof" | binding)}
//!   (verifies the server's proof; a squatter without the key stops here, before the client reveals anything)
//!   auth      {proof = HMAC(Krole, "client-proof" | binding)}  ----->   constant-time check, nonce recorded
//!             <-----  ready {session, token, caps, ttl_ms, mac = HMAC(session_key, ... )}
//! ```
//!
//! `Krole` is derived from the per-user connection key and the role name; `binding` covers both nonces, the role, the
//! requested capabilities and both versions (see `auth::Binding`). Every failure closes the exchange; the only things
//! sent back are coarse error codes in the `{"ok":false,"error":"CODE"}` shape of the JSON Lines protocol.

use crate::auth::{
    Binding, Entropy, MonoClock, NONCE_BYTES, NonceWindow, PROTOCOL_VERSION, Secret, TOKEN_BYTES,
    Version, ct_eq, derive_role_key, random_hex, ready_mac,
};
use crate::error::IpcError;
use crate::frame::{self, FrameClass, Limits, Transport};
use pime_core::json::{self, Json};
use pime_core::sha256::{from_hex, to_hex};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_CAPS: usize = 16;
const MAX_NAME: usize = 48;

/// Role and capability names: short, lowercase, no whitespace or separators that could confuse a log or a transcript.
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b':' | b'-')
        })
}

/// What one client class (TSF, bridge, UI, worker...) may ask the server for (DD 2.1: independent capability scope).
#[derive(Clone, Debug)]
pub struct RolePolicy {
    pub role: String,
    pub capabilities: Vec<String>,
}

/// Server-side rules of the handshake.
pub struct ServerPolicy {
    master_key: Secret,
    roles: Vec<RolePolicy>,
    /// Version this server speaks (default `PIME-IPC/1.0`); tests lower or change it.
    pub version: Version,
    /// Lifetime of the capability token (DD 2.1: "短时范围令牌").
    pub token_ttl: Duration,
    /// How long a challenge may wait for its answer; older answers are refused.
    pub challenge_ttl: Duration,
    pub nonce_window: Duration,
    pub nonce_capacity: usize,
}

impl ServerPolicy {
    pub fn new(master_key: Secret, roles: Vec<RolePolicy>) -> Result<ServerPolicy, IpcError> {
        if roles.is_empty() {
            return Err(IpcError::InvalidArgument("at least one role is required"));
        }
        for r in &roles {
            if !valid_name(&r.role)
                || r.capabilities.len() > MAX_CAPS
                || !r.capabilities.iter().all(|c| valid_name(c))
            {
                return Err(IpcError::InvalidArgument("invalid role or capability name"));
            }
        }
        Ok(ServerPolicy {
            master_key,
            roles,
            version: PROTOCOL_VERSION,
            token_ttl: Duration::from_secs(15 * 60),
            challenge_ttl: Duration::from_secs(1),
            nonce_window: Duration::from_secs(120),
            nonce_capacity: 65_536,
        })
    }

    fn role(&self, name: &str) -> Option<&RolePolicy> {
        self.roles.iter().find(|r| r.role == name)
    }

    /// The key the installer would hand to clients of `role`.
    pub fn role_key(&self, role: &str) -> Secret {
        derive_role_key(&self.master_key, role)
    }
}

/// A [`ServerPolicy`] plus the replay window that every connection of this server shares.
pub struct ServerState {
    pub policy: ServerPolicy,
    nonces: Mutex<NonceWindow>,
    clock: MonoClock,
}

impl ServerState {
    pub fn new(policy: ServerPolicy) -> ServerState {
        let nonces = NonceWindow::new(policy.nonce_window, policy.nonce_capacity);
        ServerState {
            policy,
            nonces: Mutex::new(nonces),
            clock: MonoClock::new(),
        }
    }

    fn with_nonces<R>(&self, f: impl FnOnce(&mut NonceWindow, u64) -> R) -> R {
        // a panic elsewhere must not turn every later handshake into a panic: the table is still consistent
        let mut guard = self.nonces.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard, self.clock.now_ms())
    }
}

/// What a client presents.
#[derive(Clone, Debug)]
pub struct ClientCredentials {
    pub role: String,
    /// The key of this role (`ServerPolicy::role_key`), not the master key.
    pub role_key: Secret,
    /// Requested capabilities; empty asks for everything the role may have.
    pub capabilities: Vec<String>,
    pub version: Version,
}

impl ClientCredentials {
    pub fn new(role: &str, role_key: Secret, capabilities: &[&str]) -> ClientCredentials {
        ClientCredentials {
            role: role.to_string(),
            role_key,
            capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
            version: PROTOCOL_VERSION,
        }
    }
}

/// The outcome of a successful handshake: who the peer is and what it may do on this connection.
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub version: Version,
    pub role: String,
    pub session: String,
    pub capabilities: Vec<String>,
    token: String,
    token_expires: Instant,
}

impl SessionInfo {
    /// The capability token of this connection (hex). Opaque, random, valid for the server's `token_ttl`.
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }

    pub fn token_expired(&self) -> bool {
        Instant::now() >= self.token_expires
    }

    /// Constant-time check of a token a message carries against this connection's token, and that it has not expired.
    pub fn verify_token(&self, presented: &str) -> bool {
        !self.token_expired() && ct_eq(presented.as_bytes(), self.token.as_bytes())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// message helpers

fn parse_message(payload: &[u8]) -> Result<Json, IpcError> {
    let text =
        std::str::from_utf8(payload).map_err(|_| IpcError::Malformed("handshake is not UTF-8"))?;
    let limits = json::Limits {
        max_bytes: frame::HANDSHAKE_FRAME_MAX,
        max_depth: 4,
    };
    let value =
        json::parse(text, limits).map_err(|_| IpcError::Malformed("handshake is not JSON"))?;
    if value.as_object().is_none() {
        return Err(IpcError::Malformed("handshake is not a JSON object"));
    }
    Ok(value)
}

fn str_field<'a>(msg: &'a Json, key: &'static str) -> Result<&'a str, IpcError> {
    msg.get(key)
        .and_then(Json::as_str)
        .ok_or(IpcError::Malformed(
            "handshake field missing or not a string",
        ))
}

fn u16_field(msg: &Json, key: &'static str) -> Result<u16, IpcError> {
    msg.get(key)
        .and_then(Json::as_i64)
        .and_then(|n| u16::try_from(n).ok())
        .ok_or(IpcError::Malformed("handshake version field invalid"))
}

fn hex_field<const N: usize>(msg: &Json, key: &'static str) -> Result<[u8; N], IpcError> {
    let s = str_field(msg, key)?;
    if s.len() != N * 2 {
        return Err(IpcError::Malformed(
            "handshake hex field has the wrong length",
        ));
    }
    let bytes = from_hex(s).ok_or(IpcError::Malformed("handshake hex field is not hex"))?;
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn caps_field(msg: &Json, key: &'static str) -> Result<Vec<String>, IpcError> {
    let arr = msg
        .get(key)
        .and_then(Json::as_array)
        .ok_or(IpcError::Malformed("handshake capabilities missing"))?;
    if arr.len() > MAX_CAPS {
        return Err(IpcError::Malformed("too many capabilities"));
    }
    let mut out: Vec<String> = Vec::with_capacity(arr.len());
    for item in arr {
        let name = item
            .as_str()
            .filter(|s| valid_name(s))
            .ok_or(IpcError::Malformed("invalid capability name"))?;
        if out.iter().any(|c| c == name) {
            return Err(IpcError::Malformed("duplicate capability"));
        }
        out.push(name.to_string());
    }
    Ok(out)
}

fn send_msg<T: Transport + ?Sized>(
    t: &mut T,
    msg: &Json,
    deadline: Instant,
) -> Result<(), IpcError> {
    frame::write_frame(
        t,
        FrameClass::Control,
        msg.to_json_string().as_bytes(),
        &Limits::HANDSHAKE,
        deadline,
    )
}

fn recv_msg<T: Transport + ?Sized>(t: &mut T, deadline: Instant) -> Result<Json, IpcError> {
    let mut consumed = false;
    let (_, payload) = frame::read_frame(t, &Limits::HANDSHAKE, deadline, &mut consumed)?;
    parse_message(&payload)
}

/// The error vocabulary on the wire. Only failures the peer may learn about are reported, and identity failures are
/// all the same code, so a prober cannot tell a wrong role from a wrong proof or a replay.
fn wire_code(e: &IpcError) -> Option<&'static str> {
    match e {
        IpcError::Timeout
        | IpcError::Closed
        | IpcError::Truncated
        | IpcError::Os { .. }
        | IpcError::Broken
        | IpcError::Stopped => None,
        other => Some(other.code()),
    }
}

/// Best effort, short deadline, errors ignored: the connection is being closed anyway.
pub(crate) fn send_error<T: Transport + ?Sized>(
    t: &mut T,
    e: &IpcError,
    supported: Option<Version>,
) {
    let Some(code) = wire_code(e) else { return };
    let mut pairs = vec![("ok", Json::Bool(false)), ("error", Json::str(code))];
    if let (IpcError::ProtocolMismatch, Some(v)) = (e, supported) {
        pairs.push(("major", Json::int(i64::from(v.major))));
        pairs.push(("minor", Json::int(i64::from(v.minor))));
    }
    let _ = send_msg(
        t,
        &Json::obj(pairs),
        Instant::now() + Duration::from_millis(200),
    );
}

/// A server error frame as the client sees it (`{"ok":false,"error":...}`) mapped back to the error type.
fn server_error(msg: &Json) -> Option<IpcError> {
    if msg.get("ok").and_then(Json::as_bool) != Some(false) {
        return None;
    }
    Some(match msg.get("error").and_then(Json::as_str) {
        Some("PROTOCOL_MISMATCH") => IpcError::ProtocolMismatch,
        Some("IDENTITY_REJECTED") => IpcError::IdentityRejected,
        Some("CAPABILITY_DENIED") => IpcError::CapabilityDenied,
        _ => IpcError::Malformed("the server refused the handshake"),
    })
}

fn expect_type(msg: &Json, want: &'static str) -> Result<(), IpcError> {
    if str_field(msg, "t")? == want {
        Ok(())
    } else {
        Err(IpcError::Malformed("unexpected handshake message"))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// server

/// Runs the server side. On any failure a coarse error frame is sent (best effort) and the error is returned; the
/// caller must drop the connection.
pub fn server_handshake<T: Transport + ?Sized>(
    t: &mut T,
    state: &ServerState,
    entropy: &mut dyn Entropy,
    deadline: Instant,
) -> Result<SessionInfo, IpcError> {
    match server_exchange(t, state, entropy, deadline) {
        Ok(info) => Ok(info),
        Err(e) => {
            send_error(t, &e, Some(state.policy.version));
            Err(e)
        }
    }
}

fn server_exchange<T: Transport + ?Sized>(
    t: &mut T,
    state: &ServerState,
    entropy: &mut dyn Entropy,
    deadline: Instant,
) -> Result<SessionInfo, IpcError> {
    let policy = &state.policy;
    // 1. hello. Validation order follows DD 2.2: length (frame layer), version, identity.
    let hello = recv_msg(t, deadline)?;
    expect_type(&hello, "hello")?;
    let client_version = Version {
        major: u16_field(&hello, "major")?,
        minor: u16_field(&hello, "minor")?,
    };
    let version = Version::negotiate(client_version, policy.version)?;
    let role_name = str_field(&hello, "role")?;
    let requested = caps_field(&hello, "caps")?;
    let client_nonce: [u8; NONCE_BYTES] = hex_field(&hello, "cnonce")?;
    let role = policy.role(role_name).ok_or(IpcError::IdentityRejected)?;
    if state.with_nonces(|w, now| w.contains(&client_nonce, now)) {
        return Err(IpcError::Replay);
    }
    let role_key = policy.role_key(&role.role);

    // 2. challenge, with the proof of the server
    let mut server_nonce = [0u8; NONCE_BYTES];
    entropy.fill(&mut server_nonce)?;
    let binding = Binding {
        role: &role.role,
        requested_caps: &requested,
        client_version,
        version,
        client_nonce: &client_nonce,
        server_nonce: &server_nonce,
    };
    let challenge = Json::obj(vec![
        ("t", Json::str("challenge")),
        ("major", Json::int(i64::from(version.major))),
        ("minor", Json::int(i64::from(version.minor))),
        ("snonce", Json::Str(to_hex(&server_nonce))),
        ("proof", Json::Str(to_hex(&binding.server_proof(&role_key)))),
    ]);
    send_msg(t, &challenge, deadline)?;
    let issued = Instant::now();

    // 3. the client's proof
    let auth = recv_msg(t, deadline)?;
    expect_type(&auth, "auth")?;
    let presented: [u8; 32] = hex_field(&auth, "proof")?;
    if issued.elapsed() > policy.challenge_ttl {
        return Err(IpcError::StaleChallenge);
    }
    if !ct_eq(&binding.client_proof(&role_key), &presented) {
        return Err(IpcError::IdentityRejected);
    }
    // recorded only for authenticated handshakes, so strangers cannot fill the table; atomic against a concurrent replay
    state.with_nonces(|w, now| w.check_and_insert(&client_nonce, now))?;

    // 4. capabilities: nothing beyond the role (an explicit over-ask is refused, not silently trimmed)
    let granted = if requested.is_empty() {
        role.capabilities.clone()
    } else if requested.iter().all(|c| role.capabilities.contains(c)) {
        requested.clone()
    } else {
        return Err(IpcError::CapabilityDenied);
    };

    // 5. ready
    let session = random_hex(entropy, TOKEN_BYTES)?;
    let token = random_hex(entropy, TOKEN_BYTES)?;
    let ttl_ms = policy.token_ttl.as_millis().min(u128::from(u32::MAX)) as u64;
    let mac = ready_mac(
        &binding.session_key(&role_key),
        &session,
        &token,
        &granted,
        ttl_ms,
    );
    let ready = Json::obj(vec![
        ("t", Json::str("ready")),
        ("session", Json::Str(session.clone())),
        ("token", Json::Str(token.clone())),
        ("caps", Json::strs(&granted)),
        ("ttl_ms", Json::Num(ttl_ms as f64)),
        ("mac", Json::Str(to_hex(&mac))),
    ]);
    send_msg(t, &ready, deadline)?;
    Ok(SessionInfo {
        version,
        role: role.role.clone(),
        session,
        capabilities: granted,
        token,
        token_expires: frame::deadline_after(Duration::from_millis(ttl_ms)),
    })
}

// ---------------------------------------------------------------------------------------------------------------
// client

/// Runs the client side; mutual: the server must prove it holds the key before the client answers.
pub fn client_handshake<T: Transport + ?Sized>(
    t: &mut T,
    creds: &ClientCredentials,
    entropy: &mut dyn Entropy,
    deadline: Instant,
) -> Result<SessionInfo, IpcError> {
    if !valid_name(&creds.role)
        || creds.capabilities.len() > MAX_CAPS
        || !creds.capabilities.iter().all(|c| valid_name(c))
    {
        return Err(IpcError::InvalidArgument("invalid role or capability name"));
    }
    let mut client_nonce = [0u8; NONCE_BYTES];
    entropy.fill(&mut client_nonce)?;
    let hello = Json::obj(vec![
        ("t", Json::str("hello")),
        ("major", Json::int(i64::from(creds.version.major))),
        ("minor", Json::int(i64::from(creds.version.minor))),
        ("role", Json::str(&creds.role)),
        ("caps", Json::strs(&creds.capabilities)),
        ("cnonce", Json::Str(to_hex(&client_nonce))),
    ]);
    send_msg(t, &hello, deadline)?;

    let challenge = recv_msg(t, deadline)?;
    if let Some(e) = server_error(&challenge) {
        return Err(e);
    }
    expect_type(&challenge, "challenge")?;
    let version = Version {
        major: u16_field(&challenge, "major")?,
        minor: u16_field(&challenge, "minor")?,
    };
    if version.major != creds.version.major || version.minor > creds.version.minor {
        return Err(IpcError::ProtocolMismatch);
    }
    let server_nonce: [u8; NONCE_BYTES] = hex_field(&challenge, "snonce")?;
    let server_proof: [u8; 32] = hex_field(&challenge, "proof")?;
    let binding = Binding {
        role: &creds.role,
        requested_caps: &creds.capabilities,
        client_version: creds.version,
        version,
        client_nonce: &client_nonce,
        server_nonce: &server_nonce,
    };
    if !ct_eq(&binding.server_proof(&creds.role_key), &server_proof) {
        // not the server we share the key with (a squatter, or the wrong user's endpoint): reveal nothing, stop here
        return Err(IpcError::IdentityRejected);
    }
    send_msg(
        t,
        &Json::obj(vec![
            ("t", Json::str("auth")),
            (
                "proof",
                Json::Str(to_hex(&binding.client_proof(&creds.role_key))),
            ),
        ]),
        deadline,
    )?;

    let ready = recv_msg(t, deadline)?;
    if let Some(e) = server_error(&ready) {
        return Err(e);
    }
    expect_type(&ready, "ready")?;
    let session = str_field(&ready, "session")?.to_string();
    let token = str_field(&ready, "token")?.to_string();
    let caps = caps_field(&ready, "caps")?;
    let ttl_ms = ready
        .get("ttl_ms")
        .and_then(Json::as_i64)
        .and_then(|n| u64::try_from(n).ok())
        .filter(|&n| n <= u64::from(u32::MAX))
        .ok_or(IpcError::Malformed("handshake ttl invalid"))?;
    let mac: [u8; 32] = hex_field(&ready, "mac")?;
    if session.len() != TOKEN_BYTES * 2 || token.len() != TOKEN_BYTES * 2 {
        return Err(IpcError::Malformed("session or token has the wrong length"));
    }
    let expected = ready_mac(
        &binding.session_key(&creds.role_key),
        &session,
        &token,
        &caps,
        ttl_ms,
    );
    if !ct_eq(&expected, &mac) {
        return Err(IpcError::IdentityRejected);
    }
    if !creds.capabilities.is_empty() && caps != creds.capabilities {
        return Err(IpcError::CapabilityDenied);
    }
    Ok(SessionInfo {
        version,
        role: creds.role.clone(),
        session,
        capabilities: caps,
        token,
        token_expires: frame::deadline_after(Duration::from_millis(ttl_ms)),
    })
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    /// A session as a finished handshake would produce it, for tests of the layers above.
    pub fn session() -> SessionInfo {
        SessionInfo {
            version: PROTOCOL_VERSION,
            role: "tsf".to_string(),
            session: "0".repeat(32),
            capabilities: Vec::new(),
            token: "1".repeat(32),
            token_expires: Instant::now() + Duration::from_secs(60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{CountingEntropy, MemEnd, far, mem_pair};
    use std::sync::Arc;
    use std::thread;

    fn master() -> Secret {
        Secret::new(&[0x42; 32]).unwrap()
    }

    fn state() -> Arc<ServerState> {
        let roles = vec![
            RolePolicy {
                role: "tsf".into(),
                capabilities: vec!["predict.local".into(), "cancel".into()],
            },
            RolePolicy {
                role: "bridge".into(),
                capabilities: vec!["snapshot".into(), "edit".into()],
            },
        ];
        Arc::new(ServerState::new(
            ServerPolicy::new(master(), roles).unwrap(),
        ))
    }

    fn creds(st: &ServerState, role: &str, caps: &[&str]) -> ClientCredentials {
        ClientCredentials::new(role, st.policy.role_key(role), caps)
    }

    /// Runs both sides over an in-memory pair. Returns (server result, client result).
    fn run(
        st: &Arc<ServerState>,
        c: &ClientCredentials,
        seed: u8,
    ) -> (Result<SessionInfo, IpcError>, Result<SessionInfo, IpcError>) {
        let (mut a, mut b) = mem_pair();
        let st2 = Arc::clone(st);
        let server = thread::spawn(move || {
            server_handshake(
                &mut a,
                &st2,
                &mut CountingEntropy::new(seed.wrapping_add(100)),
                far(),
            )
        });
        let client = client_handshake(&mut b, c, &mut CountingEntropy::new(seed), far());
        drop(b);
        (server.join().unwrap(), client)
    }

    #[test]
    fn it_003_same_user_key_succeeds_and_both_ends_agree() {
        let st = state();
        let (s, c) = run(&st, &creds(&st, "tsf", &["predict.local"]), 1);
        let (s, c) = (s.unwrap(), c.unwrap());
        assert_eq!(s.role, "tsf");
        assert_eq!(s.capabilities, vec!["predict.local".to_string()]);
        assert_eq!(s.session, c.session);
        assert_eq!(s.token(), c.token());
        assert_eq!(s.token().len(), 32);
        assert!(s.verify_token(c.token()));
        assert!(!s.verify_token("0".repeat(32).as_str()));
        assert!(!s.verify_token(""));
        assert_eq!(s.version, PROTOCOL_VERSION);
    }

    #[test]
    fn it_003_capability_token_is_short_lived_and_unique_per_connection() {
        let roles = vec![RolePolicy {
            role: "tsf".into(),
            capabilities: vec![],
        }];
        let mut policy = ServerPolicy::new(master(), roles).unwrap();
        policy.token_ttl = Duration::from_millis(80);
        let st = Arc::new(ServerState::new(policy));
        let (s1, _) = run(&st, &creds(&st, "tsf", &[]), 70);
        let (s2, _) = run(&st, &creds(&st, "tsf", &[]), 71);
        let (s1, s2) = (s1.unwrap(), s2.unwrap());
        assert_ne!(s1.token(), s2.token());
        assert_ne!(s1.session, s2.session);
        assert!(s1.verify_token(s1.token()) && !s1.verify_token(s2.token()));
        thread::sleep(Duration::from_millis(160));
        assert!(s1.token_expired());
        assert!(
            !s1.verify_token(s1.token()),
            "an expired token is refused even if it is the right one"
        );
    }

    #[test]
    fn it_003_no_requested_capabilities_means_the_whole_role() {
        let st = state();
        let (s, c) = run(&st, &creds(&st, "bridge", &[]), 2);
        assert_eq!(
            s.unwrap().capabilities,
            vec!["snapshot".to_string(), "edit".to_string()]
        );
        assert_eq!(
            c.unwrap().capabilities,
            vec!["snapshot".to_string(), "edit".to_string()]
        );
    }

    #[test]
    fn it_003_wrong_key_is_rejected_and_the_server_says_only_identity_rejected() {
        let st = state();
        let mut c = creds(&st, "tsf", &[]);
        c.role_key = derive_role_key(&Secret::new(&[0x43; 32]).unwrap(), "tsf");
        let (s, cl) = run(&st, &c, 3);
        // the client does not even answer: the server's proof does not verify against the wrong key
        assert_eq!(cl.unwrap_err(), IpcError::IdentityRejected);
        assert!(s.is_err());
    }

    #[test]
    fn it_003_a_role_key_cannot_be_used_for_another_role() {
        let st = state();
        let mut c = creds(&st, "tsf", &[]);
        c.role = "bridge".into(); // tsf's key, bridge's name
        let (s, cl) = run(&st, &c, 4);
        assert_eq!(cl.unwrap_err(), IpcError::IdentityRejected);
        assert!(s.is_err());
    }

    #[test]
    fn it_003_unknown_role_is_rejected() {
        let st = state();
        let c = ClientCredentials::new("admin", st.policy.role_key("admin"), &[]);
        let (s, cl) = run(&st, &c, 5);
        assert_eq!(s.unwrap_err(), IpcError::IdentityRejected);
        assert_eq!(cl.unwrap_err(), IpcError::IdentityRejected);
    }

    #[test]
    fn it_003_capability_the_role_does_not_have_is_denied() {
        let st = state();
        let (s, cl) = run(&st, &creds(&st, "tsf", &["snapshot"]), 6);
        assert_eq!(s.unwrap_err(), IpcError::CapabilityDenied);
        assert_eq!(cl.unwrap_err(), IpcError::CapabilityDenied);
    }

    #[test]
    fn ut_ipc_different_major_version_is_protocol_mismatch_and_reports_the_supported_one() {
        let st = state();
        let mut c = creds(&st, "tsf", &[]);
        c.version = Version { major: 2, minor: 0 };
        let (s, cl) = run(&st, &c, 7);
        assert_eq!(s.unwrap_err(), IpcError::ProtocolMismatch);
        assert_eq!(cl.unwrap_err(), IpcError::ProtocolMismatch);
    }

    #[test]
    fn ut_ipc_minor_versions_negotiate_to_the_lower_one() {
        let roles = vec![RolePolicy {
            role: "tsf".into(),
            capabilities: vec![],
        }];
        let mut policy = ServerPolicy::new(master(), roles).unwrap();
        policy.version = Version { major: 1, minor: 2 };
        let st = Arc::new(ServerState::new(policy));
        let mut c = creds(&st, "tsf", &[]);
        c.version = Version { major: 1, minor: 5 };
        let (s, cl) = run(&st, &c, 8);
        assert_eq!(s.unwrap().version, Version { major: 1, minor: 2 });
        assert_eq!(cl.unwrap().version, Version { major: 1, minor: 2 });
        // an older client talks to a newer server at its own minor
        c.version = Version { major: 1, minor: 0 };
        let (s, cl) = run(&st, &c, 9);
        assert_eq!(s.unwrap().version, Version { major: 1, minor: 0 });
        assert_eq!(cl.unwrap().version, Version { major: 1, minor: 0 });
    }

    /// Plays a hand-written client against the server (frame level) so tests can replay and tamper.
    fn raw_client(b: &mut MemEnd, msg: &Json) {
        send_msg(b, msg, far()).unwrap();
    }

    fn hello(role: &str, caps: &[&str], cnonce: &[u8; 32]) -> Json {
        Json::obj(vec![
            ("t", Json::str("hello")),
            ("major", Json::int(1)),
            ("minor", Json::int(0)),
            ("role", Json::str(role)),
            ("caps", Json::strs(caps)),
            ("cnonce", Json::Str(to_hex(cnonce))),
        ])
    }

    #[test]
    fn it_003_replayed_handshake_is_rejected() {
        let st = state();
        // 1. a legitimate handshake with a known client nonce, recorded
        let cnonce = [7u8; 32];
        let caps: Vec<String> = vec![];
        let role_key = st.policy.role_key("tsf");
        let (mut a, mut b) = mem_pair();
        let st2 = Arc::clone(&st);
        let server = thread::spawn(move || {
            server_handshake(&mut a, &st2, &mut CountingEntropy::new(10), far())
        });
        raw_client(&mut b, &hello("tsf", &[], &cnonce));
        let ch = recv_msg(&mut b, far()).unwrap();
        let snonce: [u8; 32] = hex_field(&ch, "snonce").unwrap();
        let binding = Binding {
            role: "tsf",
            requested_caps: &caps,
            client_version: PROTOCOL_VERSION,
            version: PROTOCOL_VERSION,
            client_nonce: &cnonce,
            server_nonce: &snonce,
        };
        let recorded_auth = Json::obj(vec![
            ("t", Json::str("auth")),
            ("proof", Json::Str(to_hex(&binding.client_proof(&role_key)))),
        ]);
        raw_client(&mut b, &recorded_auth);
        recv_msg(&mut b, far()).unwrap();
        server.join().unwrap().unwrap();

        // 2. the attacker replays the recorded hello+auth verbatim: same client nonce, so refused before any challenge
        let (mut a, mut b) = mem_pair();
        let st2 = Arc::clone(&st);
        let server = thread::spawn(move || {
            server_handshake(&mut a, &st2, &mut CountingEntropy::new(11), far())
        });
        raw_client(&mut b, &hello("tsf", &[], &cnonce));
        let reply = recv_msg(&mut b, far()).unwrap();
        assert_eq!(server_error(&reply), Some(IpcError::IdentityRejected));
        assert_eq!(server.join().unwrap().unwrap_err(), IpcError::Replay);

        // 3. a replay with a fresh client nonce but the recorded proof: the new server nonce makes the old proof worthless
        let (mut a, mut b) = mem_pair();
        let st2 = Arc::clone(&st);
        let server = thread::spawn(move || {
            server_handshake(&mut a, &st2, &mut CountingEntropy::new(12), far())
        });
        raw_client(&mut b, &hello("tsf", &[], &[8u8; 32]));
        recv_msg(&mut b, far()).unwrap();
        raw_client(&mut b, &recorded_auth);
        let reply = recv_msg(&mut b, far()).unwrap();
        assert_eq!(server_error(&reply), Some(IpcError::IdentityRejected));
        assert_eq!(
            server.join().unwrap().unwrap_err(),
            IpcError::IdentityRejected
        );
    }

    #[test]
    fn it_003_the_same_client_nonce_cannot_win_two_concurrent_handshakes() {
        // both pass the pre-check; the atomic insert after the proof lets only one of them in
        let st = state();
        let key = st.policy.role_key("tsf");
        let cnonce = [3u8; 32];
        let caps: Vec<String> = vec![];
        let mut ends = Vec::new();
        let mut servers = Vec::new();
        for seed in [20u8, 21] {
            let (mut a, b) = mem_pair();
            let st2 = Arc::clone(&st);
            servers.push(thread::spawn(move || {
                server_handshake(&mut a, &st2, &mut CountingEntropy::new(seed), far())
            }));
            ends.push(b);
        }
        let mut challenges = Vec::new();
        for b in &mut ends {
            raw_client(b, &hello("tsf", &[], &cnonce));
            challenges.push(recv_msg(b, far()).unwrap());
        }
        for (b, ch) in ends.iter_mut().zip(&challenges) {
            let snonce: [u8; 32] = hex_field(ch, "snonce").unwrap();
            let binding = Binding {
                role: "tsf",
                requested_caps: &caps,
                client_version: PROTOCOL_VERSION,
                version: PROTOCOL_VERSION,
                client_nonce: &cnonce,
                server_nonce: &snonce,
            };
            raw_client(
                b,
                &Json::obj(vec![
                    ("t", Json::str("auth")),
                    ("proof", Json::Str(to_hex(&binding.client_proof(&key)))),
                ]),
            );
        }
        let results: Vec<_> = servers.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            1,
            "{results:?}"
        );
        assert!(results.iter().any(|r| matches!(r, Err(IpcError::Replay))));
    }

    #[test]
    fn it_003_stale_challenge_answer_is_rejected() {
        let roles = vec![RolePolicy {
            role: "tsf".into(),
            capabilities: vec![],
        }];
        let mut policy = ServerPolicy::new(master(), roles).unwrap();
        policy.challenge_ttl = Duration::from_millis(60);
        let st = Arc::new(ServerState::new(policy));
        let key = st.policy.role_key("tsf");
        let (mut a, mut b) = mem_pair();
        let st2 = Arc::clone(&st);
        let server = thread::spawn(move || {
            server_handshake(&mut a, &st2, &mut CountingEntropy::new(30), far())
        });
        let cnonce = [5u8; 32];
        raw_client(&mut b, &hello("tsf", &[], &cnonce));
        let ch = recv_msg(&mut b, far()).unwrap();
        let snonce: [u8; 32] = hex_field(&ch, "snonce").unwrap();
        let caps: Vec<String> = vec![];
        let binding = Binding {
            role: "tsf",
            requested_caps: &caps,
            client_version: PROTOCOL_VERSION,
            version: PROTOCOL_VERSION,
            client_nonce: &cnonce,
            server_nonce: &snonce,
        };
        thread::sleep(Duration::from_millis(250));
        // the proof is correct, but too late
        raw_client(
            &mut b,
            &Json::obj(vec![
                ("t", Json::str("auth")),
                ("proof", Json::Str(to_hex(&binding.client_proof(&key)))),
            ]),
        );
        assert_eq!(
            server.join().unwrap().unwrap_err(),
            IpcError::StaleChallenge
        );
    }

    #[test]
    fn it_003_server_that_does_not_know_the_key_is_refused_by_the_client() {
        // a squatter that answers the hello with an arbitrary proof
        let st = state();
        let c = creds(&st, "tsf", &[]);
        let (mut a, mut b) = mem_pair();
        let squatter = thread::spawn(move || {
            let _hello = recv_msg(&mut a, far()).unwrap();
            let challenge = Json::obj(vec![
                ("t", Json::str("challenge")),
                ("major", Json::int(1)),
                ("minor", Json::int(0)),
                ("snonce", Json::Str("11".repeat(32))),
                ("proof", Json::Str("22".repeat(32))),
            ]);
            send_msg(&mut a, &challenge, far()).unwrap();
            // the client must not send its proof
            recv_msg(&mut a, Instant::now() + Duration::from_millis(300))
        });
        let r = client_handshake(&mut b, &c, &mut CountingEntropy::new(40), far());
        assert_eq!(r.unwrap_err(), IpcError::IdentityRejected);
        drop(b);
        assert!(
            squatter.join().unwrap().is_err(),
            "no auth frame may follow a bad server proof"
        );
    }

    #[test]
    fn it_003_tampered_ready_message_is_detected_by_the_client() {
        // a relay that grants more capabilities than the server did: the MAC over `ready` no longer fits
        let st = state();
        let c = creds(&st, "tsf", &[]);
        let (mut a, mut b) = mem_pair();
        let st2 = Arc::clone(&st);
        let relay = thread::spawn(move || {
            let (mut to_server, mut from_server) = mem_pair();
            let st3 = Arc::clone(&st2);
            let server = thread::spawn(move || {
                server_handshake(&mut from_server, &st3, &mut CountingEntropy::new(50), far())
            });
            // forward client -> server and server -> client, rewriting `ready`
            let hello = recv_msg(&mut a, far()).unwrap();
            send_msg(&mut to_server, &hello, far()).unwrap();
            let ch = recv_msg(&mut to_server, far()).unwrap();
            send_msg(&mut a, &ch, far()).unwrap();
            let auth = recv_msg(&mut a, far()).unwrap();
            send_msg(&mut to_server, &auth, far()).unwrap();
            let mut ready = recv_msg(&mut to_server, far()).unwrap();
            if let Json::Obj(o) = &mut ready {
                for (k, v) in o.iter_mut() {
                    if k == "caps" {
                        *v = Json::strs(&["predict.local", "cancel", "root"]);
                    }
                }
            }
            send_msg(&mut a, &ready, far()).unwrap();
            server.join().unwrap()
        });
        let r = client_handshake(&mut b, &c, &mut CountingEntropy::new(51), far());
        assert!(
            matches!(
                r,
                Err(IpcError::IdentityRejected) | Err(IpcError::Malformed(_))
            ),
            "{r:?}"
        );
        let _ = relay.join();
    }

    #[test]
    fn ut_ipc_malformed_hello_variants_are_rejected_without_panicking() {
        let st = state();
        let good = [9u8; 32];
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("not json", b"hello".to_vec()),
            ("array", b"[1]".to_vec()),
            ("no type", br#"{"major":1}"#.to_vec()),
            ("wrong type", br#"{"t":"auth","proof":"00"}"#.to_vec()),
            ("bad nonce length", br#"{"t":"hello","major":1,"minor":0,"role":"tsf","caps":[],"cnonce":"00"}"#.to_vec()),
            ("non-hex nonce", format!(r#"{{"t":"hello","major":1,"minor":0,"role":"tsf","caps":[],"cnonce":"{}"}}"#, "zz".repeat(32)).into_bytes()),
            ("version as string", format!(r#"{{"t":"hello","major":"1","minor":0,"role":"tsf","caps":[],"cnonce":"{}"}}"#, to_hex(&good)).into_bytes()),
            ("major out of range", format!(r#"{{"t":"hello","major":70000,"minor":0,"role":"tsf","caps":[],"cnonce":"{}"}}"#, to_hex(&good)).into_bytes()),
            ("caps not array", format!(r#"{{"t":"hello","major":1,"minor":0,"role":"tsf","caps":"x","cnonce":"{}"}}"#, to_hex(&good)).into_bytes()),
            ("duplicate caps", format!(r#"{{"t":"hello","major":1,"minor":0,"role":"tsf","caps":["cancel","cancel"],"cnonce":"{}"}}"#, to_hex(&good)).into_bytes()),
            ("bad cap name", format!(r#"{{"t":"hello","major":1,"minor":0,"role":"tsf","caps":["CANCEL"],"cnonce":"{}"}}"#, to_hex(&good)).into_bytes()),
            ("invalid utf-8", vec![0xff, 0xfe, b'{', b'}']),
            ("duplicate keys", format!(r#"{{"t":"hello","t":"hello","major":1,"minor":0,"role":"tsf","caps":[],"cnonce":"{}"}}"#, to_hex(&good)).into_bytes()),
        ];
        for (name, payload) in cases {
            let (mut a, mut b) = mem_pair();
            let st2 = Arc::clone(&st);
            let server = thread::spawn(move || {
                server_handshake(&mut a, &st2, &mut CountingEntropy::new(60), far())
            });
            frame::write_frame(
                &mut b,
                FrameClass::Control,
                &payload,
                &Limits::HANDSHAKE,
                far(),
            )
            .unwrap();
            let r = server.join().unwrap();
            assert!(
                matches!(r, Err(IpcError::Malformed(_))),
                "{name}: expected Malformed, got {r:?}"
            );
        }
    }

    #[test]
    fn ut_ipc_handshake_frames_above_4_kib_are_refused_before_buffering() {
        let st = state();
        let (mut a, mut b) = mem_pair();
        let st2 = Arc::clone(&st);
        let server = thread::spawn(move || {
            server_handshake(&mut a, &st2, &mut CountingEntropy::new(61), far())
        });
        // a header announcing 64 KiB, no payload at all: must fail on the header alone
        let header = frame::encode_header(FrameClass::Control, 64 * 1024).unwrap();
        b.write_all(&header, far()).unwrap();
        assert!(matches!(
            server.join().unwrap(),
            Err(IpcError::PayloadTooLarge { max: 4096, .. })
        ));
        let reply = recv_msg(&mut b, far()).unwrap();
        assert_eq!(
            reply.get("error").and_then(Json::as_str),
            Some("PAYLOAD_TOO_LARGE")
        );
    }

    #[test]
    fn ut_ipc_handshake_times_out_when_the_peer_is_silent() {
        let st = state();
        let (mut a, _b) = mem_pair();
        let started = Instant::now();
        let r = server_handshake(
            &mut a,
            &st,
            &mut CountingEntropy::new(62),
            Instant::now() + Duration::from_millis(120),
        );
        assert_eq!(r.unwrap_err(), IpcError::Timeout);
        assert!(started.elapsed() < Duration::from_millis(1500));
        // and the client side against a silent server
        let c = creds(&st, "tsf", &[]);
        let (mut x, _y) = mem_pair();
        let r = client_handshake(
            &mut x,
            &c,
            &mut CountingEntropy::new(63),
            Instant::now() + Duration::from_millis(120),
        );
        assert_eq!(r.unwrap_err(), IpcError::Timeout);
    }

    #[test]
    fn ut_ipc_client_refuses_bad_names_before_sending_anything() {
        let st = state();
        let mut c = creds(&st, "tsf", &[]);
        c.role = "TSF".into();
        let (mut x, mut y) = mem_pair();
        assert!(matches!(
            client_handshake(&mut x, &c, &mut CountingEntropy::new(64), far()),
            Err(IpcError::InvalidArgument(_))
        ));
        drop(x);
        assert_eq!(
            y.read_some(&mut [0u8; 1], far()),
            Err(IpcError::Closed),
            "nothing was sent"
        );
        assert!(ServerPolicy::new(master(), vec![]).is_err());
        assert!(
            ServerPolicy::new(
                master(),
                vec![RolePolicy {
                    role: "A".into(),
                    capabilities: vec![]
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn ut_ipc_valid_names() {
        for ok in ["tsf", "predict.local", "a-b_c:d", &"a".repeat(48)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "Tsf", "a b", "a/b", "a\"b", &"a".repeat(49), "é"] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }
}
