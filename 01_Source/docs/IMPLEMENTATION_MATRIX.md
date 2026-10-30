# Implementation matrix — Design baseline vs. code v0.1

**Important:** 52 functional and 12 non-functional requirements are DESIGN-traced
in the supplied spreadsheets. That does NOT mean they are implemented or test-passed.

| DD module | Code delivered | Actual v0.1 status | Gap before production |
|---|---|---|---|
| DD-01 TSF | `windows/tsf` COM prototype | Source only, Windows build unverified, keys pass through | Full TSF composition, system activation tests, candidate UI, signing, installer |
| DD-02 Core | `crates/pime-core` | Rust deterministic candidates with local JSONL | Full composition state, FFI/IPC, rule cache, performance |
| DD-03 Languages | Rust core and JS fallback | Seven simple snippet sets, file-name detection | Syntax AST parsing, language versions, accurate machine-code assembler |
| DD-04 Context | VS Code extension | Active `TextDocument.getText`, selection, version | Multi-editor bridge, explicit scope grants, 2MiB IPC framing |
| DD-05 BYOK AI | VS Code extension | OpenAI Responses + compatible Chat Completions; SecretStorage; model/effort | Separate worker/Vault, vendor capability discovery, quota/cost, exhaustive providers |
| DD-06 Realtime | VS Code inline completion | Local suggestions and optional AI (350ms debounce) | System-level candidate UI; consistent cross-app preview; concurrency metrics |
| DD-07 F1 | VS Code command | Dual result, full Diff, user confirmation, stale document and cursor checks, apply edit | Prepare/Apply token transaction, per-hunk Diff, readback and Undo E2E |
| DD-08 UI | VS Code QuickPick/status | Language dropdown, simple candidate picker | Native floating TSF UI, settings panel, local accessibility testing |
| DD-09 Rules | Static arrays | Built-in templates | Declarative rule packs, marketplace, custom language plug-ins |
| DD-10 Ops | Scripts and docs | Build/test commands, no signed installer | Binary signing, MSI/WiX, real install/uninstall, secure migration |
| DD-11 Contracts | JSONL schema | Local stdio, no privileged IPC | Secure named-pipe authentication, ACL, handshake, message IDs |
| DD-12 Testkit | Rust unit tests, node:test, fixtures | 14 Rust tests authored (not runnable in packaging container); 12 JS tests PASS | IT/ST execution on Windows 11 and VS Code, automation/evidence |

## Security and compatibility design changes in this PoC

1. No global keyboard interception in Windows. The TSF DLL observes but consumes nothing.
2. AI requires a user-entered API key in VS Code SecretStorage, a trusted workspace,
   a separately enabled realtime permission per workspace/endpoint, or an explicit F1 approval.
3. AI output is always preview-only until deliberate user confirmation. Never executes code.
4. F1 editing operates on current editor buffer, not arbitrary disk files or paths.
5. Both version and SHA-256 text hash are checked before applying. Insert also checks selection.
6. Machine-code input has only five exact x86-64 examples. It is NOT an assembler/disassembler.
7. Direct local candidate engine continues functioning with no model/key/network.
8. TSF code is not signed, built or tested on Windows, so no claim of deployment readiness.

## Need-to-verify gates

- G0: Build TSF on Windows 11 x64, sign test DLL, register and remove 3 profiles, type in Notepad.
- G1: Verify editor dirty buffer / Diff / undo / stale version / concurrent edits in VS Code.
- G2: Verify authorized model requests, key isolation, cancellation and budget controls.
- G3: Verify seven language grammars, NFR performance, Windows Terminal and additional IDEs.
