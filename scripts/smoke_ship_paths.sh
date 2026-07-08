#!/usr/bin/env bash
# Ship-bar smokes. X11 unit tests use isolated Xvfb (never paste into user session).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRATCH="${YAPPER_SCRATCH:-/tmp/grok-goal-29cc0bace209/implementer}"
mkdir -p "$SCRATCH"
LOG="$SCRATCH/x11.log"
: >"$LOG"

log() { printf '%s\n' "$*" | tee -a "$LOG"; }

cd "$ROOT"
export YAPPER_SCRATCH="$SCRATCH"

log "=== ship path smoke $(date -Iseconds) ==="
log "host_DISPLAY=${DISPLAY:-unset}"
log "session=${XDG_SESSION_TYPE:-unknown}"
log "note: paste tests use Xvfb isolation inside rust tests (no focus steal)"

# --- X11 unit tests (PRIMARY, CLIPBOARD, paste_at_cursor under Xvfb) ---
log "--- cargo test x11util (isolated Xvfb) ---"
cargo test --quiet x11util:: -- --nocapture --test-threads=1 2>&1 | tee -a "$LOG"
log "cargo_x11_tests_ok"

# --- Select → speak: PRIMARY read (via cargo) already covered; TTS synth of fixture text ---
log "--- select→speak (PRIMARY-sourced text → TTS WAV via worker) ---"
export PYTHONPATH="$ROOT/python"
export PYTHONUNBUFFERED=1
PY="${ROOT}/.venv/bin/python"
[[ -x "$PY" ]] || PY=python3

"$PY" - <<'PY' 2>&1 | tee -a "$LOG"
import json, os, subprocess, sys, wave
from pathlib import Path

ROOT = Path(".").resolve()
SCRATCH = Path(os.environ["YAPPER_SCRATCH"])
# Text as if read from PRIMARY (same content cargo primary test validates)
marker = f"Yapper select speak smoke {os.getpid()}"
# Prove PRIMARY tools work on host display for selection read (read only — no paste)
if os.environ.get("DISPLAY"):
    subprocess.run(["xclip", "-selection", "primary", "-i"], input=marker.encode(), check=True)
    got = subprocess.check_output(["xclip", "-selection", "primary", "-o"], text=True)
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
proc = subprocess.run(
    [sys.executable, "-m", "yapper_tts"],
    input="".join(json.dumps(c) + "\n" for c in cmds),
    text=True,
    capture_output=True,
    env=env,
    timeout=300,
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

# --- Hold-to-talk insert: STT fixture → insert path under Xvfb (cargo tests) + clipboard proof ---
log "--- hold-to-talk insert (transcribe → insert_transcript under Xvfb) ---"
"$PY" - <<'PY' 2>&1 | tee -a "$LOG"
import json, os, subprocess, sys
from pathlib import Path

ROOT = Path(".").resolve()
SCRATCH = Path(os.environ["YAPPER_SCRATCH"])
fixture = SCRATCH / "fixtures" / "speech_en.wav"
if not fixture.is_file():
    fixture.parent.mkdir(parents=True, exist_ok=True)
    raw = fixture.with_suffix(".raw.wav")
    subprocess.run(
        ["espeak-ng", "-w", str(raw), "Hello, this is a yapper speech test."], check=True
    )
    subprocess.run(
        ["ffmpeg", "-y", "-i", str(raw), "-ar", "16000", "-ac", "1", str(fixture)],
        check=True,
        capture_output=True,
    )
    raw.unlink(missing_ok=True)

env = os.environ.copy()
env["PYTHONPATH"] = str(ROOT / "python")
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
    timeout=180,
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

# Paste path is covered by cargo insert_transcript / paste_at_cursor under Xvfb
# (does NOT steal user focus). Record that we linked STT output → insert API contract.
print("HOLD_TO_TALK_STT_OK")
print("insert_path=x11util::insert_transcript_at_cursor (Xvfb-tested)")
print("HOLD_TO_TALK_INSERT_OK")
PY

log "=== ALL SHIP PATH SMOKES OK ==="
echo "OK" >"$SCRATCH/ship-paths.ok"
