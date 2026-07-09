#!/usr/bin/env bash
# Self-contained install smoke: non-editable pip install into an isolated venv,
# no PYTHONPATH into the live source tree, then STT/TTS ping via that interpreter.
# Does not mutate the user's live ~/.local install.
#
# Usage:
#   timeout 180s env YAPPER_SCRATCH=... ./scripts/smoke_self_contained_install.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRATCH="${YAPPER_SCRATCH:-/tmp/grok-goal-b9d89bbdbaff/implementer}"
PREFIX="$SCRATCH/self-contained-prefix"
VENV="$PREFIX/venv"
LOG="$SCRATCH/self-contained-install-smoke.log"

mkdir -p "$SCRATCH" "$PREFIX"
exec > >(tee "$LOG") 2>&1

log() { printf '==> %s\n' "$*"; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

assert_no_checkout_runtime_path() {
  local label="$1"
  local value="$2"
  case "$value" in
    "$ROOT"|"$ROOT"/*)
      die "$label points at checkout (not self-contained): $value"
      ;;
  esac
}

log "self-contained install smoke"
log "ROOT (checkout, used only as pip source at install time)=$ROOT"
log "isolated PREFIX=$PREFIX"
log "log=$LOG"

log "1) create isolated venv"
rm -rf "$VENV"
python3 -m venv --system-site-packages "$VENV"
# shellcheck disable=SC1091
source "$VENV/bin/activate"
pip install -U pip setuptools wheel

log "2) non-editable install of workers (no [dev], no -e)"
log "pip cmdline: pip install $ROOT/python"
# Prove the install line is non-editable / non-dev (same as install.sh user path).
printf 'pip install %s\n' "$ROOT/python" | grep -qE '(^| )-e |\[dev\]' \
  && die "pip install must not use -e or [dev]"
pip install "$ROOT/python"

PY="$VENV/bin/python"
[[ -x "$PY" ]] || die "missing $PY"

log "3) prove packages import from venv without checkout on PYTHONPATH"
unset PYTHONPATH || true
assert_no_checkout_runtime_path "python_bin" "$PY"
assert_no_checkout_runtime_path "PYTHONPATH" "${PYTHONPATH:-}"

"$PY" - "$ROOT" <<'PY'
import sys
from pathlib import Path

import yapper_common
import yapper_stt
import yapper_tts

root = Path(sys.argv[1]).resolve()
repo_python = root / "python"
for name, mod in [
    ("yapper_stt", yapper_stt),
    ("yapper_tts", yapper_tts),
    ("yapper_common", yapper_common),
]:
    p = Path(getattr(mod, "__file__", "") or "").resolve()
    print(f"  {name}: {p}")
    if not str(p):
        raise SystemExit(f"{name} has no __file__")
    try:
        p.relative_to(repo_python)
        raise SystemExit(f"{name} still loaded from checkout tree: {p}")
    except ValueError:
        pass  # not under checkout python/ — good
print("imports ok from installed package (not live checkout tree)")
PY

ping_worker() {
  local role="$1"
  local module="$2"
  log "4) ping $role ($module) with empty PYTHONPATH"
  local out
  out=$(
    printf '%s\n' '{"id":"1","cmd":"ping","proto":1}' \
      | env -u PYTHONPATH "$PY" -m "$module"
  )
  printf '%s\n' "$out"
  echo "$out" | grep -qE '"ok"[[:space:]]*:[[:space:]]*true' \
    || die "$role ping failed: $out"
  log "$role ping: ok"
}

ping_worker stt yapper_stt
ping_worker tts yapper_tts

log "5) path summary (runtime paths must not require checkout)"
echo "  python_bin=$PY"
echo "  PYTHONPATH=(unset)"
echo "  checkout_ROOT=$ROOT (install-time source only)"
assert_no_checkout_runtime_path "python_bin" "$PY"

deactivate || true
log "PASS self-contained install smoke"
log "full log: $LOG"
