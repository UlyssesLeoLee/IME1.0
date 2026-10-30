# 2026-10-30 Verification record (code v0.2.0, after the tests/ restructure and the upgrades)

Environment: Windows 11, Node v26.13.0 (installed side by side; the machine default is still 22.12.0), npm 12.2.1, VS Code 1.144.0
(stable, `Code.exe` driven over CDP), Playwright 1.64.1, Rust 1.99.1 (`x86_64-pc-windows-gnu`, user-level install; no MSVC), edition 2024.

| Check | Result | Evidence |
|---|---|---|
| JavaScript unit/integration tests (`tests/js`, node:test, with the real core exe so the 10 engine differential tests run) | **PASS 100 / 100** | `node-test-result.txt` (raw) |
| JavaScript syntax check (extension sources + all test scripts) | PASS 49 / 49 files | same |
| Golden fixtures (compare only) and frozen `golden-v0.2` cases | PASS, unchanged | `node tests/run-all.js` |
| Rust core `cargo test --workspace` (zero dependencies) | **PASS 220 / 220** (lib 173, benchmark 1, parity 5, parity_edit 7, protocol 5, protocol_v03 11, protocol_v03_edit 18) | `rust-test-result.txt` (raw) |
| Rust `clippy -D warnings`, `fmt --check` | clean (exit code 0 both) | `rust-test-result.txt` |
| System-IME key map in the input state machine (page-relative digits, PageUp/PageDown, Shift+Tab, Space in COMPOSING, optional punctuation paging, Shift-tap pause; Direct/Observing consume nothing) | **PASS** 11 new tests `key_01..key_11` (part of the 220); pure logic only, no keyboard hook | `rust-test-result.txt` |
| `crates/pime-tsf` (pure-Rust TSF DLL, `windows` 0.58 on the GNU target): `cargo test` 2/2, clippy, fmt, release build, PE inspection | **PASS** for build and inspection: x64 PE, exactly `DllCanUnloadNow/DllGetClassObject/DllRegisterServer/DllUnregisterServer`, only system DLL imports, 240 KB. **The DLL is NOT registered, NOT loaded by Windows and handles no key yet** (milestones M3-M5 not done) | `tests/smoke/tsf-dll-check.js`, `rust-test-result.txt` section 7 |
| Real `pime-core.exe` over JSON Lines (health/predict/encode/scan) | PASS (`tests/smoke/core-binary-smoke.js`, stored script) | `node tests/run-all.js` |
| Playwright UAT on the real VS Code 1.144.0, **JavaScript engine** | **PASS 31 passed, 1 skipped, 0 failed** (4.7 min). The skipped one is ENG-02, which needs the Rust engine | `uat/uat-results.json`, `uat/screenshots/` |
| Playwright UAT, **Rust engine** (`programmingIme.engine = rust`) | **NOT COMPLETE in the final run.** The run was stopped by the harness (low memory) after 14 of 32 scenarios, all 14 passing. An earlier run on this VS Code had 25 passed and UAT-23 failed once (it passed in 2 of 3 single reruns); the UAT helper was hardened afterwards (`openQuick` retry, `cmd()` waits for the palette) but a complete Rust-engine run has not been repeated since | - |
| Local candidate latency (UT-068) | JS P95 about 1 ms; Rust debug P95 0.254 ms, release P95 0.039 ms (limit 20 ms) | `rust-test-result.txt`; diagnostic only, **not** the formal ST-039 measurement |
| Windows TSF build, registration, activation in zh-CN/en-US/ja-JP | **NOT RUN** (no MSVC/VM); the TSF code is a 204-line pass-through skeleton with no candidate window | - |
| Signed installer, repair, uninstall, restart | **NOT RUN** | - |
| Named-pipe IPC `PIME-IPC/1`, Windows Credential Manager worker | **NOT IMPLEMENTED** | `01_Source/docs/IMPLEMENTATION_MATRIX.md` |
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
  system-wide input method in this delivery** (see `01_Source/windows/tsf/README.md`).
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
