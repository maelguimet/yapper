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
  echo "WARN: clone gold not found at $CLONE/gold — TTS will fail until refs are installed" >&2
  exit 1
fi
