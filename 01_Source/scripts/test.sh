#!/usr/bin/env bash
# Runs the stored test scripts (no GUI). Everything lives in tests/: see tests/README.md. UAT: node tests/run-all.js --all
set -euo pipefail
cd "$(dirname "$0")/.."
node tests/run-all.js "$@"
