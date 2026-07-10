#!/usr/bin/env bash
# Prove non-gpu tests pass with empty HOME + empty YAPPER_VOICES_DIR (no Eve).
# Mirrors plan verification: PYTHONPATH=python pytest -q -m 'not gpu'
#
# Usage:
#   timeout 60s ./scripts/test_empty_home_pytest.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRATCH="${TMPDIR:-/tmp}/yapper-empty-home-pytest-$$"
mkdir -p "$SCRATCH/empty-home" "$SCRATCH/empty-voices"
cleanup() { rm -rf "$SCRATCH"; }
trap cleanup EXIT

# Prefer PATH pytest if it can import under empty HOME; else shipped entrypoint.
export HOME="$SCRATCH/empty-home"
export YAPPER_VOICES_DIR="$SCRATCH/empty-voices"
export PYTHONPATH=python

PYTEST_BIN=pytest
if ! env HOME="$HOME" PYTHONPATH=python pytest --version >/dev/null 2>&1; then
  PYTEST_BIN="$ROOT/scripts/pytest"
fi

cd "$ROOT"
timeout 60s env HOME="$HOME" YAPPER_VOICES_DIR="$YAPPER_VOICES_DIR" PYTHONPATH=python \
  "$PYTEST_BIN" -q -m 'not gpu' --tb=line
echo "empty-HOME non-gpu pytest: OK (via $PYTEST_BIN)"
