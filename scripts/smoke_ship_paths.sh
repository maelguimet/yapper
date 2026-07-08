#!/usr/bin/env bash
# Ship-bar smokes. X11 unit tests use isolated Xvfb (never paste into user session).
# Every step is hard-bounded (see AGENTS.md — Agent / test discipline).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRATCH="${YAPPER_SCRATCH:-/tmp/grok-goal-29cc0bace209/implementer}"
mkdir -p "$SCRATCH"
LOG="$SCRATCH/x11.log"
: >"$LOG"

# Whole script safety net (GPU TTS+STT + cargo)
SCRIPT_DEADLINE="${YAPPER_SMOKE_DEADLINE:-600}"

log() { printf '%s\n' "$*" | tee -a "$LOG"; }

# run_to SECS DESC -- CMD...
# Always logs; fails the script on timeout (124) or non-zero.
run_to() {
  local secs="$1" desc="$2"
  shift 2
  if [[ "${1:-}" == "--" ]]; then shift; fi
  log ">>> [$desc] timeout ${secs}s: $*"
  set +e
  timeout --foreground --signal=TERM --kill-after=10s "${secs}s" "$@" >>"$LOG" 2>&1
  local rc=$?
  set -e
  if [[ $rc -eq 124 ]]; then
    log "TIMEOUT after ${secs}s: $desc"
    return 124
  elif [[ $rc -ne 0 ]]; then
    log "FAIL rc=$rc: $desc (see $LOG)"
    return "$rc"
  fi
  log "ok: $desc"
}

cd "$ROOT"
export YAPPER_SCRATCH="$SCRATCH"
export PYTHONPATH="$ROOT/python"
export PYTHONUNBUFFERED=1
PY="${ROOT}/.venv/bin/python"
[[ -x "$PY" ]] || PY=python3

# Re-exec under outer timeout if not already wrapped
if [[ -z "${YAPPER_SMOKE_WRAPPED:-}" ]]; then
  export YAPPER_SMOKE_WRAPPED=1
  exec timeout --foreground --signal=TERM --kill-after=15s "${SCRIPT_DEADLINE}s" "$0" "$@"
fi

log "=== ship path smoke $(date -Iseconds) ==="
log "host_DISPLAY=${DISPLAY:-unset}"
log "session=${XDG_SESSION_TYPE:-unknown}"
log "deadline=${SCRIPT_DEADLINE}s"
log "note: paste tests use Xvfb isolation inside rust tests (no focus steal)"

# --- X11 unit tests (PRIMARY, CLIPBOARD, paste_at_cursor under Xvfb) ---
log "--- cargo test x11util (isolated Xvfb) ---"
run_to 30 "cargo x11util" -- \
  cargo test --quiet x11util:: -- --nocapture --test-threads=1
log "cargo_x11_tests_ok"

# --- Select → speak ---
log "--- select→speak (PRIMARY → TTS WAV) ---"
run_to 300 "select_speak_tts" -- "$PY" - <<'PY'
import json, os, subprocess, sys, wave
from pathlib import Path

ROOT = Path(".").resolve()
SCRATCH = Path(os.environ["YAPPER_SCRATCH"])
marker = f"Yapper select speak smoke {os.getpid()}"
if os.environ.get("DISPLAY"):
    subprocess.run(
        ["xclip", "-selection", "primary", "-i"],
        input=marker.encode(),
        check=True,
        timeout=5,
    )
    got = subprocess.check_output(
        ["xclip", "-selection", "primary", "-o"], text=True, timeout=5
    )
    assert got == marker, (got, marker)
    print(f"host_primary_read_ok={got!r}")
else:
    got = marker
    print("no host DISPLAY; using fixture text for TTS only")

out_wav = SCRATCH / "select_speak.wav"
out_wav.unlink(missing_ok=True)
cmds = [
    {"id": "1", "cmd": "load", "params": {"model": "chatterbox-multilingual", "device": "cuda"}},
    {
        "id": "2",
        "cmd": "synthesize",
        "params": {
            "text": got,
            "language": "en",
            "tone": "neutral",
            "voice": "eve",
            "out_path": str(out_wav),
        },
    },
    {"id": "3", "cmd": "unload"},
    {"id": "4", "cmd": "shutdown"},
]
env = os.environ.copy()
env["PYTHONPATH"] = str(ROOT / "python")
env["PYTHONUNBUFFERED"] = "1"
proc = subprocess.run(
    [sys.executable, "-m", "yapper_tts"],
    input="".join(json.dumps(c) + "\n" for c in cmds),
    text=True,
    capture_output=True,
    env=env,
    timeout=280,
)
assert proc.stdout.strip(), proc.stderr[-2000:]
ok = False
for ln in proc.stdout.splitlines():
    if not ln.startswith("{"):
        continue
    r = json.loads(ln)
    print("tts_resp", r)
    if r["id"] == "2":
        assert r["ok"], r
        ok = True
assert ok and out_wav.is_file() and out_wav.stat().st_size > 1000
with wave.open(str(out_wav), "rb") as wf:
    frames = wf.getnframes()
    assert frames > 0
print(f"select_speak_wav_ok size={out_wav.stat().st_size} frames={frames}")
print("SELECT_SPEAK_OK")
PY

# --- Hold-to-talk STT ---
log "--- hold-to-talk insert (transcribe fixture) ---"
run_to 180 "hold_to_talk_stt" -- "$PY" - <<'PY'
import json, os, subprocess, sys
from pathlib import Path

ROOT = Path(".").resolve()
SCRATCH = Path(os.environ["YAPPER_SCRATCH"])
fixture = SCRATCH / "fixtures" / "speech_en.wav"
if not fixture.is_file():
    fixture.parent.mkdir(parents=True, exist_ok=True)
    raw = fixture.with_suffix(".raw.wav")
    subprocess.run(
        ["espeak-ng", "-w", str(raw), "Hello, this is a yapper speech test."],
        check=True,
        timeout=30,
    )
    subprocess.run(
        ["ffmpeg", "-y", "-i", str(raw), "-ar", "16000", "-ac", "1", str(fixture)],
        check=True,
        capture_output=True,
        timeout=30,
    )
    raw.unlink(missing_ok=True)

env = os.environ.copy()
env["PYTHONPATH"] = str(ROOT / "python")
env["PYTHONUNBUFFERED"] = "1"
cmds = [
    {"id": "1", "cmd": "load", "params": {"model": "small", "device": "cuda"}},
    {"id": "2", "cmd": "transcribe", "params": {"path": str(fixture), "language": "en"}},
    {"id": "3", "cmd": "unload"},
    {"id": "4", "cmd": "shutdown"},
]
proc = subprocess.run(
    [sys.executable, "-m", "yapper_stt"],
    input="".join(json.dumps(c) + "\n" for c in cmds),
    text=True,
    capture_output=True,
    env=env,
    timeout=160,
)
text = None
for ln in proc.stdout.splitlines():
    if not ln.startswith("{"):
        continue
    r = json.loads(ln)
    print("stt_resp", r)
    if r["id"] == "2":
        assert r["ok"], r
        text = r["result"]["text"].strip()
assert text, "empty transcript"
print(f"transcript={text!r}")
(SCRATCH / "hold-to-talk-transcript.txt").write_text(text + "\n", encoding="utf-8")
print("HOLD_TO_TALK_STT_OK")
print("insert_path=x11util::insert_transcript_at_cursor (Xvfb-tested)")
print("HOLD_TO_TALK_INSERT_OK")
PY

log "=== ALL SHIP PATH SMOKES OK ==="
echo "OK" >"$SCRATCH/ship-paths.ok"
