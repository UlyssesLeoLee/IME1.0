# 2026-10-30 Verification record (code v0.2.0, after the tests/ restructure and the upgrades)

Environment: Windows 11, Node v26.13.0 (installed side by side; the machine default is still 22.12.0), npm 12.2.1, VS Code 1.144.0
(stable, `Code.exe` driven over CDP), Playwright 1.64.1, Rust 1.99.1 (`x86_64-pc-windows-gnu`, user-level install; no MSVC), edition 2024.

| Check | Result | Evidence |
|---|---|---|
| JavaScript unit/integration tests (`tests/js`, node:test, with the real core exe so the engine differential tests run) | **PASS 118 / 118** (0 skipped) | `node-test-result.txt` (raw) |
| JavaScript syntax check (extension sources + all test scripts) | PASS 49 / 49 files | same |
| Golden fixtures (compare only) and frozen `golden-v0.2` cases | PASS, unchanged | `node tests/run-all.js` |
| Rust core `cargo test --workspace` (zero dependencies) | **PASS 359 / 359** (lib 242 + integration suites 117: parity, protocol 0.3 incl. the AI / privacy / annotate / symbols / scheduler / settings / diagnostics / i18n / UI ops, benchmark) | `rust-test-result.txt` (raw) |
| Rust `clippy -D warnings`, `fmt --check` | clean (exit code 0 both) | `rust-test-result.txt` |
| System-IME key map in the input state machine (page-relative digits, PageUp/PageDown, Shift+Tab, Space in COMPOSING, optional punctuation paging, Shift-tap pause; Direct/Observing consume nothing) | **PASS** tests `key_01..key_13` (15 test functions, part of the 359 Rust tests); pure logic only, no keyboard hook | `rust-test-result.txt` |
| `crates/pime-tsf` (pure-Rust TSF DLL, `windows` 0.58 on the GNU target): `cargo test` **156 / 156** (TIP controller and key map 63, candidate window layout 28, registration plan 29, UI-element model/driver/glue 34, base 2), clippy, fmt, release build, PE inspection | **PASS** for build and inspection: x64 PE, exactly `DllCanUnloadNow/DllGetClassObject/DllRegisterServer/DllUnregisterServer`, system DLL imports only. **The DLL is NOT registered, NOT loaded by Windows; the COM glue was only compiled, never run against TSF** (milestones M4-M5 not done) | `tests/smoke/tsf-dll-check.js`, `rust-test-result.txt` section 7 |
| Real `pime-core.exe` over JSON Lines (health/predict/encode/scan) | PASS (`tests/smoke/core-binary-smoke.js`, stored script) | `node tests/run-all.js` |
| Playwright UAT on the real VS Code 1.144.0, **JavaScript engine** | **PASS 31 passed, 1 skipped, 0 failed** (4.7 min). The skipped one is ENG-02, which needs the Rust engine | `uat/uat-results.json`, `uat/screenshots/` |
| Playwright UAT, **Rust engine** (`programmingIme.engine = rust`) | **NOT COMPLETE.** Two runs were stopped by the harness (low memory), the latest after 18 scenarios, all passing; no complete Rust-engine run exists since the Rust AI/annotate/scheduler/settings/i18n/UI ops were wired in behind the engine switch | - |
| Local candidate latency (UT-068) | JS P95 about 1 ms; Rust debug P95 0.254 ms, release P95 0.039 ms (limit 20 ms) | `rust-test-result.txt`; diagnostic only, **not** the formal ST-039 measurement |
| Windows TSF registration, activation in zh-CN/en-US/ja-JP, typing in real applications | **NOT RUN** (needs a disposable Windows 11 VM with admin rights: runbook `01_Source/docs/TSF_VM_TEST_PLAN.md`); registration code exists but only ran against an in-memory fake; the UI-element protocol (provider side) is written and registered as `TIPCAP_UIELEMENTENABLED`, tested against a fake host only; `TIPCAP_IMMERSIVESUPPORT` is deliberately NOT registered (see `docs/RUST_MIGRATION_PLAN.md` 13.11), so Store apps / Start search will not offer this IME | - |
| Installer `crates/pime-setup` (Rust, unsigned) + `.vsix` | **BUILT, dry-run only**: 50 unit tests; x64 installer 2,978,899 B (sha256 `a3dcd330013497c6fa12db8ac634fd1518f420d3446221ed0141fe1df2b9c79d`), `--dry-run` changed nothing (checked); the temp-folder integration tests were blocked by endpoint protection on this machine. **NOT RUN:** real install, registration, UAC elevation, uninstall, reboot-delete, signing, VM install. Not released | `01_Source/docs/INSTALLER.md` |
| Named-pipe IPC `PIME-IPC/1` (`crates/pime-ipc`) | **PASS** 91 tests (61 unit + 30 loopback on real named pipes, repeated 3 times by the author, once by the integrator). **NOT RUN:** different-SID refusal (IT-003), remote/cross-session clients, full IT-004/ST-037, AppContainer, 32-bit; no per-frame MAC after the handshake (see IPC.md) | `01_Source/docs/IPC.md` |
| Rust AI worker `crates/pime-ai-worker` (WinHTTP + Credential Manager) | **PASS** 90 unit tests + 25-check smoke test against the local HTTPS mock (key injected at send time and never in any output, self-signed certificate rejected by default, cancel/timeout/429/500, no test credential left behind). Not wired into the extension | `01_Source/docs/AI_WORKER.md` |
| Real AI provider APIs | **NOT RUN** (mock provider only; no real key used) | - |
| DPI 100-250 %, multi-monitor, screen reader, 30-minute offline soak, CPU idle | **NOT RUN** | - |
| Self-review | One pass by the author of the code (see below). No independent reviewer or `/code-review` run | - |

## Design-case coverage (by test-title id)

`test-execution-report.md` lists every UT/IT/ST id. Summary: UT 76 of 77 have passing automated evidence, IT 38 of 48, ST 28 of 43; none
failed. 15 tests for requirements added after the design baseline (ANN annotations, API provider compatibility) pass. "Evidence" means a passing
test carries the id; it is a mapping, not a formal sign-off of every step in the design case. The remaining ids are NOT RUN with the reason
stated per row. The planned count in the design documents (168) is the number of designed cases, not of executed ones. A skipped test is never
counted as evidence.

## What the UAT contains (32 scenarios, real VS Code)

local candidates and language rules (8), key lifecycle / consent / F1 panel / Diff / hunks / Undo / stale protection / injection / secrets /
key binding (10), routing and effort / budget and 429 / untrusted workspace / UI languages / permission revoke / realtime AI (6), annotations
(comments and Cypher graph comments, consent, secret gate) (6), engine switch (2). The mock provider counts requests, so "zero requests"
statements are measured.

## Limits of this evidence

- UAT drives VS Code only. Nothing here shows that a Windows input method works in Notepad or any other application: **there is no
  system-wide input method has been shown to work** (`crates/pime-tsf` was never registered and its COM glue never ran under TSF; see `01_Source/docs/TSF_VM_TEST_PLAN.md`).
- The mock provider certificate is self-signed; the test process disables verification (`NODE_TLS_REJECT_UNAUTHORIZED=0`).
- Playwright interacts with DOM selectors that can change between VS Code releases (verified on 1.137.0 and 1.144.0 only).

## Reproduce

```powershell
cd 01_Source
node tests/run-all.js            # fast set, no GUI: syntax, fixtures, golden, Rust, JS (+ engine differential), real core.exe smoke
node tests/run-all.js --all      # + UAT on the real VS Code with the JavaScript engine and with the Rust engine
node tests/run-all.js --report   # explicit: regenerate test-execution-report.{md,json} from the last results
```
All test scripts live in `01_Source/tests/` (see `tests/README.md`); regression runs never regenerate fixtures, certificates or reports.

Passing tests are NOT an assertion that the product meets the 64 requirements or the planned test cases.

## Self-review (author, one pass) — findings that changed the code

Found by the tests above and fixed: a TypeScript/PowerShell statement-position rule that was too strict (no candidates after a line
without a semicolon; JS and Rust updated together and covered by new fixtures), a missing "Regenerate" button in the error state of the F1
panel, apply/request timers that could let the event loop exit (now ref'd and cleared), a static-scan regex that mistook `RegExp.exec`
for process execution, a diagnostics gap (transport errors such as `DEPTH_ZERO_SELF_SIGNED_CERT` are now recorded as a safe token), and the
traceability report counting a Playwright-skipped spec as passed (now excluded).

Known open points (not fixed): the read-only Diff preview text stays in memory (max 20 entries) until the extension is deactivated;
Insert Apply cannot re-verify the cursor when the document is not visible in any editor; line-level hunks only; the Ctrl+Alt+Space
keybinding is not covered by UAT; no independent code review (`/code-review`) was run.
