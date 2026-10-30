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
  smoke/                   core-binary-smoke.js: the real pime-core.exe over JSON Lines
  uat/                     Playwright user-acceptance tests on the real VS Code (+ HTTPS mock provider, helpers)
  experiments/             feasibility probes that are scripts, e.g. wasm-panel-probe (WASM inside a VS Code webview)
```

Unit tests that need private access stay inside the Rust modules (`#[cfg(test)]` in `crates/pime-core/src`); cargo runs them with the rest.

## Running

```powershell
node tests/run-all.js                    # fast set, no GUI: syntax, fixtures, golden, Rust, JS (+ engine differential tests), smoke
node tests/run-all.js --all              # + UAT with the JavaScript engine and UAT with the Rust engine (about 8 minutes)
node tests/run-all.js --js               # single steps: --syntax --fixtures --golden --js --rust --smoke --uat --uat-rust
node tests/run-all.js --uat --only-uat "UAT-23"
node tests/run-all.js --report           # explicit: rewrite 02_Verification/test-execution-report.* from the last results
```

- Rust build output goes to `CARGO_TARGET_DIR` or `<temp>/pime-cargo-target`, never into the repository. The Rust UAT run writes its
  evidence to a temp folder, so the official evidence in `02_Verification/uat/` (JavaScript engine) is not overwritten.
- The UAT mock provider reuses one throw-away HTTPS certificate (OS temp folder, 20 hours) instead of generating one per run.
- `PIME_CORE_EXE` (path of a built `pime-core.exe`) enables the engine differential tests in `tests/js/ut-engine.test.js`; without it they are
  reported as skipped, never as passed.
- Requirements: Node 22.22+ / 24.15+ / 26 (tested with 26.13.0), Rust stable, VS Code and OpenSSL for the UAT (see `tests/uat/README.md`).

## Adding a test

Put it in the matching folder above and give it the id of the design case (UT/IT/ST/AT) or the added requirement (ANN-xx, API-xx, ENG-xx) in its
title; `scripts/traceability-report.js` maps results to ids by title. Do not keep ad-hoc scripts outside this folder.
