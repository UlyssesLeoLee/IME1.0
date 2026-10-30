# P0 results (docs/RUST_MIGRATION_PLAN.md, phase P0) — 2026-10-30

| P0 item | Result |
|---|---|
| Freeze v0.2 behaviour | Done: `tests/golden-v0.2/` (5 fixture files + UAT baseline + `SHA256SUMS.txt`); `node scripts/glue-check.js --golden` reports 239/239 frozen cases unchanged. |
| `glue-check` | Done: `scripts/glue-check.js` (`--enforce p1|p6`, `--golden`). Baseline measured **3,084 hand-written lines in 22 files** under `editor-bridges/vscode/src` (the plan's "about 2,750" was an under-count; the plan's targets are unchanged: P1 <= 700, P6 <= 150). `--enforce p1` fails today, as it should. |
| WASM feasibility (B level) | **Loading works.** `tests/experiments/wasm-panel-probe`: a dependency-free 2.2 KB Rust wasm module is fetched from the extension's `media/` folder and instantiated inside a real VS Code 1.137.0 webview with the planned CSP (`script-src 'nonce-…' 'wasm-unsafe-eval'; connect-src <cspSource>`). Data crosses both ways (view-model JSON in, result string out). Fetch + instantiate took about 26 ms (two runs: 26.1 / 26.3 ms; the panel opened about 4.3-5.5 s after VS Code was launched, which is mostly VS Code start-up). Negative control without `'wasm-unsafe-eval'`: instantiate is rejected by CSP, as expected. |
| wasm-bindgen toolchain | **Partly verified.** `rustup target add wasm32-unknown-unknown` works (user level). The `wasm-bindgen` 0.2.130 *library* compiled for wasm32 on this host. Installing the **`wasm-bindgen-cli` failed**: `windows-sys` needs `dlltool.exe` and this host uses the `x86_64-pc-windows-gnu` toolchain without mingw binutils. This is a toolchain gap of this machine (an MSVC host, or mingw-w64 on PATH, would not have it), not a CSP/design problem. The generated JS glue and the DOM bindings of wasm-bindgen were therefore **not** exercised; only the WASM load path and CSP were. |
| Correction of an earlier statement | Earlier runs on this host failed to execute a dependency's build script (`os error 5`). In the wasm-bindgen build above, proc-macro2/syn/quote and `wasm-bindgen` built fine with a different target directory. The cause of the earlier failure is therefore **not established**; do not rely on "build scripts cannot run here" as a design constraint (the plan already says so). What *is* established: `windows-sys`/`windows` need an MSVC toolchain or mingw `dlltool` on this kind of host (matters for P2-P5). |

## Consequences for the plan

- B level stays feasible: the only remaining unknowns are wasm-bindgen's generated glue and DOM binding under the same CSP, to be checked on a machine with
  an MSVC (or mingw) toolchain when P6 starts. The fallback to A level is unchanged.
- P2-P5 should be built with the MSVC toolchain (`x86_64-pc-windows-msvc`); install Visual Studio Build Tools or build on the VM/CI.

Reproduce the probe: `rustc --target wasm32-unknown-unknown --crate-type cdylib -O -C panic=abort rust/panel.rs -o ext/media/panel.wasm`
inside `tests/experiments/wasm-panel-probe`, then `node run-probe.js` (needs VS Code and `npm install` in `tests/uat/`).
