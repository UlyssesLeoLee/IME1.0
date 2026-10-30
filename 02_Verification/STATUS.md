# 2026-10-30 Verification record (v0.2)

Environment: Windows 11, Node v22.12.0, VS Code 1.137.0 (stable, `Code.exe` driven over CDP), Playwright 1.64.1,
Rust 1.99.1 (`x86_64-pc-windows-gnu`, user-level install; no MSVC), `cargo` 1.99.1.

| Check | Result | Evidence |
|---|---|---|
| JavaScript unit/integration tests (`npm test` in `editor-bridges/vscode`) | **PASS 81 / 81** | `node-test-result.txt` (raw), `test-execution-report.md` |
| JavaScript syntax check (`npm run check`) | PASS 30 / 30 files | same |
| Rust core `cargo test --workspace` (zero dependencies) | **PASS 101 / 101** (lib 90, parity 5, protocol 5, benchmark 1) | `rust-test-result.txt` (raw output of the Rust run); re-run independently by the main session: 90+0+1+5+5 passed |
| Rust `clippy -D warnings`, `fmt --check` | clean (reported by the Rust run; not re-run by the main session) | `rust-test-result.txt` |
| JS vs Rust golden fixtures | 100 predict, 51 validate, 59 x86, 18 resolve, 11 secret cases identical | `crates/pime-core/tests/parity.rs`, `editor-bridges/vscode/test/parity.test.js` |
| Real `pime-core.exe` over JSON Lines (health/predict/encode/scan) | PASS (manual smoke, same first candidate as the JS engine, no secret value echoed) | not a stored artifact |
| Playwright UAT on the real VS Code with the extension loaded from source | **PASS 24 / 24** (3 spec files) | `uat/uat-results.json`, `uat/screenshots/` |
| Local candidate latency (UT-068) | JS P95 about 1 ms, Rust debug P95 0.27 ms / release 0.036 ms (limit 20 ms) | printed by the tests; diagnostic only, **not** the formal ST-039 measurement |
| Windows TSF build, registration, activation in zh-CN/en-US/ja-JP | **NOT RUN** (no MSVC/VM) | - |
| Signed installer, repair, uninstall, restart | **NOT RUN** | - |
| Named-pipe IPC `PIME-IPC/1`, Windows Credential Manager worker | **NOT IMPLEMENTED** | `docs/IMPLEMENTATION_MATRIX.md` |
| Real AI provider APIs | **NOT RUN** (mock provider only; no real key used) | - |
| DPI 100-250 %, multi-monitor, screen reader, 30-minute offline soak, CPU idle | **NOT RUN** | - |
| Self-review | One pass by the author of the code (see below). No independent reviewer or `/code-review` run | - |

## Design-case coverage (by test-title id)

`test-execution-report.md` lists every UT/IT/ST id. Summary: UT 76 of 77 have passing automated evidence,
IT 38 of 48, ST 28 of 43; none failed. "Evidence" means a passing test carries the id; it is a mapping, not a
formal sign-off of every step in the design case. The remaining ids are NOT RUN with the reason stated per row.
The planned count in the design documents (168) is the number of designed cases, not of executed ones.

## What the UAT contains (24 scenarios, real VS Code)

local candidates and language rules (8), key lifecycle / consent / F1 panel / Diff / hunks / Undo / stale protection /
injection / secrets / key binding (10), routing and effort / budget and 429 / untrusted workspace / UI languages /
permission revoke / realtime AI (6). The mock provider counts requests, so "zero requests" statements are measured.

## Limits of this evidence

- UAT drives VS Code only. Nothing here shows that a Windows input method works in Notepad or any other application.
- The mock provider certificate is self-signed; the test process disables verification (`NODE_TLS_REJECT_UNAUTHORIZED=0`).
- The extension host and VS Code version were not varied; Playwright interacts with DOM selectors that can change between VS Code releases.

## Reproduce

```powershell
cd 01_Source\editor-bridges\vscode ; npm test ; npm run check
cd 01_Source ; cargo test --workspace            # needs Rust; set CARGO_TARGET_DIR outside the repo if the project folder blocks executables
cd 01_Source\uat ; npm install ; npx playwright test     # needs VS Code and OpenSSL (Git for Windows)
cd 01_Source ; node scripts/traceability-report.js       # regenerates test-execution-report.{md,json}
```

Passing tests are NOT an assertion that the product meets the 64 requirements or the planned test cases.

## Self-review (author, one pass) — findings that changed the code

Found by the tests above and fixed: a TypeScript/PowerShell statement-position rule that was too strict (no candidates after a line
without a semicolon; JS and Rust updated together and covered by new fixtures), a missing "Regenerate" button in the error state of the F1
panel, apply/request timers that could let the event loop exit (now ref'd and cleared), a static-scan regex that mistook `RegExp.exec`
for process execution, and a diagnostics gap (transport errors such as `DEPTH_ZERO_SELF_SIGNED_CERT` are now recorded as a safe token).

Known open points (not fixed): the read-only Diff preview text stays in memory (max 20 entries) until the extension is deactivated;
Insert Apply cannot re-verify the cursor when the document is not visible in any editor; line-level hunks only; the Ctrl+Alt+Space
keybinding is not covered by UAT; no independent code review (`/code-review`) was run.
