//! Pure security building blocks: keys, HMAC transcripts, constant-time comparison, the nonce replay window, protocol
//! versions, the pipe name and the SDDL string. No Win32 here, so every rule is unit-testable and portable.
//!
//! HMAC-SHA-256 itself is `pime_core::sha256::hmac_sha256` (RFC 2104, verified against RFC 4231 there and again below).

use crate::error::IpcError;
use pime_core::sha256::{from_hex, hmac_sha256, to_hex};
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// Shortest accepted connection key: 256 bits, the output size of the MAC.
pub const MIN_KEY_BYTES: usize = 32;
pub const MAX_KEY_BYTES: usize = 128;
pub const NONCE_BYTES: usize = 32;
/// 128-bit capability token and session id (DD 2.1 uses 128-bit identifiers for edit ids as well).
pub const TOKEN_BYTES: usize = 16;

/// Entropy source. Production uses the system CSPRNG (`winsec::SystemEntropy`); `pime-core` has none by design.
pub trait Entropy {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), IpcError>;
}

/// A long-lived secret (the per-user connection key or a role key). Never printed, wiped on drop (best effort:
/// without `unsafe` the compiler is free to drop the wipe, so this is hygiene, not a guarantee).
#[derive(Clone)]
pub struct Secret(Vec<u8>);

impl Secret {
    pub fn new(bytes: &[u8]) -> Result<Secret, IpcError> {
        if !(MIN_KEY_BYTES..=MAX_KEY_BYTES).contains(&bytes.len()) {
            return Err(IpcError::InvalidArgument(
                "connection key must be 32..=128 bytes",
            ));
        }
        Ok(Secret(bytes.to_vec()))
    }

    pub fn from_hex(hex: &str) -> Result<Secret, IpcError> {
        let bytes = from_hex(hex).ok_or(IpcError::InvalidArgument("connection key is not hex"))?;
        Secret::new(&bytes)
    }

    pub(crate) fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&mut self.0);
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Constant-time equality of two byte strings of public length: the comparison never exits early on the first
/// differing byte. (A length mismatch returns at once; MAC lengths are not secret.)
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

/// Unambiguous encoding of a label plus variable-length parts: every part is length-prefixed, so `("ab","c")`
/// and `("a","bc")` never collide, and the fixed prefix separates this protocol from any other use of the key.
pub fn transcript(label: &str, parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in [b"PIME-IPC/1".as_slice(), label.as_bytes()]
        .iter()
        .chain(parts.iter())
    {
        out.extend_from_slice(&(part.len() as u32).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
}

/// The version this crate implements: `PIME-IPC/1`, minor 0.
pub const PROTOCOL_VERSION: Version = Version { major: 1, minor: 0 };

impl Version {
    /// DD 2.1: a different major version is `PROTOCOL_MISMATCH`; within one major the lower minor wins, because a
    /// newer peer must be able to talk to an older one (new optional fields have defaults).
    pub fn negotiate(client: Version, server: Version) -> Result<Version, IpcError> {
        if client.major != server.major {
            return Err(IpcError::ProtocolMismatch);
        }
        Ok(Version {
            major: server.major,
            minor: client.minor.min(server.minor),
        })
    }

    fn bytes(self) -> [u8; 4] {
        let mut b = [0u8; 4];
        b[..2].copy_from_slice(&self.major.to_be_bytes());
        b[2..].copy_from_slice(&self.minor.to_be_bytes());
        b
    }
}

/// Everything a proof binds: both nonces, the role, the requested capabilities and both versions (a downgrade of the
/// negotiated version breaks the MAC).
pub struct Binding<'a> {
    pub role: &'a str,
    pub requested_caps: &'a [String],
    pub client_version: Version,
    pub version: Version,
    pub client_nonce: &'a [u8; NONCE_BYTES],
    pub server_nonce: &'a [u8; NONCE_BYTES],
}

impl Binding<'_> {
    fn parts(&self) -> Vec<Vec<u8>> {
        let mut parts: Vec<Vec<u8>> = vec![
            self.role.as_bytes().to_vec(),
            self.client_version.bytes().to_vec(),
            self.version.bytes().to_vec(),
            self.client_nonce.to_vec(),
            self.server_nonce.to_vec(),
            self.requested_caps.len().to_string().into_bytes(),
        ];
        parts.extend(self.requested_caps.iter().map(|c| c.as_bytes().to_vec()));
        parts
    }

    fn mac(&self, key: &[u8], label: &str) -> [u8; 32] {
        let parts = self.parts();
        let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        hmac_sha256(key, &transcript(label, &refs))
    }

    /// Proof of the server (sent first, so a squatter learns nothing the real client would have revealed).
    pub fn server_proof(&self, role_key: &Secret) -> [u8; 32] {
        self.mac(role_key.expose(), "server-proof")
    }

    /// Proof of the client. A different label from the server proof: a proof can never be reflected back.
    pub fn client_proof(&self, role_key: &Secret) -> [u8; 32] {
        self.mac(role_key.expose(), "client-proof")
    }

    /// Fresh key of this connection only; it authenticates the `ready` message and is available to callers that
    /// want per-message MACs later.
    pub fn session_key(&self, role_key: &Secret) -> [u8; 32] {
        self.mac(role_key.expose(), "session-key")
    }
}

/// The key of one role, derived from the per-user master key: the installer can hand each client class only its own
/// role key, and a role key cannot be turned into another role's key or into the master.
pub fn derive_role_key(master: &Secret, role: &str) -> Secret {
    let k = hmac_sha256(master.expose(), &transcript("role-key", &[role.as_bytes()]));
    Secret(k.to_vec())
}

/// MAC of the `ready` message under the session key: the granted capabilities and the token cannot be altered.
pub fn ready_mac(
    session_key: &[u8; 32],
    session: &str,
    token: &str,
    caps: &[String],
    ttl_ms: u64,
) -> [u8; 32] {
    let mut parts: Vec<Vec<u8>> = vec![
        session.as_bytes().to_vec(),
        token.as_bytes().to_vec(),
        ttl_ms.to_string().into_bytes(),
        caps.len().to_string().into_bytes(),
    ];
    parts.extend(caps.iter().map(|c| c.as_bytes().to_vec()));
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    hmac_sha256(session_key, &transcript("ready", &refs))
}

/// Server-side memory of client nonces that completed a handshake. A repeated nonce is a replay; entries expire after
/// `window`, a full table refuses new nonces (fail closed) instead of forgetting old ones. Time is injected as
/// milliseconds on the server's own monotonic clock: peer timestamps are never trusted (DD 2.2).
pub struct NonceWindow {
    window_ms: u64,
    capacity: usize,
    order: VecDeque<(u64, [u8; NONCE_BYTES])>,
    index: HashSet<[u8; NONCE_BYTES]>,
}

impl NonceWindow {
    pub fn new(window: Duration, capacity: usize) -> NonceWindow {
        NonceWindow {
            window_ms: window.as_millis().min(u128::from(u64::MAX)) as u64,
            capacity,
            order: VecDeque::new(),
            index: HashSet::new(),
        }
    }

    fn prune(&mut self, now_ms: u64) {
        while let Some(&(at, nonce)) = self.order.front() {
            if now_ms.saturating_sub(at) < self.window_ms {
                break;
            }
            self.order.pop_front();
            self.index.remove(&nonce);
        }
    }

    pub fn contains(&mut self, nonce: &[u8; NONCE_BYTES], now_ms: u64) -> bool {
        self.prune(now_ms);
        self.index.contains(nonce)
    }

    /// Records `nonce`; `Err(Replay)` if it is already inside the window or the table is full.
    pub fn check_and_insert(
        &mut self,
        nonce: &[u8; NONCE_BYTES],
        now_ms: u64,
    ) -> Result<(), IpcError> {
        self.prune(now_ms);
        if self.index.contains(nonce) || self.order.len() >= self.capacity {
            return Err(IpcError::Replay);
        }
        // keep `order` sorted even if a caller's clock steps back
        let at = self
            .order
            .back()
            .map_or(now_ms, |&(last, _)| last.max(now_ms));
        self.order.push_back((at, *nonce));
        self.index.insert(*nonce);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// Monotonic millisecond clock for the replay window and token expiry.
#[derive(Clone, Copy, Debug)]
pub struct MonoClock {
    start: Instant,
}

impl MonoClock {
    pub fn new() -> MonoClock {
        MonoClock {
            start: Instant::now(),
        }
    }

    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }
}

impl Default for MonoClock {
    fn default() -> Self {
        MonoClock::new()
    }
}

/// `S-1-<authority>-<sub>...` with digits only: the string is formatted into an SDDL string and a pipe name, so
/// anything else (parentheses, semicolons, backslashes) is refused, never escaped.
pub fn validate_sid(sid: &str) -> Result<(), IpcError> {
    const BAD: IpcError =
        IpcError::InvalidArgument("not a plain SID string (S-1-<digits>-<digits>...)");
    let rest = sid.strip_prefix("S-1-").ok_or(BAD)?;
    if sid.len() > 184 {
        return Err(BAD);
    }
    let mut components = 0;
    for part in rest.split('-') {
        if part.is_empty() || part.len() > 20 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(BAD);
        }
        components += 1;
    }
    // authority + at least one sub-authority (even S-1-5-18, the SYSTEM account, has both)
    if components < 2 {
        return Err(BAD);
    }
    Ok(())
}

/// Access mask granted to the user on the pipe: `FILE_GENERIC_READ | FILE_GENERIC_WRITE`, which includes
/// `FILE_CREATE_PIPE_INSTANCE` (0x4). That right is needed: the server creates one more instance per accepted
/// client, and Windows checks it against this very DACL for the server's own token (measured here: with the masks
/// 0x120083 and 0x120087 the server's replacement instances could not be created, ERROR_ACCESS_DENIED observed for
/// the first mask; the full generic read/write set works). Consequence, documented in `docs/IPC.md` sections 5 and 6:
/// another process of the same user can add instances to the live pipe too; the handshake keeps it from posing as the
/// server or a client, but it can relay a handshake it cannot forge, and frames are not yet authenticated one by one.
/// Deliberately not `GA`: no `WRITE_DAC`, `WRITE_OWNER` or `DELETE`.
pub const PIPE_USER_ACCESS_MASK: u32 = 0x0012_019F;

/// `D:P(A;;<mask>;;;<SID>)`: a protected DACL (no inheritance) with a single ACE for this user, nobody else.
pub fn sddl_for_sid(sid: &str) -> Result<String, IpcError> {
    validate_sid(sid)?;
    Ok(format!("D:P(A;;{PIPE_USER_ACCESS_MASK:#x};;;{sid})"))
}

/// The pipe a user's endpoints share: `\\.\pipe\PIME-IPC-1-<user SID>` (the `\\.\` form is always the local machine).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipeName(String);

const PIPE_PREFIX: &str = r"\\.\pipe\PIME-IPC-1-";

impl PipeName {
    pub fn for_sid(sid: &str) -> Result<PipeName, IpcError> {
        validate_sid(sid)?;
        Ok(PipeName(format!("{PIPE_PREFIX}{sid}")))
    }

    /// A throw-away name for tests: `\\.\pipe\PIME-IPC-1-TEST-<suffix>`. Tests must pass a random suffix.
    pub fn for_test(suffix: &str) -> Result<PipeName, IpcError> {
        if suffix.is_empty()
            || suffix.len() > 64
            || !suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(IpcError::InvalidArgument(
                "test suffix must be 1..=64 of [A-Za-z0-9-]",
            ));
        }
        Ok(PipeName(format!("{PIPE_PREFIX}TEST-{suffix}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PipeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Lowercase hex of random bytes (tokens, session ids, nonces on the wire).
pub fn random_hex(entropy: &mut dyn Entropy, bytes: usize) -> Result<String, IpcError> {
    let mut buf = vec![0u8; bytes];
    entropy.fill(&mut buf)?;
    Ok(to_hex(&buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::hex;

    #[test]
    fn ut_ipc_hmac_rfc4231_test_case_1() {
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            to_hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn ut_ipc_hmac_rfc4231_test_case_2_short_key() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            to_hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn ut_ipc_hmac_rfc4231_test_case_3_and_4() {
        assert_eq!(
            to_hex(&hmac_sha256(&[0xaa; 20], &[0xdd; 50])),
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        );
        let key = hex("0102030405060708090a0b0c0d0e0f10111213141516171819");
        assert_eq!(
            to_hex(&hmac_sha256(&key, &[0xcd; 50])),
            "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"
        );
    }

    #[test]
    fn ut_ipc_hmac_rfc4231_test_case_6_and_7_keys_longer_than_the_block() {
        assert_eq!(
            to_hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
        assert_eq!(
            to_hex(&hmac_sha256(
                &[0xaa; 131],
                b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm."
            )),
            "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2"
        );
    }

    #[test]
    fn ut_ipc_ct_eq_semantics() {
        assert!(ct_eq(b"", b""));
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(!ct_eq(b"\x00", b"\x80"));
        // a difference in the very first and in the very last byte are both found
        let a = [7u8; 32];
        let mut first = a;
        first[0] ^= 1;
        let mut last = a;
        last[31] ^= 1;
        assert!(!ct_eq(&a, &first) && !ct_eq(&a, &last) && ct_eq(&a, &a));
    }

    #[test]
    fn ut_ipc_transcript_is_unambiguous() {
        assert_ne!(
            transcript("l", &[b"ab", b"c"]),
            transcript("l", &[b"a", b"bc"])
        );
        assert_ne!(transcript("l", &[b"ab"]), transcript("m", &[b"ab"]));
        assert_ne!(transcript("l", &[b"a", b""]), transcript("l", &[b"a"]));
    }

    fn secret(byte: u8) -> Secret {
        Secret::new(&[byte; 32]).unwrap()
    }

    #[test]
    fn ut_ipc_secret_length_is_enforced_and_debug_is_redacted() {
        assert!(Secret::new(&[1; 31]).is_err());
        assert!(Secret::new(&[1; 32]).is_ok());
        assert!(Secret::new(&[1; 129]).is_err());
        assert!(Secret::from_hex("zz").is_err());
        assert_eq!(format!("{:?}", secret(9)), "Secret(<redacted>)");
    }

    fn binding<'a>(
        caps: &'a [String],
        cn: &'a [u8; 32],
        sn: &'a [u8; 32],
        v: Version,
    ) -> Binding<'a> {
        Binding {
            role: "tsf",
            requested_caps: caps,
            client_version: PROTOCOL_VERSION,
            version: v,
            client_nonce: cn,
            server_nonce: sn,
        }
    }

    #[test]
    fn ut_ipc_proofs_bind_every_input_and_never_reflect() {
        let key = derive_role_key(&secret(1), "tsf");
        let caps = vec!["predict.local".to_string()];
        let (cn, sn) = ([1u8; 32], [2u8; 32]);
        let base = binding(&caps, &cn, &sn, PROTOCOL_VERSION);
        let client = base.client_proof(&key);
        // deterministic
        assert_eq!(
            client,
            binding(&caps, &cn, &sn, PROTOCOL_VERSION).client_proof(&key)
        );
        // server and client proofs and the session key are different domains
        assert_ne!(client, base.server_proof(&key));
        assert_ne!(client, base.session_key(&key));
        // each input changes the proof
        assert_ne!(
            client,
            binding(&caps, &sn, &cn, PROTOCOL_VERSION).client_proof(&key)
        );
        assert_ne!(
            client,
            binding(&caps, &cn, &[3u8; 32], PROTOCOL_VERSION).client_proof(&key)
        );
        assert_ne!(
            client,
            binding(&[], &cn, &sn, PROTOCOL_VERSION).client_proof(&key)
        );
        let other_caps = vec!["predict.local".to_string(), "snapshot".to_string()];
        assert_ne!(
            client,
            binding(&other_caps, &cn, &sn, PROTOCOL_VERSION).client_proof(&key)
        );
        let downgraded = Version { major: 1, minor: 7 };
        assert_ne!(
            client,
            binding(&caps, &cn, &sn, downgraded).client_proof(&key)
        );
        let mut other_role = binding(&caps, &cn, &sn, PROTOCOL_VERSION);
        other_role.role = "ui";
        assert_ne!(client, other_role.client_proof(&key));
        assert_ne!(
            client,
            base.client_proof(&derive_role_key(&secret(2), "tsf"))
        );
    }

    #[test]
    fn ut_ipc_role_keys_are_independent() {
        let master = secret(5);
        let tsf = derive_role_key(&master, "tsf");
        let bridge = derive_role_key(&master, "bridge");
        assert_ne!(tsf.expose(), bridge.expose());
        assert_ne!(tsf.expose(), master.expose());
        assert_eq!(tsf.expose(), derive_role_key(&master, "tsf").expose());
    }

    #[test]
    fn ut_ipc_ready_mac_covers_token_caps_and_lifetime() {
        let k = [9u8; 32];
        let caps = vec!["a".to_string()];
        let base = ready_mac(&k, "s", "t", &caps, 1000);
        assert_ne!(base, ready_mac(&k, "s", "T", &caps, 1000));
        assert_ne!(base, ready_mac(&k, "s", "t", &["b".to_string()], 1000));
        assert_ne!(base, ready_mac(&k, "s", "t", &caps, 1001));
        assert_ne!(base, ready_mac(&[8u8; 32], "s", "t", &caps, 1000));
    }

    #[test]
    fn ut_ipc_nonce_window_rejects_replays_inside_the_window_only() {
        let mut w = NonceWindow::new(Duration::from_secs(10), 16);
        let n = [1u8; 32];
        assert!(!w.contains(&n, 0));
        assert_eq!(w.check_and_insert(&n, 0), Ok(()));
        assert!(w.contains(&n, 9_999));
        assert_eq!(w.check_and_insert(&n, 9_999), Err(IpcError::Replay));
        assert_eq!(w.check_and_insert(&[2u8; 32], 5_000), Ok(()));
        // after the window the entry is forgotten (the proof binds a fresh server nonce anyway), and the table shrinks
        assert_eq!(w.len(), 2);
        assert_eq!(w.check_and_insert(&n, 10_000), Ok(()));
        assert_eq!(
            w.len(),
            2,
            "the first nonce expired and was pruned, the second is still inside"
        );
    }

    #[test]
    fn ut_ipc_nonce_window_full_table_fails_closed() {
        let mut w = NonceWindow::new(Duration::from_secs(60), 3);
        for i in 0..3u8 {
            assert_eq!(w.check_and_insert(&[i; 32], u64::from(i)), Ok(()));
        }
        assert_eq!(w.check_and_insert(&[9u8; 32], 10), Err(IpcError::Replay));
        // room again once the oldest entry expires
        assert_eq!(w.check_and_insert(&[9u8; 32], 60_000), Ok(()));
    }

    #[test]
    fn ut_ipc_nonce_window_tolerates_a_clock_that_steps_back() {
        let mut w = NonceWindow::new(Duration::from_secs(1), 8);
        assert_eq!(w.check_and_insert(&[1; 32], 5_000), Ok(()));
        assert_eq!(w.check_and_insert(&[2; 32], 100), Ok(()));
        assert_eq!(w.check_and_insert(&[2; 32], 100), Err(IpcError::Replay));
        assert!(w.contains(&[1; 32], 5_500));
    }

    #[test]
    fn ut_ipc_version_negotiation() {
        let v = |major, minor| Version { major, minor };
        assert_eq!(Version::negotiate(v(1, 0), v(1, 0)), Ok(v(1, 0)));
        assert_eq!(Version::negotiate(v(1, 3), v(1, 1)), Ok(v(1, 1)));
        assert_eq!(Version::negotiate(v(1, 0), v(1, 4)), Ok(v(1, 0)));
        assert_eq!(
            Version::negotiate(v(2, 0), v(1, 0)),
            Err(IpcError::ProtocolMismatch)
        );
        assert_eq!(
            Version::negotiate(v(0, 9), v(1, 0)),
            Err(IpcError::ProtocolMismatch)
        );
    }

    #[test]
    fn ut_ipc_sid_validation_accepts_real_sids_and_refuses_everything_else() {
        for ok in [
            "S-1-5-18",
            "S-1-5-21-1004336348-1177238915-682003330-512",
            "S-1-5-5-0-123456",
            "S-1-0-0",
        ] {
            assert!(validate_sid(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "S-1-",
            "S-1-5",
            "S-1-5-",
            "S-1--5",
            "s-1-5-18",
            "S-2-5-18",
            "S-1-5-18 ",
            "S-1-5-1a",
            "S-1-5-18)(A;;GA;;;WD",
            "S-1-5-18;",
            "S-1-5-18\\x",
            "S-1-5-18\0",
            "S-1-5-٣",
        ] {
            assert!(validate_sid(bad).is_err(), "{bad:?}");
        }
        assert!(validate_sid(&format!("S-1-5-{}", "9".repeat(200))).is_err());
    }

    #[test]
    fn ut_ipc_sddl_is_a_protected_dacl_with_exactly_one_ace_for_the_user() {
        let sid = "S-1-5-21-1004336348-1177238915-682003330-1001";
        let sddl = sddl_for_sid(sid).unwrap();
        assert_eq!(sddl, format!("D:P(A;;0x12019f;;;{sid})"));
        assert_eq!(sddl.matches("(A;").count(), 1);
        assert!(sddl.starts_with("D:P("), "protected: no inherited entries");
        // an injection attempt must be refused, not escaped
        assert!(sddl_for_sid("S-1-5-21-1)(A;;GA;;;WD").is_err());
        assert!(sddl_for_sid("WD").is_err());
        // exactly FILE_GENERIC_READ (0x120089) | FILE_GENERIC_WRITE (0x120116)
        assert_eq!(PIPE_USER_ACCESS_MASK, 0x0012_0089 | 0x0012_0116);
        assert_ne!(
            PIPE_USER_ACCESS_MASK & 0x4,
            0,
            "create-pipe-instance is part of it"
        );
        // no change-the-ACL, take-ownership or delete rights
        assert_eq!(
            PIPE_USER_ACCESS_MASK & (0x0004_0000 | 0x0008_0000 | 0x0001_0000),
            0
        );
    }

    #[test]
    fn ut_ipc_pipe_names() {
        let sid = "S-1-5-21-1-2-3-1001";
        assert_eq!(
            PipeName::for_sid(sid).unwrap().as_str(),
            r"\\.\pipe\PIME-IPC-1-S-1-5-21-1-2-3-1001"
        );
        assert!(PipeName::for_sid(r"S-1-5-1\..\x").is_err());
        let t = PipeName::for_test("abc-123").unwrap();
        assert_eq!(t.as_str(), r"\\.\pipe\PIME-IPC-1-TEST-abc-123");
        for bad in ["", "a b", "a\\b", "..", &"x".repeat(65)] {
            assert!(PipeName::for_test(bad).is_err(), "{bad:?}");
        }
    }
}
