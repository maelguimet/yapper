#!/usr/bin/env bash
# Copy/symlink Eve tone refs + knobs into the voices root.
# Destination: $YAPPER_VOICES_DIR, else XDG ~/.local/share/yapper/voices
# (same env the Rust shell injects from config.toml [models] voices_dir).
set -euo pipefail

DEST="${YAPPER_VOICES_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/yapper/voices}"
CLONE="${YAPPER_TTS_CLONE:-$HOME/projects/tts/clone}"

mkdir -p "$DEST"

if [[ -d "$CLONE/gold" ]]; then
  echo "installing Eve gold tones from $CLONE/gold → $DEST"
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
  count=$(find "$DEST" -name 'eve_*.wav' | wc -l)
  echo "voices ready: $count refs in $DEST"
else
  echo "WARN: clone gold not found at $CLONE/gold — cannot install Eve refs" >&2
  echo "  Set YAPPER_TTS_CLONE to a tree with gold/eve_*.wav, or copy refs into:" >&2
  echo "  $DEST" >&2
  echo "  Speak stays disabled until $DEST/eve_neutral.wav exists." >&2
  exit 1
fi

# Honesty gate: neutral is required for Speak (UI disables without it).
if [[ ! -f "$DEST/eve_neutral.wav" && ! -L "$DEST/eve_neutral.wav" ]]; then
  echo "ERROR: eve_neutral.wav missing in $DEST after install" >&2
  echo "  Speak will stay disabled until that file exists." >&2
  echo "  Expected source: $CLONE/gold/eve_neutral.wav" >&2
  exit 1
fi

echo "eve_neutral: ok ($DEST/eve_neutral.wav)"
