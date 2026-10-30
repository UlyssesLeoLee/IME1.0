# Technology versions and currency check (2026-10-30)

Method: latest versions from the official registries (npm, crates.io, nodejs.org, update.code.visualstudio.com, static.rust-lang.org) and API
facts from the vendors' documentation read through Context7 (OpenAI API reference, VS Code API, Playwright, Rust Edition Guide, wasm-bindgen)
and the Anthropic API version pages. "Latest" is only as good as those sources on that day.

| Technology | Used by | Version in the project / on the dev machine | Latest found | Status |
|---|---|---|---|---|
| Playwright (`@playwright/test`) | UAT | 1.64.1 (pinned) | 1.64.1 | current |
| Rust toolchain | core, probes | 1.99.1 stable | 1.99.1 | current |
| Rust edition | `pime-core` | **2024** (was 2021) | 2024 | upgraded; 209 tests, clippy `-D warnings` and fmt clean |
| wasm-bindgen | P0 probe only | 0.2.130 | 0.2.130 | current (CLI could not be installed on this machine: gnu toolchain without `dlltool`) |
| OpenSSL (Git for Windows) | UAT test certificate | 3.5.7 | - | current build |
| Node.js | tests, UAT, tools (not the extension) | 22.12.0 is still the machine default; **26.13.0 installed side by side (nvm) and used for every verification run** | LTS 26.13.0 | upgraded for the project; the machine default was deliberately not switched (`nvm use 26.13.0` switches it) |
| npm | tests, UAT | **12.2.1** (under Node 26) | 12.2.1 | upgraded (needs Node 22.22.2 / 24.15 / 26+) |
| VS Code | UAT host, extension target | **1.144.0** installed (was 1.137.0; installer sha256 and Microsoft signature verified); `engines.vscode` stays `^1.94.0` | 1.144.0 | upgraded; the UAT was re-run on 1.144.0 (see `02_Verification/STATUS.md`) |
| Anthropic API version header | `anthropic-messages` adapter | **2026-10-21** default (was 2023-06-01); per-provider `apiVersion` override | 2026-10-21 | upgraded; request shape sends none of the removed parameters (temperature/top_p/top_k) |
| OpenAI Responses API | `openai-responses` adapter | `instructions`, `input`, `max_output_tokens`, `reasoning.effort`; reads `output_text`/`output[].content[].text` and `usage.input_tokens/output_tokens` | OpenAI recommends Responses for new projects | confirmed against the API reference |
| OpenAI Chat Completions | `openai-chat` adapter (compatible servers) | `max_tokens` by default; per-provider `maxTokensParam` | `max_tokens` is deprecated and **not compatible with o-series models**; `max_completion_tokens` replaces it | fixed by the option (default kept for third-party servers) |
| OpenAI reasoning effort values | effort negotiation | `auto, none, minimal, low, medium, high, xhigh, max, extended` | `none, minimal, low, medium, high, xhigh, max`; "not all models support every value" | updated; a level is sent only when the capability record says the model supports it |
| VS Code API used | extension | `InlineCompletionItem.filterText`, `QuickPickItemKind`, `asWebviewUri`, `WorkspaceEdit`, SecretStorage | documented and current in the API reference | no deprecation found in the docs; behaviour on 1.144 is checked by the UAT |
| Playwright API used | UAT | `chromium.connectOverCDP` (documented; Electron `launch` is not used because the release VS Code build refuses the inspector) | documented | current |
| `windows` / `windows-core` crates (TSF DLL `crates/pime-tsf`) | system input method | **0.58.0, pinned on purpose** | 0.62.2 (`windows`), 0.100.0 (`windows-core`) | **deliberately NOT latest**: 0.60, 0.61 and 0.62 import Windows functions with raw-dylib, which needs a working binutils `dlltool` + `as` on `x86_64-pc-windows-gnu`; this machine's Rust-bundled `dlltool.exe` has no assembler and the build fails (tried 0.60, 0.61, 0.62). 0.58 ships prebuilt import libraries and builds with no MSVC. Upgrade once MSVC or MinGW-w64 binutils is installed |
| CMake / C++ standard (legacy C++ TSF skeleton) | `windows/tsf` | CMake >= 3.21, C++17 | newer exist | **not checked**: never built; superseded by the Rust `crates/pime-tsf` (see above) |
| JSON Schema dialect | `schemas/` | draft 2020-12 | 2020-12 | current |

## Edition 2024 migration (done)

Following the Rust Edition Guide: `cargo fix --edition`, `edition = "2024"`, build and test, `cargo fmt`. Result: one automatic fix (the test
helper named `gen`, now a reserved keyword, renamed `id_gen`); `cargo fmt` re-ordered imports; clippy's `collapsible_if` now suggests let-chains in
edition 2024, applied with `cargo clippy --fix` (about a dozen places). The crate has no dependencies and forbids `unsafe`, so the unsafe-related
2024 changes did not apply. 209 tests pass before and after.

## Open points

- Real-provider behaviour (OpenAI, Anthropic with `2026-10-21`, OpenAI-compatible servers) is unverified: only a mock provider was used.
- wasm-bindgen's generated glue and DOM binding under the planned CSP are unverified (see `docs/P0_RESULTS.md`).
- Re-run this check whenever a phase of `docs/RUST_MIGRATION_PLAN.md` starts.
