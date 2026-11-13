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
  tsf/ime_*.rs             the end-to-end binaries of the text service: ime_registration.rs (IT-002, the other input methods stay untouched), ime_context_security.rs (IT-005, a secure field collects nothing), ime_dpi_language.rs (IT-037, the geometry at 200 % and the language independence), ime_resilience.rs (IT-042, a core that does not answer) - the two that only partly prove their case carry NO case number on purpose, so the traceability report does not count them as covered
  smoke/                   core-binary-smoke.js: the real pime-core.exe over JSON Lines; tsf-dll-check.js: builds crates/pime-tsf and inspects the DLL; ai-worker-smoke.js; ipc-smoke.js; ime-ui-capture.ps1 + ime-ui-check.js: the native windows of the IME (open them, take a picture, check that it is not a flat rectangle)
  ipc/, setup/              integration tests of crates/pime-ipc and crates/pime-setup (wired through [[test]] entries in their Cargo.toml)
  uat/                     Playwright user-acceptance tests on the real VS Code (+ HTTPS mock provider, helpers)
  uat/tests/08-*.spec.js   the acceptance scenarios of the third round: the candidate overlay of the local rules (IT-006), the syntax context per language (ST-008), the drop-down of the seven languages (ST-031), and the safety ones (IT-043, ST-009, ST-026, ST-029)
  e2e/                     the runner of the end-to-end cases that need a REAL Windows 11 guest: Invoke-E2E.ps1 (the entry point, -PlanOnly lists what it would do), lib/ (the transport, the guest helper and the cases), guest/ (the probes that run inside the guest), cases/ (ST-001..005 and ST-035). It needs a guest it can reach over WinRM or ssh - see its README; the machine that runs the runner is not the machine that runs the tests
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
node scripts/gate-log.js                  # regenerate the evidence log 02_Verification/run-all-2026-11-04.log: it runs the gate and writes the log WHILE the gate runs, so the claims checks inside the gate read this round (D-134); it cleans the pime-core / pime-tsf artefacts first, because the red-green step leaves the broken binary in the cache (D-133)
powershell tests/e2e/Invoke-E2E.ps1 -PlanOnly   # what the Windows 11 guest runner would do, without a guest
powershell tests/e2e/Invoke-E2E.ps1 -Config tests/e2e/vmconfig.json   # the end-to-end cases ST-001..005 and ST-035 (needs a guest)
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
- A **new Rust test target** may be blocked **once** by the security scanner of the machine: a freshly linked test binary is quarantined
  ("could not execute process ... (never executed)", and the file is gone afterwards), and the gate's cargo retry only knows `os error 5`,
  so it reports the step as failed instead of retrying. Run the new target once by hand (`cargo test --test <name>`) before the gate, or on a
  machine without that scanner. See D-138.
- After a gate run that includes the **red-green (mutation) step**, clean the build cache before the next run: the restore puts the backup's
  mtime back on the source, the artefact built from the broken source is the newer one, so cargo does not rebuild it and the next run tests the
  **broken** binaries (the JS engine differential test then fails on the grant broker and never exits). `cargo clean -p pime-core` in `01_Source`
  and `cargo clean -p pime-tsf` in `01_Source/crates/pime-tsf` (that crate has its own workspace). See D-133.

## Adding a test

Put it in the matching folder above and give it the id of the design case (UT/IT/ST/AT) or the added requirement (ANN-xx, API-xx, ENG-xx) in its
title; `scripts/traceability-report.js` maps results to ids by title. Do not keep ad-hoc scripts outside this folder.

**Only put a case number in the title when the test proves the WHOLE criterion of that case.** A test that proves half of it must carry no
number: the report counts a numbered test as "covered", so a half-proven number makes the report say something the test does not show. Two of
the new text-service binaries (`ime_dpi_language.rs`, `ime_resilience.rs`) are written that way on purpose.

**A test that passes is not yet a test that holds.** Break the implementation and see whether the test notices - that is the only way to
know. `scripts/mutation-check.js` does it for the whole implementation; for a single new test, change the code, run that test, and put the
file back (write the content back and push its mtime to now - `fs.copyFileSync` keeps the old mtime and cargo would then keep using the
broken binary, see D-133).
