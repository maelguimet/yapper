#!/usr/bin/env bash
# Generate a redistributable default neutral reference for Chatterbox (Piper ljspeech).
# Produces: $DEST/default_neutral.wav
set -euo pipefail

VOICE_ID="${YAPPER_VOICE_ID:-default}"
DEST="${YAPPER_VOICES_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/yapper/voices}"
MODELS="${YAPPER_MODELS_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/yapper/models}/piper"
PIPER_VOICE="${YAPPER_PIPER_VOICE:-en_US-ljspeech-medium}"

mkdir -p "$DEST" "$MODELS"

if [[ -f "$DEST/${VOICE_ID}_neutral.wav" ]]; then
  echo "neutral ref already present: $DEST/${VOICE_ID}_neutral.wav"
  exit 0
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "ERROR: python3 required to run Piper TTS" >&2
  exit 1
fi

# Piper TTS Python API (piper-tts on PyPI).
python3 - <<'PY' "$MODELS" "$PIPER_VOICE" "$DEST" "$VOICE_ID"
import sys
from pathlib import Path

models_dir = Path(sys.argv[1])
voice_id = sys.argv[2]
dest = Path(sys.argv[3])
prefix = sys.argv[4]

try:
    from piper import PiperVoice
except ImportError as e:
    raise SystemExit(
        "piper-tts not installed in this Python. Install with: pip install piper-tts"
    ) from e

onnx = models_dir / f"{voice_id}.onnx"
json_path = models_dir / f"{voice_id}.onnx.json"
if not onnx.is_file() or not json_path.is_file():
    raise SystemExit(
        f"Piper model missing under {models_dir}. "
        f"Download {voice_id}.onnx and {voice_id}.onnx.json from rhasspy/piper-voices "
        f"(see assets/voices/ATTRIBUTION.md)."
    )

out = dest / f"{prefix}_neutral.wav"
voice = PiperVoice.load(str(onnx), config_path=str(json_path))
with out.open("wb") as f:
    voice.synthesize(
        "This is the default reference voice for Yapper.",
        f,
    )
print(f"wrote {out}")
PY