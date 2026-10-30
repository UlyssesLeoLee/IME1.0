# PIME-IPC/1: named-pipe transport (RUST_MIGRATION_PLAN P3)

Status 2026-10-30: implemented in `crates/pime-ipc` and verified **on one machine, one user, in one process** (loopback). Everything that needs a second
Windows user, a second session, a remote machine, the TSF DLL or a VM is listed in section 8 as **NOT RUN**. Design sources: Detailed Design (DD) 2.1
(transport and security), 2.2 (message model), 2.3 (operations), 12.2 (`IPC_TIMEOUT`), 15.2 (limits); Basic Design 7.2, 13.1, 16.1; IT-003, IT-004,
UT-064, UT-067, ST-037.

## 1. What it is

One authenticated, size-limited, deadline-bound message channel between the user's processes: TSF text service, Core, AI Worker, UI, editor bridge.

```
\\.\pipe\PIME-IPC-1-<SID of the current user>        one endpoint per user, served by Core
```

Layers (all in `crates/pime-ipc`, outside the main workspace, own `Cargo.lock`, `windows` pinned `=0.58.0` like `pime-tsf`; `pime-core` is a path dependency
used only for SHA-256/HMAC (`sha256.rs`) and the JSON parser (`json.rs`), it gains no dependency and stays `forbid(unsafe_code)`):

| Module | Content | Windows needed |
|---|---|---|
| `frame` | 8-byte header, limits, `Transport` trait | no |
| `auth` | keys, HMAC transcripts, `ct_eq`, `NonceWindow`, `Version`, `PipeName`, SDDL builder | no |
| `handshake` | mutual challenge-response, capability token, version negotiation | no (generic over `Transport`) |
| `connection` | `Connection<T>`: `send`/`recv`/`request`, fail-closed state, `Timeouts` | no |
| `winsec` | process-token SID, security descriptor from SDDL, session id, `BCryptGenRandom` | yes (`unsafe`) |
| `pipe` | overlapped pipe I/O with deadlines, server instances, client open | yes (`unsafe`) |
| `endpoint` | `Listener` (bind/accept/serve/stop), `connect` | yes |

`unsafe` is `deny`-ed crate-wide and allowed in exactly `pipe` and `winsec` (compiler-enforced; `tests/smoke/ipc-smoke.js` also scans for it). Every `unsafe`
block is one FFI call with a `SAFETY` comment.

## 2. Frame format

```
offset 0  2 bytes  magic "PI"
offset 2  1 byte   class: 0 control, 1 bulk (anything else: malformed)
offset 3  1 byte   reserved, must be 0
offset 4  4 bytes  payload length, u32 little endian, 1 ..= limit of the class (0: malformed)
offset 8  n bytes  payload: UTF-8 text that is one JSON object
```

| Limit | Value | Source |
|---|---|---|
| before authentication (handshake frames) | 4 KiB, control only | chosen: nothing bigger is buffered from a peer that has not proved anything |
| control frame | 64 KiB | DD 2.1, 15.2 |
| bulk frame (snapshot text) | 2 MiB | DD 2.1, 15.2 |
| hard maximum any configuration may reach | 16 MiB (`pime_core::MAX_BULK_LINE_BYTES`) | `Limits::new` refuses more |

- The header is validated completely (magic, class, reserved, zero length, size) **before one payload byte is buffered**; `u32::MAX` cannot wrap or allocate.
  A sender that exceeds its limit gets `PAYLOAD_TOO_LARGE` locally and nothing is written: frames are never truncated silently (DD 15.2).
- Every delivered frame has been checked to be UTF-8 and a JSON object (depth <= 16, the limit of protocol 0.3, through `pime_core::json::parse`, which also
  rejects duplicate keys). `Frame.text` is exactly a JSON Lines request/response line without its newline, so `pime_core::protocol::handle_line` can consume it.
- Errors on the wire use the JSON Lines convention `{"ok":false,"error":"CODE"}`. Codes: `PAYLOAD_TOO_LARGE`, `MALFORMED_FRAME`, `PROTOCOL_MISMATCH`
  (also carries the supported `major`/`minor`), `IDENTITY_REJECTED` (the one code for every identity failure: unknown role, wrong proof, replay, stale challenge,
  wrong session), `CAPABILITY_DENIED`. Timeouts and closes are local conditions and send nothing.
- Fail closed: a malformed, oversized or non-UTF-8/JSON frame sends one coarse error frame (best effort, 200 ms) and the connection is closed and marked
  broken; every later call returns `Broken`. The decoder never panics (3000 pseudo-random inputs in `ut_ipc_frame_decoder_survives_pseudo_random_input`).

**Open point for the integrator.** The stdio protocol 0.3 allows 16 MiB request lines because a 2 MiB document can grow to about 12 MiB when every
character needs a `\u00XX` escape. The DD limit for a snapshot frame is 2 MiB, so such a document does not fit one bulk frame; the DD answer is chunking or
a smaller range ("可降级分块传输"), which is application-layer work not done here. Raise `Limits::new(control, bulk)` (up to 16 MiB) if the product prefers to keep one-frame snapshots.

## 3. Handshake

```
client                                                    server
  hello   {t, major, minor, role, caps[], cnonce}  ---->  frame/UTF-8/JSON checks; major must match (else PROTOCOL_MISMATCH);
                                                          role must exist; cnonce must not be in the replay window
          <----  challenge {t, major, minor, snonce, proof = HMAC(Krole, "server-proof" | binding)}
  verifies the server's proof; on mismatch it hangs up WITHOUT sending anything
  auth    {t, proof = HMAC(Krole, "client-proof" | binding)}  ---->  constant-time compare; challenge must not be older than challenge_ttl;
                                                          cnonce recorded atomically (a concurrent twin is refused)
          <----  ready {t, session, token, caps[], ttl_ms, mac = HMAC(session_key, "ready" | ...)}
```

- **Version negotiation** is in the first frame (DD 2.1): `major` must be equal (`PROTOCOL_MISMATCH` otherwise, with the supported version in the error frame);
  the lower `minor` wins. Both versions are inside every proof, so a downgrade of the negotiated version breaks the MAC.
- **Keys.** `Kmaster` is a per-user secret of 32..=128 bytes (`Secret`: never printed, wiped on drop best-effort). `Krole = HMAC(Kmaster, "role-key" | role)`:
  clients get only their role key, which cannot be turned into another role's key or into `Kmaster`.
- **binding** = both nonces, the role, the requested capabilities, both versions, every field length-prefixed under the fixed prefix `PIME-IPC/1` and a label
  (`transcript()`); the labels differ for the server proof, client proof, session key and `ready`, so a proof cannot be reflected. HMAC-SHA-256 is
  `pime_core::sha256::hmac_sha256` (reused, not re-implemented), checked against RFC 4231 test cases 1, 2, 3, 4, 6, 7 again in `auth::tests`.
- **Mutual.** `FILE_FLAG_FIRST_PIPE_INSTANCE` only stops a second *server* from joining; a client can still reach a squatter. Therefore the server proves it holds
  the key first and the client refuses to answer a server that cannot.
- **Replay.** Nonces are 256-bit from `BCryptGenRandom(BCRYPT_USE_SYSTEM_PREFERRED_RNG)` (no other source, no fallback). The server remembers client nonces of
  *authenticated* handshakes for 120 s (cap 65,536; a full table refuses, it does not forget) and never trusts a peer timestamp: time is the server's own
  monotonic clock (DD 2.2). A replayed `hello` is refused at once; a replayed proof with a fresh nonce fails because the server's nonce differs; a proof that arrives after
  `challenge_ttl` (1 s) is refused as stale.
- **Capabilities.** `RolePolicy { role, capabilities }` is the server's table. The client's request is granted exactly or refused (`CAPABILITY_DENIED`), never
  silently trimmed; an empty request means the whole role. The granted list is inside the `ready` MAC.
- **Capability token.** 128-bit random, issued per connection in `ready`, valid `token_ttl` (15 min). `SessionInfo::verify_token` compares in constant time
  and refuses an expired token. It is the credential a Core dispatcher checks on privileged operations, so a message relayed from another connection does not carry authority.
- **Session id.** 128-bit random, the DD `session_id`.

## 4. Timeouts

DD 2.1 requires explicit deadlines ("Bridge 请求与 Apply 有显式截止时间"; `IPC_TIMEOUT` in 12.2; "TSF 输入路径不等待 Core/AI") but **fixes no numbers**. The values below are
chosen here from what the design does say: the `ttl_ms: 500` of the `PredictLocal` example (DD 2.2), and NFR-102 / UT-067 / IT-004 / ST-037 (Direct input within 2 s of a Core failure).

| Timeout | Default | Why |
|---|---|---|
| connect (`Timeouts::connect`) | 500 ms | includes waiting for a busy instance; a missing server (`ERROR_FILE_NOT_FOUND`) is reported at once as `Unavailable`, not after 500 ms |
| handshake (`Timeouts::handshake`), whole exchange | 1000 ms | connect + handshake = 1.5 s < 2 s, so the slowest path to "give up and go Direct" stays inside NFR-102 |
| read (`Timeouts::read`), also the reply wait of `request` | 500 ms | the 500 ms `ttl_ms` of DD 2.2; callers pass their own deadline (`*_within`) for Bridge requests and Apply |
| write (`Timeouts::write`) | 500 ms | same |
| challenge lifetime (`ServerPolicy::challenge_ttl`) | 1 s | equal to the handshake deadline |
| token lifetime (`ServerPolicy::token_ttl`) | 15 min | "short-lived" in DD 2.1; no design number |
| replay window (`nonce_window`) | 120 s | chosen; the proof binds the server's fresh nonce anyway |

Every blocking call is an overlapped Win32 operation waited on with the remaining time. On expiry the operation is cancelled (`CancelIoEx`) and **waited for**
(`GetOverlappedResult(bWait = TRUE)`) before the buffer and `OVERLAPPED` go out of scope, so a stuck peer can never block the caller beyond its deadline and never
leaves a dangling I/O (tests: silent peer, peer that stopped reading a 1.5 MB write, peer that stalls inside a frame, server that never accepts, busy pipe).
After a failed write, a timeout inside a frame, or any failed `request`, the connection is broken: a late reply must never be read as the answer to the next
request, and DD 2.1 forbids a blind repeat of a write. Only an *idle* `recv` timeout (nothing of a frame had arrived) is recoverable, so a server loop can poll its stop flag.

These timeouts guarantee that a *background* IPC thread gives up in time. They do **not** make a TSF callback safe: the TSF input path must not call this crate
synchronously at all (DD 1.1 rule 1, DD 2.1); see section 7.

## 5. Pipe, ACL and endpoint hardening

Server (`Listener::bind` / `bind_current_user`):

- `CreateNamedPipeW` with `PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE` (first instance only), byte mode,
  `PIPE_REJECT_REMOTE_CLIENTS`, `nMaxInstances` 32 (configurable). A second server on a taken name gets `NameInUse`; after the owner is dropped the name is free again
  (both tested, which proves all handles are closed).
- A listening instance always exists: the replacement instance is created before the connected one is handed out.
- **ACL.** The security descriptor is built from the SID of the process token (`OpenProcessToken` + `TokenUser` + `ConvertSidToStringSidW`), validated strictly
  (`S-1-` + digits only; an injection string such as `S-1-5-21-1)(A;;GA;;;WD` is refused, not escaped) and converted with
  `ConvertStringSecurityDescriptorToSecurityDescriptorW`:

  ```
  D:P(A;;0x12019f;;;<user SID>)      protected DACL, one ACE, FILE_GENERIC_READ | FILE_GENERIC_WRITE, nobody else
  ```

  Test `it_003_pipe_acl_names_only_the_current_user` reads the DACL back from the live pipe (`GetSecurityInfo` + SDDL conversion) and asserts: protected DACL, a single ACE, this SID, this mask.
  Not `GA`: no `WRITE_DAC`, `WRITE_OWNER`, `DELETE`.
- **Measured trade-off (do not "tighten" without reading this).** The mask had to include `FILE_CREATE_PIPE_INSTANCE`: Windows checks the creation of every
  further instance against this DACL for the server's own token, and with `0x120083` (read/write data) and `0x120087` (plus create-instance) the server's
  second instance could not be created (`ERROR_ACCESS_DENIED` was observed with `0x120083`; clients after the first one got `Unavailable`). Consequence: another process **of the same user** can add an
  instance to the live pipe (test `it_003_same_user_process_can_add_an_instance_to_a_live_pipe_so_trust_rests_on_the_handshake`). Other users still cannot. What this lets a
  same-user process do is limited by the handshake, see section 6. A hardened alternative (not done): create the instance pool at start-up, remove the right with
  `SetSecurityInfo`, recycle instances with `DisconnectNamedPipe`; it needs a bounded flush before disconnecting.
- **Session check.** After `ConnectNamedPipe` the server compares `GetNamedPipeClientSessionId` with its own session (`ProcessIdToSessionId`) and drops other-session
  clients (`ServerConfig::require_same_session`, default on; DD 2.1: "连接拒绝其他用户/会话"). Fail closed: if the client's session cannot be read, the client is refused.
- PID, window title or a localhost address are not used for any decision (DD 2.1).

Client (`connect`): `CreateFileW` with `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION` (a squatting server cannot impersonate the client), overlapped, only the access
rights it needs; `ERROR_PIPE_BUSY` waits (`WaitNamedPipeW`, steps of at most 100 ms) until the connect deadline; `ERROR_ACCESS_DENIED` is `IdentityRejected`.

Shutdown: `Listener::stop_handle()` returns a `StopHandle` (an event). Signalling it wakes a blocked `accept` with `Stopped`, ends `serve`, **and aborts every pending and later read/write of every
connection accepted by that listener with `Stopped`**, so shutdown does not depend on the handler watching the handle: a handler that returns on any error other than an idle `Timeout` ends at
once (the test handler is deliberately naive and a client is stuck in an unanswered request; `serve` returns in well under 1.5 s). A handler that swallows every error and loops would keep `serve` waiting; that is a bug in the handler.
`serve` runs one thread per authenticated connection, at most `max_connections` (over the cap, or if no thread can be created, the client is closed at once; the slot is a drop guard, so a panic anywhere in the
worker cannot leak it), catches panics of the handler (`catch_unwind`: only that connection ends) and joins its threads on exit. Closing is `CloseHandle` without `DisconnectNamedPipe` and without `FlushFileBuffers`: disconnecting discards data the
peer has not read yet (the error frame before a close would be lost), flushing blocks while the peer is stuck. Test: the `PAYLOAD_TOO_LARGE` frame is read after the server closed.

## 6. Threat model

What the **ACL + endpoint flags** defend against:

| Threat | Defence |
|---|---|
| another Windows user connecting (DD 16.1 "跨用户 IPC 访问") | DACL names only the user's SID (read back from the live pipe: tested). The refusal itself is NOT RUN (needs a second user) |
| a network client reaching the pipe | `PIPE_REJECT_REMOTE_CLIENTS` is set; its effect is NOT RUN (needs a second machine) |
| another logon session of the same user | session id comparison; the refusal is NOT RUN (needs a second session) |
| pre-creating the pipe name before Core starts (squatting) | `FILE_FLAG_FIRST_PIPE_INSTANCE`: the real server refuses to start on a taken name; the client additionally refuses a server without the key (test with a rogue first instance) |
| the server impersonating the connecting client | `SECURITY_IDENTIFICATION` |
| oversize/garbage from any connected peer | limits before allocation, 4 KiB before authentication |

What the **HMAC handshake** defends against (it must, because the ACL cannot tell two processes of the same user apart):

| Threat | Defence |
|---|---|
| a same-user process without the key connecting | cannot produce the proof; refused (`IDENTITY_REJECTED`), the server keeps serving |
| a rogue server (squatter or added instance) | cannot produce the server proof; the client sends nothing and closes |
| replaying a recorded handshake | client nonce window, fresh server nonce per challenge, stale-challenge limit |
| a client claiming a role or capabilities it was not given | role keys are derived per role; capabilities are granted exactly or refused |
| tampering with `ready` (capabilities, token) or the version | `ready` MAC under the session key; versions are inside every proof |
| reflecting one side's proof back | different labels per proof |
| timing side channel on the comparison | `ct_eq` (XOR accumulate + `black_box`) |

What **neither** defends against (residual risk; DD 2.1: "同一 Windows 用户下其他恶意进程仍属剩余风险"):

- **Malware running as the same user that can read the key** (wherever it is stored), read the memory of a legitimate process, or inject into it. It *is* the user.
- **A same-user relay without the key.** Because the DACL has to allow instance creation (section 5), such a process can accept a client and forward the handshake
  frames unchanged to the real server, which succeeds without either side noticing. **Frames after the handshake are not authenticated or encrypted** (`session_key` is derived and
  covers `ready`, but no per-frame MAC exists), so the relay could then read and alter traffic. This is the largest known gap; the recommended follow-up is a per-frame MAC
  (HMAC with direction keys from `session_key` and a sequence number) and, if source text must be hidden from such a process, encryption. Not done.
- `role` is **self-declared** by whoever holds the key of that role; with one `Kmaster` per user the roles separate keys, not people. Process identity (executable signature, extension
  origin, `editor_instance_id`; Basic Design 7.2) is **not** verified by this crate.
- Denial of service by a same-user process: it can add instances, open many connections (bounded by `max_instances` and `max_connections`; each pre-authentication connection costs
  one thread for at most the handshake deadline) or sit on the pipe. Availability is not claimed; the product must fall back to Direct (section 7).
- A stolen capability token is valid on any connection that is able to present it until it expires; it is bound to nothing else.
- Key compromise: no rotation or revocation protocol; replace the key and restart.
- The key storage is **not** implemented (section 9). Wiping secrets from memory is best effort (safe Rust cannot force it).
- Confidentiality on the wire: none (local pipe, same user). Nothing secret (API keys) may ever be sent over it (Basic Design 13.2: payloads never carry provider keys).

## 7. How the components use it

One endpoint per user, `\\.\pipe\PIME-IPC-1-<SID>`, **served by Core**; everybody else is a client with a role. Roles and capability names are the policy of the Core, not of this crate;
a proposal following DD 2.3:

| Role | Process | Capabilities (proposal) | Notes |
|---|---|---|---|
| `tsf` | `pime_tsf.dll` in host processes | `predict.local`, `cancel`, `state` | never waits on the pipe in a TSF callback (below) |
| `bridge` | `pime-bridge.exe` for the VS Code shell | `snapshot`, `edit` | see below |
| `ui` | `pime-ui.exe` | `candidates`, `cancel` | |
| `worker` | `pime-ai-worker.exe` | `ai.predict` | the Worker connects to Core as a client; after the handshake the channel is symmetric, so Core can send `PredictAI` down it |

- **Core:** `Listener::bind_current_user(ServerConfig::new(policy))`, then `serve(max, handler)`; the handler loops `conn.recv_within(250 ms)`, checks
  `conn.info().has_capability(..)` and `verify_token(..)`, feeds `frame.text` to the operation dispatcher and sends the reply (`FrameClass::Bulk` for snapshots). A
  `Core` instance is not shared across connection threads by this crate: the integrator decides between one mutex-guarded `Core` and per-connection sessions.
- **TSF:** the input path never calls this crate. A background thread owns the connection, `connect` with the default timeouts, and exchanges requests through a non-blocking
  handoff; on `Unavailable`, `Timeout` or any error the text service drops to Direct, cancels old candidates (`IPC_TIMEOUT`, DD 12.2) and reconnects with back-off. Hosts that block
  pipe access (AppContainer) simply never connect: Direct (Basic Design 5, RV-02). Handling of the 32-bit build and of the key for DLLs in arbitrary hosts is open (section 9).
- **AI Worker / UI:** clients as above; Worker requests carry the capability token of their own connection.
- **VS Code shell:** the JavaScript glue must not contain authorization, hashing or other security logic (plan 1.2, 5.5), so it does **not** speak PIME-IPC. `pime-bridge.exe` (P6) is the
  PIME-IPC client with role `bridge` and keeps the stdio JSON Lines link to the extension.

Minimal use:

```rust
let policy = ServerPolicy::new(master_key, vec![RolePolicy { role: "tsf".into(), capabilities: vec!["predict.local".into()] }])?;
let mut listener = Listener::bind_current_user(ServerConfig::new(policy))?;
let role_key = listener.role_key("tsf");                       // what the installer would hand to the TSF DLL
listener.serve(16, |mut conn| loop {                            // returns after stop_handle().signal()
    match conn.recv_within(Duration::from_millis(250)) {
        Ok(frame) => { /* dispatch frame.text, conn.send(..) */ }
        Err(IpcError::Timeout) => continue,                     // idle only; everything else, including Stopped, ends the handler
        Err(_) => return,
    }
})?;

let creds = ClientCredentials::new("tsf", role_key, &["predict.local"]);
let mut c = connect(&PipeName::for_sid(&current_user_sid_string()?)?, &creds, &ClientConfig::default())?;
let reply = c.request_within(r#"{"op":"health","id":1}"#, Duration::from_millis(500))?;
```

## 8. What is verified, what is NOT

Run on this machine (Windows 11, `x86_64-pc-windows-gnu`, Rust 1.99.1, debug build), from `01_Source`:

```
node tests/smoke/ipc-smoke.js --repeat=3      # runs cargo test 3 times; exact runner command: node tests/smoke/ipc-smoke.js
cd crates/pime-ipc && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo test --workspace                       # from 01_Source: unchanged, 220 tests (the crate is outside the workspace)
```

Results: 61 unit tests + 30 loopback tests per run, 0 failed, identical in every run (`ipc-smoke.js --repeat=3`: 91 tests x 3). Also run with 32 test threads (3 runs), with one test thread, and with 8 busy-loop processes hogging the CPU (3 runs
of the loopback suite plus the unit tests), all green.
**Naming caveat.** Tests whose names carry `it_003` or `it_004` (design-case ids, as the project convention asks) are the parts of those cases that can run on one machine without TSF. They do **not** make IT-003 or IT-004 passed:
the rows below stay NOT RUN. `scripts/traceability-report.js` maps Rust tests only by `ut_NNN` today, so nothing here is counted; if it is ever extended to `it_NNN`, exclude `crates/pime-ipc` or rename these tests.

Unit tests: frame encode/decode (truncated at every cut point, oversized from the header alone, zero length, bad magic/class/reserved, chunked reads, pseudo-random input);
HMAC RFC 4231 cases 1, 2, 3, 4, 6, 7 (including keys longer than the block); constant-time compare; transcript unambiguity; proof binding of every input;
role-key independence; nonce window (replay, expiry, full table, clock stepping back); version negotiation (UT-ipc: different major, lower minor); SID validation and SDDL
injection; pipe names; the whole handshake over an in-memory transport (right key, wrong key, other role's key, unknown role, over-asked capability, replay, concurrent twin nonce, stale challenge,
squatting server, tampered `ready`, malformed hello variants, 4 KiB limit before authentication, silent peers, token expiry); the framing layer (UTF-8/JSON checks, error frame before close, idle versus partial timeouts, request timeout).
Loopback tests (`tests/loopback.rs`, each on its own `\\.\pipe\PIME-IPC-1-TEST-<random>`): round trip with token and session agreement; wrong key; replayed handshake (same hello,
recorded proof with a fresh nonce); stale challenge; oversized frame from the header alone with the error frame still delivered; garbage instead of a hello; client vanishing mid-frame (server survives, saw
`Truncated`); stalled peer inside a frame; 1.5 MB bulk round trip; sender-side limits; 150 sequential connections; 12 concurrent clients x 15 connections x 3 requests without crosstalk;
connection cap; panic in a handler; clean shutdown; stop handle waking `accept`; client connected before `accept`; busy pipe; the DACL read back from the live pipe; a second server refused (`NameInUse`) and the name free
after shutdown; a squatter that owns the name first; the measured same-user-instance behaviour of section 5.
Measured (not a requirement): one small request/response round trip over the pipe, debug build, 400 requests: p50 12 to 45 us, p95 19 to 61 us, max about 100 us (includes JSON validation on both ends).

**NOT RUN / NOT VERIFIED here:**

| Case | Why not | Needed |
|---|---|---|
| IT-003 "different SID rejected" (the pipe refuses another user) | needs a second Windows user or a restricted/low-integrity token launched in a VM | VM with two accounts |
| remote client refused (`PIPE_REJECT_REMOTE_CLIENTS` effect) | needs a second machine; `\\localhost\pipe\..` goes through SMB, which is a network call | two machines |
| client of another logon session refused | needs a second session (fast user switching / RDP) | VM |
| IT-004 in full: real TSF DLL + fake Core suspended 3 s, 100 characters typed, Direct <= 2 s after recovery | the TSF DLL is not wired to IPC and may not be loaded here; only the transport timeouts are tested | VM + `pime_tsf.dll` (P4) |
| ST-037 kill Worker/Core/UI, Direct <= 2 s | same | VM |
| UT-064 / UT-067 | the transport part (timeout -> error, no panic, give-up inside 1.5 s) is covered above; the Direct state machine and the virtual-scheduler measurement belong to the TSF side | P4 |
| AppContainer or other restricted hosts opening the pipe | none available | VM |
| 32-bit clients | `pime-ipc` was built for x86_64 only | i686 toolchain |
| performance under the NFR-201 workload and in a release build | only a debug-build round trip was measured | benchmark |
| key provisioning, rotation, per-frame MAC, encryption | not implemented (sections 6, 9) | design decision |
| process-identity binding (executable signature, extension origin) | not implemented | design decision |
| fuzzing beyond the 3000-input smoke test | no fuzzer here | |

## 9. Open points and notes for the integrator

1. **Key provisioning** is the main unsolved piece: where `Kmaster` lives and how each client class gets its role key. Candidates: a per-user file under `%LOCALAPPDATA%` with a DACL for
   the user only, written by the installer (P5) or by Core at first start, optionally DPAPI-wrapped; the TSF DLL in an arbitrary host process must be able to read it, which an AppContainer host cannot.
2. **Per-frame MAC** (section 6) closes the relay gap for tampering; do it before trusting `edit`/`apply` traffic across this pipe.
3. **Snapshot size** versus the 2 MiB frame (section 2).
4. **Integration files this work item did not touch** (manifest): root `01_Source/Cargo.toml` needs `exclude = ["crates/pime-tsf", "crates/pime-ipc"]`; `tests/run-all.js` needs an `ipc`
   step, for example in the `FAST` list and the `--` pattern, `if (want('ipc')) step('PIME-IPC/1 (crates/pime-ipc): unit + loopback tests on random pipe names; windows pinned 0.58; unsafe only in the FFI modules', () => sh(process.execPath, [path.join(tests, 'smoke', 'ipc-smoke.js')], {env: {...process.env, CARGO_TARGET_DIR: targetDir}}));`
   `tests/README.md` could list `ipc-smoke.js`; `docs/VERSIONS.md` already covers the `windows` 0.58 pin.
5. The integration tests are in `crates/pime-ipc/tests/loopback.rs` (wired by an explicit `[[test]]` entry) because the work item could only write inside the crate; moving them to
   `tests/rust/` is a one-line path change in `Cargo.toml`.
6. The test server and clients use the real per-user SID only to compute a name (`ut_ipc_per_user_pipe_name_is_built_from_the_token_sid`); no test binds the real `PIME-IPC-1-<SID>` pipe.
