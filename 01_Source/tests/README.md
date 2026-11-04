# tests/ — every stored test script lives here

Regression runs only **run** these scripts. Nothing is regenerated unless you ask for it explicitly.

```
tests/
  run-all.js               single entry point (see below)
  gen-fixtures.js          golden-data generator: COMPARES by default; `--write` regenerates (a deliberate behaviour change)
  fixtures/                golden data shared by the JavaScript engine and the Rust core (+ core-jsonl-*.txt protocol samples)
  golden-v0.2/             frozen behaviour of release v0.2 - read-only (SHA256SUMS.txt)
  js/                      node:test suites for the extension's pure logic and engine adapter (ut-*.test.js, parity.test.js, helpers.js)
  rust/                    integration tests of crates/pime-core (wired through [[test]] entries in its Cargo.toml)
  tsf/                     the tests of the input method itself (crates/pime-tsf): common/ = the harness (a fake host, a scripted predictor, the transcript writer), common/scenarios.rs = the 21 behaviour scenarios + the 23-function ledger, ime_scenarios.rs, ime_functions.rs, ime_host.rs (the host-level COM test)
  smoke/                   core-binary-smoke.js: the real pime-core.exe over JSON Lines; tsf-dll-check.js: builds crates/pime-tsf and inspects the DLL; ai-worker-smoke.js; ipc-smoke.js; ime-ui-capture.ps1 + ime-ui-check.js: the native windows of the IME (open them, take a picture, check that it is not a flat rectangle)
  ipc/, setup/              integration tests of crates/pime-ipc and crates/pime-setup (wired through [[test]] entries in their Cargo.toml)
  uat/                     Playwright user-acceptance tests on the real VS Code (+ HTTPS mock provider, helpers)
  experiments/             feasibility probes that are scripts, e.g. wasm-panel-probe (WASM inside a VS Code webview)
```

Unit tests that need private access stay inside the Rust modules (`#[cfg(test)]` in `crates/pime-core/src`); cargo runs them with the rest.

## Running

```powershell
node tests/run-all.js                    # fast set, no GUI: syntax, fixtures, golden, Rust, JS (+ engine differential tests), smoke, TSF DLL build + PE check
node tests/run-all.js --all              # + UAT with the JavaScript engine and UAT with the Rust engine (about 8 minutes)
node tests/run-all.js --js               # single steps: --syntax --fixtures --golden --js --rust --smoke --tsf --uat --uat-rust
node tests/run-all.js --tsf              # crates/pime-tsf unit tests, then build the DLL and inspect it (in the fast set)
node tests/run-all.js --tsf-live         # the live host test: the real msctf, a real document context (OPTS IN, it opens the candidate window)
node tests/run-all.js --tsf-ui           # the native windows: the probe opens the candidate window / the floating bar / the settings window, a picture of the desktop is taken and checked (OPTS IN, it takes over the screen for a few seconds)
node tests/run-all.js --worker           # crates/pime-ai-worker: unit tests + smoke test against the HTTPS mock (needs OpenSSL; PIME-TEST session credentials, removed afterwards)
node tests/run-all.js --ipc              # crates/pime-ipc: unit + loopback tests on real named pipes (unique test pipe names)
node tests/run-all.js --setup            # crates/pime-setup: unit + temp-folder integration tests (never touches Program Files, HKLM or regsvr32)
node tests/run-all.js --uat --only-uat "UAT-23"
node tests/run-all.js --report           # explicit: rewrite 02_Verification/test-execution-report.* from the last results
```

- Rust build output goes to `CARGO_TARGET_DIR` or `<temp>/pime-cargo-target`, never into the repository. The Rust UAT run writes its
  evidence to a temp folder, so the official evidence in `02_Verification/uat/` (JavaScript engine) is not overwritten.
- The UAT mock provider reuses one throw-away HTTPS certificate (OS temp folder, 20 hours) instead of generating one per run.
- `PIME_CORE_EXE` (path of a built `pime-core.exe`) enables the engine differential tests in `tests/js/ut-engine.test.js`; without it they are
  reported as skipped, never as passed.
- `PIME_IME_LIVE=1` makes `tests/tsf/ime_host.rs` drive the real `msctf` instead of the fake host (it opens the candidate window).
  `PIME_IME_EVIDENCE_DIR=<dir>` makes every IME scenario write its transcript as a JSON file there; the regression chain does not set it, so a
  normal run writes nothing.
- The function ledger of the input method (`tests/tsf/common/scenarios.rs`, `IME_FUNCTIONS`) names, for each of the 23 functions, the test that
  covers it; `tests/tsf/ime_functions.rs` looks the named tests up in the source, so a renamed or deleted test turns the ledger red instead of
  letting it rot silently. See `01_Source/docs/IME_TEST_MATRIX.md`.
- Requirements: Node 22.22+ / 24.15+ / 26 (tested with 26.13.0), Rust stable, VS Code and OpenSSL for the UAT (see `tests/uat/README.md`).

## Adding a test

Put it in the matching folder above and give it the id of the design case (UT/IT/ST/AT) or the added requirement (ANN-xx, API-xx, ENG-xx) in its
title; `scripts/traceability-report.js` maps results to ids by title. Do not keep ad-hoc scripts outside this folder.
