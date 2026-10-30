//! Loopback tests of `PIME-IPC/1` on real named pipes, server and clients in this process.
//!
//! Every test binds its own pipe, `\\.\pipe\PIME-IPC-1-TEST-<random hex>`: never a fixed name, so tests never collide
//! with each other, with a second run or with a real PIME service. Nothing here needs a second Windows user, so the
//! cases that do (IT-003 "different SID", remote clients, another session) are listed as NOT RUN in docs/IPC.md.
//!
//! The ids IT-003 / IT-004 are the design cases; these tests cover the parts of them that are runnable on one machine
//! without TSF (see the doc comments). The full IT-004 / ST-037 need the real TSF DLL and a VM.
#![cfg(windows)]

use pime_core::json::{self, Json};
use pime_core::sha256::to_hex;
use pime_ipc::auth::{Binding, PROTOCOL_VERSION, derive_role_key};
use pime_ipc::endpoint::{
    ClientConfig, Listener, PipeConnection, ServerConfig, connect, unique_test_pipe_name,
};
use pime_ipc::frame::{self, FrameClass, HEADER_LEN, Limits, Transport};
use pime_ipc::handshake::client_handshake;
use pime_ipc::pipe::{Pipe, StopHandle};
use pime_ipc::winsec::{SystemEntropy, current_user_sid_string};
use pime_ipc::{ClientCredentials, IpcError, PipeName, RolePolicy, Secret, ServerPolicy, Timeouts};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

type Events = Arc<Mutex<Vec<String>>>;

const MASTER: [u8; 32] = [0x5a; 32];

fn parse(text: &str) -> Json {
    json::parse(text, json::Limits::DEFAULT).expect("valid JSON")
}

fn within(limit: Duration) -> Instant {
    Instant::now() + limit
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if cond() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for: {what}");
}

/// The per-connection behaviour of the test server: a tiny request/response protocol.
fn handle(mut conn: PipeConnection, stop: &StopHandle, events: &Events) {
    loop {
        let frame = match conn.recv_within(Duration::from_millis(200)) {
            Ok(f) => f,
            Err(IpcError::Timeout) if !stop.is_signalled() => continue,
            Err(e) => {
                events.lock().unwrap().push(format!("{e:?}"));
                return;
            }
        };
        let req = parse(&frame.text);
        let id = req.get("id").and_then(Json::as_i64).unwrap_or(0);
        let reply = match req.get("op").and_then(Json::as_str) {
            Some("health") => Json::obj(vec![("ok", Json::Bool(true)), ("id", Json::int(id))]),
            Some("echo") => Json::obj(vec![
                ("ok", Json::Bool(true)),
                ("id", Json::int(id)),
                ("data", req.get("data").cloned().unwrap_or(Json::Null)),
            ]),
            Some("whoami") => Json::obj(vec![
                ("ok", Json::Bool(true)),
                ("role", Json::str(&conn.info().role)),
                ("caps", Json::strs(&conn.info().capabilities)),
                ("token", Json::str(conn.info().token())),
                ("session", Json::str(&conn.info().session)),
                (
                    "token_ok",
                    Json::Bool(
                        req.get("token")
                            .and_then(Json::as_str)
                            .is_some_and(|t| conn.info().verify_token(t)),
                    ),
                ),
            ]),
            // keep the connection but never answer, until the client goes away
            Some("silent") => {
                while conn.recv_within(Duration::from_millis(100)).is_ok() || !stop.is_signalled() {
                    if conn.is_broken() {
                        break;
                    }
                }
                return;
            }
            // stop reading for a while: the peer's writes pile up
            Some("stall") => {
                thread::sleep(Duration::from_millis(1500));
                return;
            }
            Some("panic") => panic!("handler panics on purpose"),
            _ => Json::obj(vec![
                ("ok", Json::Bool(false)),
                ("error", Json::str("UNKNOWN_OP")),
            ]),
        };
        let text = reply.to_json_string();
        let class = if text.len() > 60_000 {
            FrameClass::Bulk
        } else {
            FrameClass::Control
        };
        if conn.send(class, &text).is_err() {
            return;
        }
    }
}

struct Server {
    name: PipeName,
    stop: StopHandle,
    events: Events,
    join: Option<JoinHandle<Result<(), IpcError>>>,
}

fn roles() -> Vec<RolePolicy> {
    vec![
        RolePolicy {
            role: "tsf".into(),
            capabilities: vec!["predict.local".into(), "cancel".into()],
        },
        RolePolicy {
            role: "bridge".into(),
            capabilities: vec!["snapshot".into(), "edit".into()],
        },
    ]
}

fn master() -> Secret {
    Secret::new(&MASTER).unwrap()
}

fn creds(role: &str, caps: &[&str]) -> ClientCredentials {
    ClientCredentials::new(role, derive_role_key(&master(), role), caps)
}

impl Server {
    fn start() -> Server {
        Server::start_with(|_| {})
    }

    fn start_with(tune: impl FnOnce(&mut ServerConfig)) -> Server {
        Server::launch(64, tune)
    }

    fn launch(max_connections: usize, tune: impl FnOnce(&mut ServerConfig)) -> Server {
        let name = unique_test_pipe_name().expect("unique name");
        let mut cfg = ServerConfig::new(ServerPolicy::new(master(), roles()).unwrap());
        tune(&mut cfg);
        let mut listener = Listener::bind(name.clone(), cfg).expect("bind");
        let stop = listener.stop_handle();
        let events: Events = Arc::default();
        let (h_stop, h_events) = (stop.clone(), Arc::clone(&events));
        let join = thread::spawn(move || {
            listener.serve(max_connections, move |c| handle(c, &h_stop, &h_events))
        });
        Server {
            name,
            stop,
            events,
            join: Some(join),
        }
    }

    fn connect(&self, role: &str, caps: &[&str]) -> Result<PipeConnection, IpcError> {
        connect(&self.name, &creds(role, caps), &ClientConfig::default())
    }

    fn ask(&self, conn: &mut PipeConnection, text: &str) -> Json {
        parse(
            &conn
                .request_within(text, Duration::from_secs(3))
                .expect("reply")
                .text,
        )
    }

    fn shutdown(&mut self) {
        self.stop.signal();
        if let Some(j) = self.join.take() {
            j.join().expect("serve thread").expect("serve ends cleanly");
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.signal();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// IT-003: identity

/// IT-003 "合法连接成功": same user, right key.
#[test]
fn it_003_same_user_round_trip_and_token() {
    let server = Server::start();
    let mut c = server.connect("tsf", &["predict.local"]).expect("connect");
    assert_eq!(c.info().role, "tsf");
    assert_eq!(c.info().capabilities, vec!["predict.local".to_string()]);
    let r = server.ask(&mut c, r#"{"op":"health","id":7}"#);
    assert_eq!(r.get("ok"), Some(&Json::Bool(true)));
    assert_eq!(r.get("id").and_then(Json::as_i64), Some(7));

    let token = c.info().token().to_string();
    let r = server.ask(&mut c, &format!(r#"{{"op":"whoami","token":"{token}"}}"#));
    assert_eq!(r.get("role").and_then(Json::as_str), Some("tsf"));
    assert_eq!(
        r.get("token").and_then(Json::as_str),
        Some(token.as_str()),
        "both ends hold the same token"
    );
    assert_eq!(
        r.get("session").and_then(Json::as_str),
        Some(c.info().session.as_str())
    );
    assert_eq!(r.get("token_ok"), Some(&Json::Bool(true)));
    let r = server.ask(
        &mut c,
        &format!(r#"{{"op":"whoami","token":"{}"}}"#, "0".repeat(32)),
    );
    assert_eq!(
        r.get("token_ok"),
        Some(&Json::Bool(false)),
        "a token of another connection is refused"
    );
}

/// IT-003 "非法连接拒绝": a client with the wrong key. The server keeps serving.
#[test]
fn it_003_wrong_key_rejected_and_server_survives() {
    let server = Server::start();
    let wrong = ClientCredentials::new(
        "tsf",
        derive_role_key(&Secret::new(&[0x11; 32]).unwrap(), "tsf"),
        &[],
    );
    let r = connect(&server.name, &wrong, &ClientConfig::default());
    assert_eq!(r.err(), Some(IpcError::IdentityRejected));
    // an unknown role, a role key of another role, and a capability the role lacks are all refused
    let r = connect(
        &server.name,
        &ClientCredentials::new("admin", derive_role_key(&master(), "admin"), &[]),
        &ClientConfig::default(),
    );
    assert_eq!(r.err(), Some(IpcError::IdentityRejected));
    let r = connect(
        &server.name,
        &ClientCredentials::new("bridge", derive_role_key(&master(), "tsf"), &[]),
        &ClientConfig::default(),
    );
    assert_eq!(r.err(), Some(IpcError::IdentityRejected));
    let r = server.connect("tsf", &["snapshot"]);
    assert_eq!(r.err(), Some(IpcError::CapabilityDenied));
    // the server is unaffected
    let mut ok = server
        .connect("tsf", &[])
        .expect("a good client still connects");
    assert_eq!(
        server.ask(&mut ok, r#"{"op":"health","id":1}"#).get("ok"),
        Some(&Json::Bool(true))
    );
}

/// A hand-written client for the replay tests: the handshake at frame level, over a raw pipe.
struct RawClient {
    pipe: Pipe,
}

impl RawClient {
    fn open(name: &PipeName) -> RawClient {
        RawClient {
            pipe: Pipe::connect_client(name, within(Duration::from_secs(2))).expect("open pipe"),
        }
    }

    fn send(&mut self, msg: &Json) {
        frame::write_frame(
            &mut self.pipe,
            FrameClass::Control,
            msg.to_json_string().as_bytes(),
            &Limits::HANDSHAKE,
            within(Duration::from_secs(2)),
        )
        .expect("send");
    }

    fn recv(&mut self) -> Result<Json, IpcError> {
        let mut consumed = false;
        let (_, payload) = frame::read_frame(
            &mut self.pipe,
            &Limits::HANDSHAKE,
            within(Duration::from_secs(3)),
            &mut consumed,
        )?;
        Ok(parse(std::str::from_utf8(&payload).expect("utf-8")))
    }

    fn hello(&mut self, role: &str, cnonce: &[u8; 32]) {
        self.send(&Json::obj(vec![
            ("t", Json::str("hello")),
            ("major", Json::int(1)),
            ("minor", Json::int(0)),
            ("role", Json::str(role)),
            ("caps", Json::Arr(vec![])),
            ("cnonce", Json::Str(to_hex(cnonce))),
        ]));
    }
}

fn client_proof(role: &str, cnonce: &[u8; 32], snonce: &[u8; 32]) -> Json {
    let key = derive_role_key(&master(), role);
    let caps: Vec<String> = vec![];
    let b = Binding {
        role,
        requested_caps: &caps,
        client_version: PROTOCOL_VERSION,
        version: PROTOCOL_VERSION,
        client_nonce: cnonce,
        server_nonce: snonce,
    };
    Json::obj(vec![
        ("t", Json::str("auth")),
        ("proof", Json::Str(to_hex(&b.client_proof(&key)))),
    ])
}

fn nonce_of(msg: &Json, key: &str) -> [u8; 32] {
    let v = pime_core::sha256::from_hex(msg.get(key).and_then(Json::as_str).expect("field"))
        .expect("hex");
    v.try_into().expect("32 bytes")
}

/// IT-003 "重放 challenge": a recorded legitimate handshake is replayed on new connections.
#[test]
fn it_003_replayed_handshake_rejected() {
    let server = Server::start();
    let cnonce = [0x77u8; 32];

    // the legitimate exchange, recorded
    let mut first = RawClient::open(&server.name);
    first.hello("tsf", &cnonce);
    let challenge = first.recv().expect("challenge");
    let snonce = nonce_of(&challenge, "snonce");
    let recorded = client_proof("tsf", &cnonce, &snonce);
    first.send(&recorded);
    let ready = first.recv().expect("ready");
    assert_eq!(ready.get("t").and_then(Json::as_str), Some("ready"));

    // replay 1: the same hello again: refused with the generic identity error before any challenge
    let mut replay = RawClient::open(&server.name);
    replay.hello("tsf", &cnonce);
    let r = replay.recv().expect("error frame");
    assert_eq!(r.get("ok"), Some(&Json::Bool(false)));
    assert_eq!(
        r.get("error").and_then(Json::as_str),
        Some("IDENTITY_REJECTED")
    );
    assert!(
        matches!(
            replay.recv(),
            Err(IpcError::Closed) | Err(IpcError::Truncated)
        ),
        "then the server closes"
    );

    // replay 2: fresh client nonce, recorded proof: the server's new nonce makes it worthless
    let mut replay = RawClient::open(&server.name);
    replay.hello("tsf", &[0x78u8; 32]);
    let ch2 = replay.recv().expect("challenge");
    assert_ne!(
        nonce_of(&ch2, "snonce"),
        snonce,
        "the server never reuses a challenge"
    );
    replay.send(&recorded);
    let r = replay.recv().expect("error frame");
    assert_eq!(
        r.get("error").and_then(Json::as_str),
        Some("IDENTITY_REJECTED")
    );

    // and the server still accepts a good client
    assert!(server.connect("tsf", &[]).is_ok());
}

/// IT-003: a challenge that is answered too late is refused (stale), even with a correct proof.
#[test]
fn it_003_stale_challenge_rejected_on_the_real_pipe() {
    let server = Server::start_with(|cfg| cfg.policy.challenge_ttl = Duration::from_millis(100));
    let cnonce = [0x55u8; 32];
    let mut c = RawClient::open(&server.name);
    c.hello("tsf", &cnonce);
    let challenge = c.recv().expect("challenge");
    let proof = client_proof("tsf", &cnonce, &nonce_of(&challenge, "snonce"));
    thread::sleep(Duration::from_millis(400));
    c.send(&proof);
    let r = c.recv().expect("error frame");
    assert_eq!(
        r.get("error").and_then(Json::as_str),
        Some("IDENTITY_REJECTED")
    );
}

/// The handshake has its own deadline: a client that connects and says nothing is dropped, the server goes on.
#[test]
fn it_004_handshake_timeout_part_silent_client_is_dropped_within_the_limit() {
    let server = Server::start_with(|cfg| {
        cfg.timeouts = Timeouts {
            handshake: Duration::from_millis(300),
            ..Timeouts::default()
        }
    });
    let mut silent = RawClient::open(&server.name);
    let started = Instant::now();
    // the server must close this connection by itself
    let r = silent
        .pipe
        .read_some(&mut [0u8; 1], within(Duration::from_secs(3)));
    let waited = started.elapsed();
    assert_eq!(
        r,
        Err(IpcError::Closed),
        "the server closes a client that never starts the handshake"
    );
    assert!(
        waited < Duration::from_millis(300 + 1500),
        "closed after {waited:?}"
    );
    assert!(
        server.connect("tsf", &[]).is_ok(),
        "other clients are not held up"
    );
}

// ---------------------------------------------------------------------------------------------------------------
// frames and malformed input

/// UT-ipc: a header that announces more than the limit is refused from the 8 header bytes alone (no payload is read
/// or waited for), answered with PAYLOAD_TOO_LARGE, and the connection is closed.
#[test]
fn ut_ipc_oversized_frame_rejected_with_an_error_frame_and_close() {
    let server = Server::start();
    let mut pipe = Pipe::connect_client(&server.name, within(Duration::from_secs(2))).unwrap();
    client_handshake(
        &mut pipe,
        &creds("tsf", &[]),
        &mut SystemEntropy,
        within(Duration::from_secs(2)),
    )
    .expect("handshake");
    // announce 70,000 bytes as a control frame (limit 65,536) and send none of them
    let mut header = frame::encode_header(FrameClass::Control, 1).unwrap();
    header[4..].copy_from_slice(&70_000u32.to_le_bytes());
    pipe.write_all(&header, within(Duration::from_secs(2)))
        .unwrap();
    // the error frame is delivered before the server closes (CloseHandle without Disconnect keeps buffered data)
    let mut consumed = false;
    let (_, payload) = frame::read_frame(
        &mut pipe,
        &Limits::DEFAULT,
        within(Duration::from_secs(3)),
        &mut consumed,
    )
    .expect("error frame");
    let reply = parse(std::str::from_utf8(&payload).unwrap());
    assert_eq!(reply.get("ok"), Some(&Json::Bool(false)));
    assert_eq!(
        reply.get("error").and_then(Json::as_str),
        Some("PAYLOAD_TOO_LARGE")
    );
    assert_eq!(
        frame::read_frame(
            &mut pipe,
            &Limits::DEFAULT,
            within(Duration::from_secs(3)),
            &mut consumed
        )
        .err(),
        Some(IpcError::Closed)
    );
    wait_until("server saw the oversized frame", || {
        server
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.contains("PayloadTooLarge"))
    });
    assert!(server.connect("tsf", &[]).is_ok());
}

#[test]
fn ut_ipc_sender_side_limits_are_enforced_before_the_pipe() {
    let server = Server::start();
    let mut c = server.connect("tsf", &[]).unwrap();
    let big = format!(r#"{{"op":"echo","data":"{}"}}"#, "x".repeat(70_000));
    assert!(matches!(
        c.send(FrameClass::Control, &big),
        Err(IpcError::PayloadTooLarge { max: 65_536, .. })
    ));
    let huge = format!(
        r#"{{"op":"echo","data":"{}"}}"#,
        "x".repeat(2 * 1024 * 1024)
    );
    assert!(matches!(
        c.send(FrameClass::Bulk, &huge),
        Err(IpcError::PayloadTooLarge { .. })
    ));
    // nothing was sent, so the connection is still good
    assert_eq!(
        server
            .ask(&mut c, r#"{"op":"health","id":2}"#)
            .get("id")
            .and_then(Json::as_i64),
        Some(2)
    );
}

/// A bulk frame of 1.5 MiB passes both ways: it is far larger than the pipe buffer, so both sides must keep up.
#[test]
fn ut_ipc_bulk_frame_round_trip_through_the_pipe_buffers() {
    let server = Server::start();
    let mut c = server.connect("bridge", &[]).unwrap();
    // 1 + 2 + 3 bytes per repetition: multi-byte characters straddle every chunk boundary of the pipe reads
    let data = "a\u{e9}\u{6f22}".repeat(150_000);
    let req = format!(r#"{{"op":"echo","id":9,"data":"{data}"}}"#);
    assert!(
        req.len() > 800_000 && req.len() < 1_000_000,
        "{}",
        req.len()
    );
    c.send(FrameClass::Bulk, &req).expect("send bulk");
    let reply = c.recv_within(Duration::from_secs(5)).expect("bulk reply");
    assert_eq!(reply.class, FrameClass::Bulk);
    assert_eq!(
        parse(&reply.text).get("data").and_then(Json::as_str),
        Some(data.as_str())
    );
    // the connection is still in sync afterwards
    assert_eq!(
        server
            .ask(&mut c, r#"{"op":"health","id":10}"#)
            .get("id")
            .and_then(Json::as_i64),
        Some(10)
    );
}

/// IT-003/DD 2.1: server survives a client that disconnects in the middle of a frame.
#[test]
fn it_003_server_survives_a_client_that_disconnects_mid_frame() {
    let server = Server::start();
    {
        let mut pipe = Pipe::connect_client(&server.name, within(Duration::from_secs(2))).unwrap();
        let info = client_handshake(
            &mut pipe,
            &creds("tsf", &[]),
            &mut SystemEntropy,
            within(Duration::from_secs(2)),
        )
        .expect("handshake");
        assert_eq!(info.role, "tsf");
        // a header promising 1000 bytes, then only 10 of them, then the client vanishes
        let mut partial = frame::encode_header(FrameClass::Control, 1000)
            .unwrap()
            .to_vec();
        partial.extend_from_slice(b"{\"op\":\"ech");
        assert_eq!(partial.len(), HEADER_LEN + 10);
        pipe.write_all(&partial, within(Duration::from_secs(2)))
            .unwrap();
    } // pipe dropped here
    wait_until("server noticed the truncated frame", || {
        server
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.contains("Truncated"))
    });
    let mut ok = server.connect("tsf", &[]).expect("server still serves");
    assert_eq!(
        server
            .ask(&mut ok, r#"{"op":"health","id":3}"#)
            .get("id")
            .and_then(Json::as_i64),
        Some(3)
    );
}

/// A peer that stalls in the middle of a frame cannot hold a connection: the read deadline ends it and the
/// connection is dropped (slow-loris).
#[test]
fn it_004_ipc_timeout_part_peer_that_stalls_inside_a_frame_is_dropped() {
    let server = Server::start();
    let mut pipe = Pipe::connect_client(&server.name, within(Duration::from_secs(2))).unwrap();
    client_handshake(
        &mut pipe,
        &creds("tsf", &[]),
        &mut SystemEntropy,
        within(Duration::from_secs(2)),
    )
    .expect("handshake");
    let mut partial = frame::encode_header(FrameClass::Control, 200)
        .unwrap()
        .to_vec();
    partial.extend_from_slice(b"{\"op\":");
    pipe.write_all(&partial, within(Duration::from_secs(2)))
        .unwrap();
    let started = Instant::now();
    // the client now just sits there, connection open
    wait_until("server dropped the stalled connection", || {
        server
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.contains("Broken"))
    });
    assert!(
        started.elapsed() < Duration::from_millis(200 + 1500),
        "{:?}",
        started.elapsed()
    );
    drop(pipe);
}

/// The server caps simultaneous connections; a client over the cap is closed at once, never queued unbounded.
#[test]
fn it_004_connection_cap_drops_clients_over_the_limit_and_recovers() {
    let server = Server::launch(1, |_| {});
    let mut first = server.connect("tsf", &[]).expect("first client");
    assert_eq!(
        server
            .ask(&mut first, r#"{"op":"health","id":1}"#)
            .get("ok"),
        Some(&Json::Bool(true))
    );
    let second = server.connect("tsf", &[]);
    assert!(
        second.is_err(),
        "over the cap: {:?}",
        second.as_ref().map(|_| ())
    );
    drop(first);
    // after the first client left, a new one is served again
    wait_until("the slot is free again", || {
        server.connect("tsf", &[]).is_ok()
    });
}

/// Garbage on the pipe (a JSON Lines client, wrong magic) closes that connection and nothing else.
#[test]
fn ut_ipc_garbage_instead_of_a_hello_closes_the_connection() {
    let server = Server::start();
    let mut pipe = Pipe::connect_client(&server.name, within(Duration::from_secs(2))).unwrap();
    pipe.write_all(b"{\"op\":\"health\"}\n", within(Duration::from_secs(2)))
        .unwrap();
    let mut buf = [0u8; 256];
    let mut total = Vec::new();
    // the server may send one MALFORMED_FRAME error frame, then closes
    loop {
        match pipe.read_some(&mut buf, within(Duration::from_secs(3))) {
            Ok(n) => total.extend_from_slice(&buf[..n]),
            Err(IpcError::Closed) => break,
            Err(e) => panic!("unexpected {e:?}"),
        }
    }
    assert!(total.len() < 200);
    assert!(server.connect("tsf", &[]).is_ok());
}

// ---------------------------------------------------------------------------------------------------------------
// timeouts

/// IT-004 (the IPC part): the peer never answers; the caller returns within the limit, so Direct input is not held up.
/// NOT the whole IT-004: the TSF DLL and the real "Core suspended" scenario need a VM.
#[test]
fn it_004_ipc_timeout_part_peer_that_never_answers_times_out_within_the_limit() {
    let server = Server::start();
    let mut c = server.connect("tsf", &[]).unwrap();
    let started = Instant::now();
    let r = c.request_within(r#"{"op":"silent"}"#, Duration::from_millis(400));
    let took = started.elapsed();
    assert_eq!(r.err(), Some(IpcError::Timeout));
    assert!(
        took >= Duration::from_millis(350),
        "returned early: {took:?}"
    );
    assert!(
        took < Duration::from_millis(400 + 1000),
        "returned late: {took:?}"
    );
    assert!(
        c.is_broken(),
        "no late reply may be mistaken for the next answer"
    );
    assert_eq!(
        c.request(r#"{"op":"health"}"#).err(),
        Some(IpcError::Broken)
    );
}

/// A peer that stops reading must not block a large write beyond the write timeout (overlapped cancel path).
#[test]
fn it_004_ipc_timeout_part_write_to_a_peer_that_stopped_reading_times_out() {
    let server = Server::start();
    let mut c = server.connect("bridge", &[]).unwrap();
    c.send(FrameClass::Control, r#"{"op":"stall"}"#).unwrap();
    thread::sleep(Duration::from_millis(100));
    let big = format!(r#"{{"op":"echo","data":"{}"}}"#, "x".repeat(1_500_000));
    let started = Instant::now();
    let r = c.send_within(FrameClass::Bulk, &big, Duration::from_millis(300));
    let took = started.elapsed();
    assert_eq!(r.err(), Some(IpcError::Timeout));
    assert!(took < Duration::from_millis(300 + 1000), "{took:?}");
    assert!(c.is_broken(), "half a frame may have been written");
}

/// No server on the pipe: the client learns it at once (Direct must not wait), not after a timeout.
#[test]
fn it_004_ipc_timeout_part_missing_server_is_reported_immediately() {
    let name = unique_test_pipe_name().unwrap();
    let started = Instant::now();
    let r = connect(&name, &creds("tsf", &[]), &ClientConfig::default());
    assert_eq!(r.err(), Some(IpcError::Unavailable));
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "{:?}",
        started.elapsed()
    );
}

/// A server that is bound but never accepts: the client's handshake deadline ends the attempt.
#[test]
fn it_004_ipc_timeout_part_server_that_never_accepts() {
    let name = unique_test_pipe_name().unwrap();
    let cfg = ServerConfig::new(ServerPolicy::new(master(), roles()).unwrap());
    let _listener = Listener::bind(name.clone(), cfg).unwrap();
    let client_cfg = ClientConfig {
        timeouts: Timeouts {
            handshake: Duration::from_millis(300),
            ..Timeouts::default()
        },
        ..ClientConfig::default()
    };
    let started = Instant::now();
    let r = connect(&name, &creds("tsf", &[]), &client_cfg);
    assert_eq!(r.err(), Some(IpcError::Timeout));
    assert!(
        started.elapsed() < Duration::from_millis(300 + 1000),
        "{:?}",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------------------------------------------
// load

#[test]
fn it_003_many_sequential_connections() {
    let server = Server::start();
    for i in 0..150 {
        let mut c = server
            .connect("tsf", &[])
            .unwrap_or_else(|e| panic!("connection {i}: {e}"));
        let r = server.ask(&mut c, &format!(r#"{{"op":"health","id":{i}}}"#));
        assert_eq!(r.get("id").and_then(Json::as_i64), Some(i));
    }
}

#[test]
fn it_003_concurrent_clients_do_not_see_each_others_data() {
    let server = Arc::new(Server::start());
    let done = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..12)
        .map(|t| {
            let (server, done) = (Arc::clone(&server), Arc::clone(&done));
            thread::spawn(move || {
                for round in 0..15 {
                    let role = if (t + round) % 2 == 0 {
                        "tsf"
                    } else {
                        "bridge"
                    };
                    let mut c = server
                        .connect(role, &[])
                        .unwrap_or_else(|e| panic!("t{t} r{round}: {e}"));
                    for k in 0..3 {
                        let marker = format!("t{t}-r{round}-k{k}");
                        let r = server.ask(
                            &mut c,
                            &format!(r#"{{"op":"echo","id":{k},"data":"{marker}"}}"#),
                        );
                        assert_eq!(r.get("data").and_then(Json::as_str), Some(marker.as_str()));
                    }
                    done.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    for t in threads {
        t.join().expect("client thread");
    }
    assert_eq!(done.load(Ordering::SeqCst), 12 * 15);
}

// ---------------------------------------------------------------------------------------------------------------
// ACL, squatting, shutdown

/// The pipe carries the ACL we asked for: a protected DACL with exactly one ACE, for the current user's SID.
#[test]
fn it_003_pipe_acl_names_only_the_current_user() {
    let name = unique_test_pipe_name().unwrap();
    let listener = Listener::bind(
        name,
        ServerConfig::new(ServerPolicy::new(master(), roles()).unwrap()),
    )
    .unwrap();
    let sddl = listener
        .dacl_sddl()
        .expect("read the DACL back from the live pipe");
    let sid = current_user_sid_string().unwrap();
    println!("DACL of the live pipe: {sddl}");
    assert!(
        sddl.starts_with("D:P"),
        "protected, no inherited entries: {sddl}"
    );
    assert_eq!(sddl.matches("(A;").count(), 1, "one ACE only: {sddl}");
    assert_eq!(
        sddl.matches('(').count(),
        1,
        "no deny, audit or other entries: {sddl}"
    );
    assert!(
        sddl.contains(&format!(";;;{sid})")),
        "the ACE is for {sid}: {sddl}"
    );
    assert!(
        sddl.contains("0x12019f"),
        "generic read+write incl. create-instance, no change-ACL/owner/delete: {sddl}"
    );
}

#[test]
fn it_003_a_second_server_on_the_same_name_is_refused_first_instance_flag() {
    let name = unique_test_pipe_name().unwrap();
    let cfg = || ServerConfig::new(ServerPolicy::new(master(), roles()).unwrap());
    let first = Listener::bind(name.clone(), cfg()).unwrap();
    assert_eq!(
        Listener::bind(name.clone(), cfg()).err(),
        Some(IpcError::NameInUse)
    );
    // the squatting attempt does not disturb the real server
    drop(first);
    // once the owner has gone, the name is free again (the handles were really closed)
    assert!(Listener::bind(name, cfg()).is_ok());
}

/// A process that wins the name before the server starts (squatting): the server refuses to start on it
/// (FILE_FLAG_FIRST_PIPE_INSTANCE) and a client that reaches it refuses to talk, because the squatter cannot prove it
/// holds the key.
#[test]
fn it_003_squatter_that_owns_the_name_first_is_detected_by_both_sides() {
    let name = unique_test_pipe_name().unwrap();
    let sd = pime_ipc::winsec::SecurityDescriptor::for_current_user().unwrap();
    let mut rogue = Pipe::create_server_instance(&name, true, &sd, 4).unwrap();
    let rogue_thread = thread::spawn(move || {
        rogue
            .wait_for_client(within(Duration::from_secs(3)), None)
            .expect("a client arrives");
        let mut consumed = false;
        let (_, hello) = frame::read_frame(
            &mut rogue,
            &Limits::HANDSHAKE,
            within(Duration::from_secs(2)),
            &mut consumed,
        )
        .expect("hello");
        assert!(std::str::from_utf8(&hello).unwrap().contains("\"hello\""));
        // a plausible challenge with a proof made up without the key
        let fake = Json::obj(vec![
            ("t", Json::str("challenge")),
            ("major", Json::int(1)),
            ("minor", Json::int(0)),
            ("snonce", Json::Str("ab".repeat(32))),
            ("proof", Json::Str("cd".repeat(32))),
        ]);
        frame::write_frame(
            &mut rogue,
            FrameClass::Control,
            fake.to_json_string().as_bytes(),
            &Limits::HANDSHAKE,
            within(Duration::from_secs(2)),
        )
        .unwrap();
        // the client must hang up without sending its own proof
        frame::read_frame(
            &mut rogue,
            &Limits::HANDSHAKE,
            within(Duration::from_secs(2)),
            &mut consumed,
        )
        .err()
    });
    let cfg = ServerConfig::new(ServerPolicy::new(master(), roles()).unwrap());
    assert_eq!(
        Listener::bind(name.clone(), cfg).err(),
        Some(IpcError::NameInUse),
        "the real server detects the squatter"
    );
    let r = connect(&name, &creds("tsf", &[]), &ClientConfig::default());
    assert_eq!(
        r.err(),
        Some(IpcError::IdentityRejected),
        "the client detects the squatter"
    );
    assert_eq!(
        rogue_thread.join().unwrap(),
        Some(IpcError::Closed),
        "no client proof reached the squatter"
    );
}

/// MEASURED, and the reason the handshake carries the trust: the server creates one more pipe instance per client,
/// so the DACL must allow FILE_CREATE_PIPE_INSTANCE to the user, and then a second process of the same user can add an
/// instance to the live pipe as well. (The ACL still keeps other users out.) See docs/IPC.md section 5.
#[test]
fn it_003_same_user_process_can_add_an_instance_to_a_live_pipe_so_trust_rests_on_the_handshake() {
    let name = unique_test_pipe_name().unwrap();
    let listener = Listener::bind(
        name.clone(),
        ServerConfig::new(ServerPolicy::new(master(), roles()).unwrap()),
    )
    .unwrap();
    let sd = pime_ipc::winsec::SecurityDescriptor::for_current_user().unwrap();
    let extra = Pipe::create_server_instance(&name, false, &sd, 32);
    assert!(
        extra.is_ok(),
        "if Windows ever refuses this, the ACL got stricter: update docs/IPC.md section 5"
    );
    drop(extra);
    drop(listener);
}

#[test]
fn it_004_stop_handle_wakes_accept_and_ends_serve_and_frees_the_name() {
    let name = unique_test_pipe_name().unwrap();
    let cfg = || ServerConfig::new(ServerPolicy::new(master(), roles()).unwrap());
    let mut listener = Listener::bind(name.clone(), cfg()).unwrap();
    let stop = listener.stop_handle();
    let waker = thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        stop.signal();
    });
    let started = Instant::now();
    let r = listener.accept(Duration::from_secs(30));
    assert_eq!(r.err(), Some(IpcError::Stopped));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    waker.join().unwrap();
    // `serve` on a stopped listener returns at once
    assert!(listener.serve(4, |_c| {}).is_ok());
    drop(listener);
    assert!(
        Listener::bind(name, cfg()).is_ok(),
        "all pipe handles were closed"
    );
}

#[test]
fn it_004_accept_times_out_without_a_client_and_stays_usable() {
    let name = unique_test_pipe_name().unwrap();
    let mut listener = Listener::bind(
        name.clone(),
        ServerConfig::new(ServerPolicy::new(master(), roles()).unwrap()),
    )
    .unwrap();
    let started = Instant::now();
    assert_eq!(
        listener.accept(Duration::from_millis(200)).err(),
        Some(IpcError::Timeout)
    );
    assert!(started.elapsed() < Duration::from_millis(200 + 1000));
    // a client arriving after the timeout is still served on the same listener
    let name2 = name.clone();
    let client = thread::spawn(move || {
        connect(&name2, &creds("tsf", &[]), &ClientConfig::default()).map(|c| c.info().role.clone())
    });
    let conn = listener
        .accept(Duration::from_secs(3))
        .expect("accept after a timeout");
    assert_eq!(conn.info().role, "tsf");
    assert_eq!(client.join().unwrap(), Ok("tsf".to_string()));
}

/// A client that connected before the server called accept (ERROR_PIPE_CONNECTED: no event is ever signalled for it)
/// is accepted at once.
#[test]
fn it_003_client_connected_before_accept_is_accepted() {
    let name = unique_test_pipe_name().unwrap();
    let mut listener = Listener::bind(
        name.clone(),
        ServerConfig::new(ServerPolicy::new(master(), roles()).unwrap()),
    )
    .unwrap();
    let name2 = name.clone();
    let client = thread::spawn(move || {
        connect(&name2, &creds("bridge", &[]), &ClientConfig::default())
            .map(|c| c.info().role.clone())
    });
    // let the client open the pipe and send its hello before accept starts
    thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    let conn = listener.accept(Duration::from_secs(3)).expect("accept");
    assert_eq!(conn.info().role, "bridge");
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(client.join().unwrap(), Ok("bridge".to_string()));
}

/// All instances busy (limit 1, one client connected): a second client waits only until its connect deadline.
#[test]
fn it_004_ipc_timeout_part_busy_pipe_waits_only_until_the_connect_deadline() {
    let name = unique_test_pipe_name().unwrap();
    let mut cfg = ServerConfig::new(ServerPolicy::new(master(), roles()).unwrap());
    cfg.max_instances = 1;
    let mut listener = Listener::bind(name.clone(), cfg).unwrap();
    let name2 = name.clone();
    let first =
        thread::spawn(move || connect(&name2, &creds("tsf", &[]), &ClientConfig::default()));
    let held = listener
        .accept(Duration::from_secs(3))
        .expect("first client");
    let _first_client = first.join().unwrap().expect("first handshake");
    let quick = ClientConfig {
        timeouts: Timeouts {
            connect: Duration::from_millis(300),
            ..Timeouts::default()
        },
        ..ClientConfig::default()
    };
    let started = Instant::now();
    let r = connect(&name, &creds("tsf", &[]), &quick);
    assert_eq!(r.err(), Some(IpcError::Timeout));
    assert!(
        started.elapsed() < Duration::from_millis(300 + 1000),
        "{:?}",
        started.elapsed()
    );
    drop(held);
}

/// A panic in the user's handler ends that connection only.
#[test]
fn it_004_handler_panic_does_not_take_the_server_down() {
    let server = Server::start();
    let mut c = server.connect("tsf", &[]).unwrap();
    c.send(FrameClass::Control, r#"{"op":"panic"}"#).unwrap();
    assert!(matches!(
        c.recv_within(Duration::from_secs(3)),
        Err(IpcError::Closed) | Err(IpcError::Truncated)
    ));
    let mut ok = server
        .connect("tsf", &[])
        .expect("server survived the panic");
    assert_eq!(
        server
            .ask(&mut ok, r#"{"op":"health","id":5}"#)
            .get("id")
            .and_then(Json::as_i64),
        Some(5)
    );
}

#[test]
fn it_004_clean_shutdown_closes_clients_and_returns_ok() {
    let mut server = Server::start();
    let mut c = server.connect("tsf", &[]).unwrap();
    assert_eq!(
        server.ask(&mut c, r#"{"op":"health","id":1}"#).get("ok"),
        Some(&Json::Bool(true))
    );
    let name = server.name.clone();
    server.shutdown();
    assert!(matches!(
        c.recv_within(Duration::from_secs(2)),
        Err(IpcError::Closed) | Err(IpcError::Truncated)
    ));
    // the endpoint is gone: a new client is told at once
    assert_eq!(
        connect(&name, &creds("tsf", &[]), &ClientConfig::default()).err(),
        Some(IpcError::Unavailable)
    );
}

#[test]
fn ut_ipc_per_user_pipe_name_is_built_from_the_token_sid() {
    let sid = current_user_sid_string().unwrap();
    let name = PipeName::for_sid(&sid).unwrap();
    assert_eq!(name.as_str(), format!(r"\\.\pipe\PIME-IPC-1-{sid}"));
}
