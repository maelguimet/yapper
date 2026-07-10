#!/usr/bin/env bash
# Install Chatterbox reference WAVs into the voices root.
# Open install: Piper-generated default_neutral.wav (see generate_default_reference.sh).
# Dev-only: optional YAPPER_TTS_CLONE proprietary tree (not for redistribution).
set -euo pipefail

DEST="${YAPPER_VOICES_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/yapper/voices}"
VOICE_ID="${YAPPER_VOICE_ID:-default}"
CLONE="${YAPPER_TTS_CLONE:-}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

mkdir -p "$DEST"

neutral="$DEST/${VOICE_ID}_neutral.wav"

if [[ -f "$neutral" || -L "$neutral" ]]; then
  echo "neutral ref ok: $neutral"
  exit 0
fi

# Legacy local dev tree (proprietary — do not publish these files).
if [[ -n "$CLONE" && -d "$CLONE/gold" ]]; then
  echo "WARN: installing from YAPPER_TTS_CLONE — for local dev only, not redistributable" >&2
  shopt -s nullglob
  for f in "$CLONE"/gold/eve_*.wav; do
    base=$(basename "$f")
    if [[ ! -e "$DEST/$base" ]]; then
      ln -s "$f" "$DEST/$base" 2>/dev/null || cp -a "$f" "$DEST/$base"
    fi
  done
  if [[ -f "$CLONE/knobs.json" && ! -e "$DEST/knobs.json" ]]; then
    ln -s "$CLONE/knobs.json" "$DEST/knobs.json" 2>/dev/null || cp -a "$CLONE/knobs.json" "$DEST/knobs.json"
  fi
  if [[ -f "$DEST/eve_neutral.wav" ]]; then
    echo "legacy eve_neutral installed (Speak enabled for existing configs)"
    exit 0
  fi
fi

if bash "$ROOT/scripts/generate_default_reference.sh"; then
  exit 0
fi

echo "ERROR: could not install default neutral reference in $DEST" >&2
echo "  Install piper-tts, download en_US-ljspeech-medium ONNX files, then re-run." >&2
echo "  See assets/voices/ATTRIBUTION.md" >&2
exit 1