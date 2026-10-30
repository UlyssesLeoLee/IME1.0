#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if command -v cargo >/dev/null 2>&1; then cargo test --workspace; else echo 'SKIP: cargo unavailable (not a pass)'; fi
(cd editor-bridges/vscode && npm test && npm run check)
