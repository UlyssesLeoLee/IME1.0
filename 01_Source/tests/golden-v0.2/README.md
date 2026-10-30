# golden-v0.2 — frozen behaviour of release v0.2 (P0 of docs/RUST_MIGRATION_PLAN.md)

Read-only reference copies, taken at commit `cfca543` (before the logic moved into Rust):

- `predict-cases.json`, `validate-cases.json`, `x86-vectors.json`, `resolve-cases.json`, `secret-cases.json`:
  outputs of the JavaScript engine for the shared fixture inputs. The live fixtures in `../fixtures/` may be
  extended on purpose; these copies must never be edited.
- `uat-results-baseline.json`: the Playwright result of the 24 UAT scenarios (all passed) with the JavaScript engine.
- `SHA256SUMS.txt`: integrity of the files above (`sha256sum -c SHA256SUMS.txt`).

Use: when the JavaScript engine is deleted (plan step 1.5) the Rust core must still reproduce every case here, unless a
behaviour change was decided and documented. The Rust `tests/parity.rs` checks the live fixtures; `scripts/glue-check.js
--golden` checks that the live fixtures still contain every frozen case unchanged.
