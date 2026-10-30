# AI worker (`crates/pime-ai-worker`, plan phase P2)

`pime-ai-worker.exe` is the only process that ever sees an API key. It sends HTTPS requests to AI providers through WinHTTP (SChannel: the
Windows trust store and the system proxy settings, no TLS stack of our own) and keeps keys in Windows Credential Manager. A key is stored once
(`cred.set`) and from then on it is only injected into an outgoing request header by this process. **There is deliberately no operation that
returns a key.**

Status (2026-10-30): the worker, its protocol and its tests exist and pass on the development machine; nothing in the VS Code extension or in
`pime-core` calls it yet (see "How it connects to the rest"). Decisions already taken: TLS through WinHTTP (plan P2, option A); the `windows`
crate pinned to `=0.58.0` on the GNU target (see `docs/VERSIONS.md`); a crate outside the main workspace (own `Cargo.lock`, listed in the root
`exclude`) because it needs `unsafe`.

## Running it

```text
pime-ai-worker.exe                      # the product: no options
pime-ai-worker.exe --help
```

JSON Lines over stdin/stdout, the framing and error conventions of `pime-core` protocol 0.3 (one JSON object per line, one reply line per
request line, optional integer `id` echoed in the reply, `{"ok":false,"error":"TOKEN"[,"field":"name"]}` for failures). The worker never logs;
stderr carries only fixed start-up messages. End of input aborts every request in flight and exits.

The product start takes no option. Three **test-only** options exist, each one loosens something, and **each refuses to start (exit code 2)
unless the environment variable `PIME_WORKER_TEST_MODE=1` is also set**, so a stray flag cannot weaken a real installation:

| Option | Effect |
|---|---|
| `--insecure-test-ca` | Accept a certificate whose authority is not trusted (`SECURITY_FLAG_IGNORE_UNKNOWN_CA`), for **loopback hosts only**. Host name and validity dates are still checked; every other host is verified against the system store as always. Used to trust the UAT mock provider's throw-away self-signed certificate. |
| `--test-allow-loopback-http` | Accept plain `http://` for `127.x.x.x` and `localhost`. |
| `--test-credential-namespace N` | Use Credential Manager targets `N/<provider>` with **session** persistence (the entries disappear at log-off even if a test was killed). `N` must start with `PIME-TEST-`, so a test can never address a real credential. |

## Protocol

Every operation takes and returns the fields below; unknown request fields are refused (a mistyped `credentialRef` must not silently send a
request without a key). `http.send` answers on its own thread and **replies are matched by `id`, not by order**; every other operation answers
before the next line is read, so an `http.cancel` always finds the `http.send` that precedes it.

| Operation | Request | Reply (`ok:true`) |
|---|---|---|
| `health` | - | `name`, `version`, `protocol:"0.3"`, `transport:"winhttp"`, `ops`, `namespace`, `testMode`, `insecureTestCa`, `allowLoopbackHttp`, `limits` |
| `http.send` | `id` (required, unique among requests in flight), `method` (`GET`\|`POST`), `url`, `headers?` (allow-list), `body?` (string, `POST` only), `timeoutMs?` (1..300000, default 30000), `credentialRef?` + `authHeader?` (both or neither) | `status`, `headers` (allow-list, lower-case names), `body` (text; invalid UTF-8 becomes U+FFFD), `bodyBytes`, `redactions`, `elapsedMs` |
| `http.cancel` | `request` = the `id` of an `http.send` | `cancelled` (`true` only when this call aborted a request that was still in flight) |
| `cred.set` | `credentialRef`, `host` (`host` or `host:port`, the origin the key may be sent to), `secret` | `stored:true`, `host` (canonical `host:port`) |
| `cred.exists` | `credentialRef` | `exists`, and `host` when it exists (never the key) |
| `cred.delete` | `credentialRef` | `deleted` (`false` when there was none) |

A non-2xx provider answer is a **normal reply** (`ok:true`, `status:429` ...): mapping HTTP statuses to product errors stays with the caller
(`mapHttpStatus` in the JavaScript client, later the core). `ok:false` means the worker could not complete the exchange.

**Credential injection.** `credentialRef` is a provider id (`[A-Za-z0-9._-]{1,64}`); the Credential Manager target is
`ProgrammingIME/<credentialRef>`. `authHeader` is a template `<Name>: <prefix>{key}<suffix>` with exactly one `{key}`; the name must be
`Authorization` or `x-api-key`, the prefix/suffix are at most 32 characters of `[A-Za-z0-9 _.-]`, for example `Authorization: Bearer {key}` or
`x-api-key: {key}`. The worker reads the key, renders the header line, sends the request and wipes its copies. The caller's own `headers` may
only be `Content-Type`, `Accept`, `anthropic-version`, `anthropic-beta`, `openai-organization`, `openai-project`, `x-request-id`; credential
headers, `Host`, `Content-Length`, `Cookie` and hop-by-hop headers are refused (`HEADER_NOT_ALLOWED`).

**Origin binding.** `cred.set` stores the origin the key belongs to (in the credential's `UserName` field, visible in the Credential Manager UI).
`http.send` refuses (`CREDENTIAL_ORIGIN_MISMATCH`) a URL whose `host:port` differs, so a compromised caller cannot make the worker send a key to
an attacker's server. Changing a provider's endpoint therefore needs the key to be entered again (the same rule as a new grant for a new host).

**Keys.** 8..1024 printable ASCII characters without space, `"` and `\` (every real provider key qualifies); anything else is refused
(`INVALID_REQUEST`, field `secret`) without echoing it. The key is stored as UTF-16 like `cmdkey` does, so it can be replaced in the Control
Panel.

**Errors.** Failures carry one of these tokens in `error`, plus `field` (a name from this protocol, or `?`) and, for unknown operating-system
errors, `winError` (a number). No token or field ever contains text from a request, a response, a certificate or the OS.

| Group | Tokens |
|---|---|
| Request | `INVALID_JSON`, `INVALID_REQUEST`, `UNKNOWN_OPERATION`, `PAYLOAD_TOO_LARGE`, `BUSY` (more than 8 requests in flight), `INTERNAL` |
| Policy | `URL_NOT_ALLOWED` (not https / user info / numeric host / fragment ...), `HEADER_NOT_ALLOWED`, `NO_CREDENTIAL`, `CREDENTIAL_ORIGIN_MISMATCH`, `VAULT_ERROR` |
| Transport | `TIMEOUT`, `CANCELLED`, `DNS`, `CONNECT_FAILED` (WinHTTP does not say refused / unreachable apart), `CONNECTION_RESET`, `PROXY_ERROR`, `REDIRECT_BLOCKED`, `RESPONSE_TOO_LARGE`, `BAD_RESPONSE`, `TRANSPORT_ERROR` (+ `winError`) |
| TLS | `DEPTH_ZERO_SELF_SIGNED_CERT` (untrusted authority and the certificate is self-issued), `CERT_UNTRUSTED` (untrusted authority, for example a corporate TLS-inspection root), `CERT_HOSTNAME_MISMATCH`, `CERT_EXPIRED`, `CERT_REVOKED`, `CERT_REVOCATION_UNKNOWN`, `CERT_INVALID`, `TLS_FAILED` |

**Limits.** Request line 64 KiB, except `http.send` up to 16 MiB (the `pime-core` bulk limit); request body 8 MiB; response body 4 MiB (the cap the
JavaScript client already applied; the download is abandoned the moment it is exceeded); 8 requests in flight; 16 headers.

## Security properties

| # | Property | Enforced by | Checked by |
|---|---|---|---|
| 1 | No operation returns a key; `cred.exists` returns the origin only. | the protocol has no such operation (`engine.rs`) | `it_016_*` unit test, smoke "IT-016 round trip" |
| 2 | The key is only injected at send time into a credential header (`Authorization` / `x-api-key`), built by the worker. | `policy::AuthTemplate`, header allow-list | `ut_031_*`, smoke "success path" (the mock saw it) |
| 3 | A key is only ever sent to the origin it was stored for. | `CREDENTIAL_ORIGIN_MISMATCH` check before any I/O | unit test, smoke "origin it was not stored for" |
| 4 | A provider that echoes the key gets it removed from body and headers (`[redacted]`, counted in `redactions`). | `redact::scrub_exact` | unit test, smoke "echoes the key back" (positive control: the mock really received it) |
| 5 | The key appears in no reply line and no stderr line; the worker has no log. | design + `Secret` has no value in `Debug`; stderr only via `redact::diagnostic` | smoke final check over every line of every worker started |
| 6 | Product default = system trust store, no relaxation; test relaxations need the environment gate, apply to loopback only and are visible in `health`. Under `--insecure-test-ca` only the authority check is relaxed. | `config::parse_config`, `PreparedRequest::accept_untrusted_ca` | `config` unit tests; smoke: the bare product start rejects the mock's self-signed certificate (exact token `DEPTH_ZERO_SELF_SIGNED_CERT`, the mock saw zero requests), test mode without the flag does too, and with the flag a certificate served under a name it does not cover is still `CERT_HOSTNAME_MISMATCH` (validity dates: by design, not tested) |
| 7 | https only (plain http for loopback in test mode only); no user info, numeric hosts, fragments or backslashes in URLs. | `policy::parse_url` | unit tests, smoke policy check |
| 8 | Redirects are never followed (the credential header must not travel to another host); a 3xx is `REDIRECT_BLOCKED`. | `WINHTTP_OPTION_REDIRECT_POLICY_NEVER` + engine | smoke "redirects are never followed" (exactly one request reached the mock) |
| 9 | Timeout and cancel really abort the WinHTTP request (the request handle is closed). | `transport::Flight`, `winhttp.rs` | smoke "timeout" (deadline held), "cancel mid-flight" (CANCELLED within about 30 ms of a 30 s request); a throw-away stress run of 360 overlapping send/cancel/timeout cycles ended with the right tokens and a healthy worker (not part of the repository) |
| 10 | Response size cap, header allow-list (`set-cookie`, `www-authenticate`, `location` never leave). WinHTTP decompression is on (the request carries `Accept-Encoding: gzip`), so the cap applies to decoded bytes. | `engine::response_fields`, `winhttp.rs` | unit tests, smoke (a compressed response itself was not exercised) |
| 11 | Key material this crate owns is wiped after use (volatile writes). | `secret.rs`: `Secret`, `WideSecret`, buffers built with exact capacity | wipe unit tests (spare capacity included) |
| 12 | TLS 1.2/1.3 only, no cookies, no automatic Windows credentials. | session/request options in `winhttp.rs` | NOT separately verified (see below) |

**Scrubbing is an exact match.** `scrub_exact` removes the key as the worker holds it. A provider that echoes it in another form (a JSON string
that escapes `/` as `\/`, percent-encoding, a partial key such as `sk-...abcd`) is not caught; keys may contain `/`. The mitigation is that a key is
only ever sent to the one origin it was stored for.

**A panic ends the worker.** The release profile uses `panic = "abort"` (like `pime-tsf`), so the `catch_unwind` in the engine only fires in
debug builds; in the product a panic terminates the process silently (the panic hook prints nothing, a message could contain request text).
The caller must treat that as the worker's death, which the architecture already requires it to survive (typing stays unaffected).

**What wiping does not reach (stated, not hidden).** Copies the operating system, the stdio pipe buffers and WinHTTP make internally; the
intermediate buffers of the JSON parser while it grows the string of a `cred.set` line (the line buffer and the final string are wiped, a
reallocation may leave an earlier, shorter prefix in freed heap); a crash dump. Any process of the same Windows user can read a generic
credential with `CredReadW`, so Credential Manager protects against *the key being in the extension host, in settings or in a file*, not
against malware running as the user.

**The abort mechanism and its residual risk.** One request is one synchronous WinHTTP exchange on its own thread. A cancel or the deadline
timer closes the request handle from another thread (the documented way to stop a synchronous WinHTTP call); the blocked call fails with
`ERROR_WINHTTP_OPERATION_CANCELLED` and the engine reports `CANCELLED` or `TIMEOUT` from its own record of who aborted. Every WinHTTP call is
preceded by a check of that record, but a handle can still be closed between the check and the call, and WinHTTP could in theory reuse the freed
handle value for another request inside that window. The stress run above did not hit it; it cannot be excluded by construction. `WinHttpSetTimeouts`
is set as a backstop behind the deadline.

## What is verified, and what is not

Verified on this machine (Windows 11 Pro 10.0.26300, `x86_64-pc-windows-gnu`, Rust 1.99.1, Node 26.13.0):

- `cargo test` in the crate: **90** unit tests (protocol engine with a fake transport and an in-memory vault, URL/header/template policy, redaction,
  framing, error mapping, wipe primitives, the test-mode gate), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
- `node tests/smoke/ai-worker-smoke.js` (25 checks, run against both the debug and the release build; exit code 0): the real `pime-ai-worker.exe` over
  WinHTTP against the UAT mock provider on loopback HTTPS (and a Node HTTP server for the plain-http test option); the executable is x64 and
  imports only Windows system DLLs; real Credential Manager entries named `PIME-TEST-<random>/<provider>` with session persistence, listed by
  `cmdkey` while they exist, deleted afterwards, none left (checked by `cmdkey` and by a fresh worker).
- `cargo test --workspace` from `01_Source` is unaffected (the crate is excluded from the workspace).

**Not verified (and why).** The WinHTTP path was proven against the local mock only.

- Real providers (OpenAI, Anthropic, OpenAI-compatible servers): no network call was made. HTTP/2, large streaming responses, real certificate chains
  and SNI behaviour are unexercised.
- **Proxies**: `WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY` is used for every non-loopback host, but no proxy (static, PAC, WPAD, authenticated) was ever
  in the path. Automatic Windows credentials are **disabled** (`WINHTTP_DISABLE_AUTHENTICATION`), so an NTLM/Kerberos proxy that expects integrated
  logon will answer 407, which is passed through as a status; supporting it needs an explicit decision.
- **Corporate TLS inspection**: an inspecting proxy whose root is installed in the Windows store is trusted automatically; one whose root is not
  gives `CERT_UNTRUSTED`. Neither case was exercised. Certificate **revocation checking is not enabled** (WinHTTP default); turning it on
  (`WINHTTP_ENABLE_SSL_REVOCATION`) is a follow-up decision because it adds network dependencies.
- Windows versions other than this one: `AUTOMATIC_PROXY` needs 8.1 or later (a fallback to the default proxy configuration exists but was not
  run), the TLS 1.3 flag falls back to 1.2-only when WinHTTP rejects it (not run). Windows 10 and Server editions are untested.
- **Persistent** credentials: the tests use session persistence only (the one allowed side effect); `CRED_PERSIST_LOCAL_MACHINE`, the product
  setting, is written by the same code but was never exercised. IT-016 asks for an independent test user account; the smoke test uses the
  current user with a `PIME-TEST-` namespace instead.
- DNS failures end-to-end (`DNS` token): mapped and unit-tested, but not provoked (it would need a lookup outside loopback).
- The 32-bit and ARM64 builds, and the whole UAT / VS Code path: not run (Playwright and VS Code are off limits on this machine). A fresh
  `cargo build --release` can fail with `os error 5` while Defender scans a dependency's build script (the same transient seen elsewhere in this
  project); retrying succeeds.

## How it connects to the rest (transition path of plan 5.7)

1. **Today (P1, unchanged by this work).** JavaScript reads the key from VS Code SecretStorage (`extension.js:91`, `f1.js:210`, `annotate.js:24`,
   `extension.js:310`), `core/ai.js` puts it into the request headers and calls `fetch`. The key is in the extension host and crosses the pipe to
   the core in `ai.prepare`.
2. **Step A (worker in place, JavaScript relays).** Key entry (`extension.js:270`) calls `cred.set` once with the endpoint's `host` and does not
   write SecretStorage; the first run migrates an existing SecretStorage key (`cred.set`, then `secrets.delete`); key deletion (`extension.js:283`)
   calls `cred.delete`. `AiClient.infer` stops receiving `key`: the family adapters build the request *without* a credential header and add
   `credentialRef` + `authHeader`; the HTTP send becomes `http.send`, `AbortController` becomes `http.cancel`, and the worker's tokens map onto the
   existing codes (`TIMEOUT` -> `AI_TIMEOUT`, `CANCELLED` -> `AI_CANCELLED`, `NO_CREDENTIAL` -> `VAULT_ERROR`, the rest -> `AI_NETWORK` with the
   token as detail). The key is then in JavaScript only at the moment the user types it. For the UAT (HTTPS mock provider) start the worker with
   `PIME_WORKER_TEST_MODE=1` and `--insecure-test-ca` so that the mock's self-signed certificate is accepted without touching the system trust
   store, and add `--test-credential-namespace PIME-TEST-<run>` so the UAT never touches the real `ProgrammingIME/...` credentials.
3. **Step B (P3).** `pime-core` talks to the worker over the `PIME-IPC/1` named pipe (core `ai.prepare` returns `http{...}` plus `credentialRef`
   and `authHeader`); JavaScript is out of the AI path. **Step C (P4)**: key entry moves to a native dialog so JavaScript never sees a key.
4. Plan P2 exit conditions: the UAT-under-worker run, the disk/memory scan beyond the worker's own output, and "kill the worker, typing is
   unaffected" (IT-042 / ST-037) need the extension side of step A and are **not done** here.

## Dependencies (decision record, plan section 6)

| Dependency | Purpose | Alternatives considered | Cost | Build script | Licence |
|---|---|---|---|---|---|
| `windows =0.58.0` with the features `Win32_Foundation`, `Win32_Networking_WinHttp`, `Win32_Security_Credentials`, `Win32_Security_Cryptography` | WinHTTP, Credential Manager; the last feature (the `crypt32` function `CertFreeCertificateContext` and the certificate structs) is used only to tell a self-signed certificate (`DEPTH_ZERO_SELF_SIGNED_CERT`) from an untrusted root (`CERT_UNTRUSTED`). It goes beyond the two features decided for P2; dropping it only degrades that one distinction. | rustls (rejected in the P2 decision), `windows-sys`, hand-written `extern` blocks (more `unsafe` of our own) | release executable 388 KB (`opt-level = "s"`, LTO, stripped); imports only `winhttp`, `advapi32`, `crypt32`, `kernel32`, `ntdll`, `msvcrt`, `bcryptprimitives` and one `api-ms-win-core` set | `windows_x86_64_gnu` (links the prebuilt import libraries; it is the one that can hit Defender's `os error 5` on a fresh build) and `proc-macro2` | MIT OR Apache-2.0 |
| transitive: `windows-core`, `-result`, `-strings`, `-targets`, `-implement`, `-interface`, `proc-macro2`, `quote`, `syn`, `unicode-ident` | pulled in by `windows-core 0.58` (the proc macros run at build time only) | - | - | see above | MIT OR Apache-2.0 |
| `pime-core` (path dependency) | its `json` module: the parser/serializer with size and depth limits, so the worker and the core read and write exactly the same JSON | a second parser (a differential-parser risk) | none (`pime-core` has no dependencies and `forbid(unsafe_code)`) | none | MIT |

The pin `=0.58.0` is deliberate and has the same reason as for `pime-tsf` (see `docs/VERSIONS.md`): 0.60 and later import with raw-dylib, which needs a
working `dlltool` on this GNU host. Supply-chain tooling (`cargo deny` / `cargo vet`) is still the open decision of plan section 10, item 3.

## Choices beyond the brief (to confirm)

- **`cred.set` requires `host`** (origin binding, see above). It protects the key from a compromised caller at the cost of one more field and of
  re-entering the key when a provider's endpoint changes.
- The worker answers `CONNECT_FAILED`, not `REFUSED`: WinHTTP reports one error for refused, unreachable and filtered connections.
- **Every** redirect is refused (`REDIRECT_BLOCKED`), not only cross-host ones, matching `redirect: 'error'` in `core/ai.js`.
- "Loopback" means `127.0.0.0/8` and `localhost` (it decides which hosts the test options may relax and which skip the system proxy).
- Only `GET` and `POST`; unknown request fields are refused; the credential operations use the same `credentialRef` name as `http.send`.

## Developer commands

```text
cargo fmt --manifest-path crates/pime-ai-worker/Cargo.toml --check
cargo clippy --manifest-path crates/pime-ai-worker/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path crates/pime-ai-worker/Cargo.toml
cargo build --manifest-path crates/pime-ai-worker/Cargo.toml
node tests/smoke/ai-worker-smoke.js        # PIME_WORKER_EXE, or <CARGO_TARGET_DIR>/debug/pime-ai-worker.exe
```

The smoke test needs OpenSSL for the mock's certificate (as the UAT does), `cmdkey` (part of Windows) and Node 22.22+/24.15+/26.
