#!/usr/bin/env bash
# Unit tests for install.sh honesty: hard/optional dep classification + YAPPER_MODELS.
# Sources the shipped install.sh (does not reimplement parse/check logic).
#
# Usage:
#   timeout 30s ./scripts/test_install_truth.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../install.sh
source "$ROOT/install.sh"

PASS=0
FAIL=0

ok() {
  PASS=$((PASS + 1))
  printf '  PASS %s\n' "$*"
}

bad() {
  FAIL=$((FAIL + 1))
  printf '  FAIL %s\n' "$*" >&2
}

assert_eq() {
  local label="$1" got="$2" want="$3"
  if [[ "$got" == "$want" ]]; then
    ok "$label"
  else
    bad "$label (got=${got@Q} want=${want@Q})"
  fi
}

assert_contains() {
  local label="$1" hay="$2" needle="$3"
  if [[ "$hay" == *"$needle"* ]]; then
    ok "$label"
  else
    bad "$label (missing ${needle@Q} in output)"
  fi
}

# Restore real have_cmd between mock tests.
_real_have_cmd() {
  command -v "$1" >/dev/null 2>&1
}

restore_have_cmd() {
  have_cmd() { _real_have_cmd "$1"; }
}

# --- parse_yapper_models (shipped function) ---------------------------------

echo "== parse_yapper_models =="

unset YAPPER_MODELS || true
assert_eq "default is small" "$(parse_yapper_models)" "small"

YAPPER_MODELS=small
assert_eq "explicit small" "$(parse_yapper_models)" "small"

YAPPER_MODELS=small,medium
assert_eq "comma small,medium" "$(parse_yapper_models)" "small medium"

YAPPER_MODELS="small medium"
assert_eq "space small medium" "$(parse_yapper_models)" "small medium"

YAPPER_MODELS=medium,small
assert_eq "canonical order small before medium" "$(parse_yapper_models)" "small medium"

YAPPER_MODELS=medium
assert_eq "medium only" "$(parse_yapper_models)" "medium"

YAPPER_MODELS=tiny
set +e
out="$(parse_yapper_models 2>&1)"
rc=$?
set -e
if [[ "$rc" -eq 0 ]]; then
  bad "invalid size should die, got: $out"
else
  assert_contains "invalid size names token" "$out" "tiny"
  assert_contains "invalid size names allowed" "$out" "small"
fi

YAPPER_MODELS=""
set +e
out="$(parse_yapper_models 2>&1)"
rc=$?
set -e
if [[ "$rc" -eq 0 ]]; then
  bad "empty YAPPER_MODELS should die"
else
  assert_contains "empty models message" "$out" "empty"
fi

unset YAPPER_MODELS || true

# --- HARD_DEPS / OPTIONAL_DEPS table vs README-required tools ---------------

echo "== dep tables cover README hard tools =="
hard_tools=""
for entry in "${HARD_DEPS[@]}"; do
  hard_tools+=" ${entry%%|*}"
done
for tool in rustc cargo python3 ffmpeg arecord xclip xdotool; do
  if [[ " $hard_tools " == *" $tool "* ]]; then
    ok "HARD_DEPS includes $tool"
  else
    bad "HARD_DEPS missing $tool"
  fi
done

opt_tools=""
for entry in "${OPTIONAL_DEPS[@]}"; do
  opt_tools+=" ${entry%%|*}"
done
for tool in mpv ffplay pactl; do
  if [[ " $opt_tools " == *" $tool "* ]]; then
    ok "OPTIONAL_DEPS includes $tool"
  else
    bad "OPTIONAL_DEPS missing $tool"
  fi
done

# Every hard/optional entry must carry a non-empty impact string.
for entry in "${HARD_DEPS[@]}" "${OPTIONAL_DEPS[@]}"; do
  tool="${entry%%|*}"
  impact="${entry#*|}"
  if [[ -n "$tool" && -n "$impact" && "$tool" != "$impact" ]]; then
    ok "impact text for $tool"
  else
    bad "missing impact for entry ${entry@Q}"
  fi
done

# --- check_hard_deps / check_optional_deps with mocked have_cmd -------------

echo "== missing hard tool dies with impact =="
have_cmd() {
  [[ "$1" == "arecord" ]] && return 1
  _real_have_cmd "$1"
}
set +e
out="$(check_hard_deps 2>&1)"
rc=$?
set -e
restore_have_cmd
if [[ "$rc" -eq 0 ]]; then
  bad "check_hard_deps should die when arecord missing"
else
  assert_contains "names arecord" "$out" "arecord"
  assert_contains "names mic impact" "$out" "mic capture"
  assert_contains "dies with hard tools summary" "$out" "missing hard tools"
fi

echo "== missing optional tool warns with impact (no die) =="
have_cmd() {
  [[ "$1" == "mpv" ]] && return 1
  _real_have_cmd "$1"
}
set +e
out="$(check_optional_deps 2>&1)"
rc=$?
set -e
restore_have_cmd
if [[ "$rc" -ne 0 ]]; then
  bad "check_optional_deps should not die for missing mpv (rc=$rc)"
else
  assert_contains "warns missing mpv" "$out" "mpv"
  assert_contains "warns playlist/fallback impact" "$out" "pause"
fi

echo "== missing ffplay warns with impact =="
have_cmd() {
  [[ "$1" == "ffplay" ]] && return 1
  _real_have_cmd "$1"
}
set +e
out="$(check_optional_deps 2>&1)"
rc=$?
set -e
restore_have_cmd
assert_contains "names ffplay" "$out" "ffplay"
assert_contains "ffplay last-resort impact" "$out" "last-resort"
if [[ "$rc" -ne 0 ]]; then
  bad "ffplay-missing optional check should exit 0"
else
  ok "ffplay-missing optional check exits 0"
fi

# --- structural: install.sh is source-safe ----------------------------------

echo "== source-safe install.sh =="
if declare -f parse_yapper_models >/dev/null && declare -f check_deps >/dev/null; then
  ok "sourced functions available without running main"
else
  bad "expected install.sh functions after source"
fi

# --- summary ----------------------------------------------------------------

echo
echo "install truth tests: $PASS passed, $FAIL failed"
if [[ "$FAIL" -ne 0 ]]; then
  exit 1
fi
exit 0
