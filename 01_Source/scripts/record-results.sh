#!/usr/bin/env bash
# Explicitly rewrites the raw verification records 02_Verification/rust-test-result.txt and node-test-result.txt
# (the traceability report reads them). Regression runs (tests/run-all.js) never touch these files; run this on purpose.
# Needs cargo and node on PATH. CARGO_TARGET_DIR defaults to a temp folder outside the repository.
set -uo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${TMPDIR:-/tmp}/pime-cargo-target}"
OUT_DIR="../02_Verification"
RUST_OUT="$OUT_DIR/rust-test-result.txt"
NODE_OUT="$OUT_DIR/node-test-result.txt"
core_version=$(grep -m1 '^version' crates/pime-core/Cargo.toml | cut -d'"' -f2)
{
echo "# Rust core (pime-core $core_version) verification record"
echo "# date: $(date -Iseconds)"
echo "# host: $(rustc -vV | grep '^host'), $(rustc --version), $(cargo --version)"
echo "# Raw output of the commands below, in order. Build outputs went to a scratch CARGO_TARGET_DIR outside the repo. Edition 2024."
echo
echo "==================== 1. cargo test --workspace (debug / unoptimized test profile) ===================="
cargo test --workspace 2>&1
echo
echo "==================== 2. cargo test --test parity -- --nocapture --test-threads=1 (golden fixtures vs JS engine) ===================="
cargo test --test parity -- --nocapture --test-threads=1 2>&1
echo
echo "==================== 3. UT-068 benchmark, DEBUG build: cargo test --test benchmark -- --nocapture ===================="
cargo test --test benchmark -- --nocapture 2>&1
echo
echo "==================== 4. UT-068 benchmark, RELEASE build: cargo test --release --test benchmark -- --nocapture ===================="
cargo test --release --test benchmark -- --nocapture 2>&1
echo
echo "==================== 5. cargo clippy --workspace --all-targets -- -D warnings ===================="
cargo clippy --workspace --all-targets -- -D warnings 2>&1; echo "clippy exit code: $?"
echo
echo "==================== 6. cargo fmt --all -- --check ===================="
cargo fmt --all -- --check 2>&1; echo "fmt exit code: $?"
echo
echo "==================== 7. crates/pime-tsf (GNU target): cargo test, clippy -D warnings, fmt --check ===================="
( cd crates/pime-tsf && cargo test 2>&1; echo "tsf test exit code: $?"; cargo clippy --all-targets -- -D warnings 2>&1; echo "tsf clippy exit code: $?"; cargo fmt -- --check 2>&1; echo "tsf fmt exit code: $?" )
echo
echo "==================== 8. Differential fuzz JS vs Rust: NOT re-run in this record (the earlier script is not part of the repository); the stored differential tests are tests/js/ut-engine.test.js ===================="
} > "$RUST_OUT"

core_exe="$CARGO_TARGET_DIR/debug/pime-core.exe"
[ -f "$core_exe" ] || core_exe="$CARGO_TARGET_DIR/debug/pime-core"
{
echo "# JavaScript tests (tests/js, node:test) - raw output, $(date -Iseconds), node $(node -v)"
echo "# command: PIME_CORE_EXE=<debug pime-core> node --test --test-reporter=tap *.test.js   (the engine differential tests run because the core exe is set); then node scripts/check-syntax.js"
echo
( cd tests/js && PIME_CORE_EXE="$core_exe" node --test --test-reporter=tap *.test.js 2>&1 )
echo
echo "# ---- syntax check"
( cd editor-bridges/vscode && node scripts/check-syntax.js 2>&1 )
} > "$NODE_OUT"
grep -E "^test result|exit code" "$RUST_OUT" | cut -c1-110
grep -E "^# (tests|pass|fail|skipped)" "$NODE_OUT"
