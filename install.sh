#!/usr/bin/env bash
# Yapper installer — build binary, venv, voices, optional autostart.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${YAPPER_PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/yapper"
CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/yapper"
VENV="$DATA/venv"
DRY_RUN="${YAPPER_DRY_RUN:-0}"

log() { printf '==> %s\n' "$*"; }
warn() { printf 'WARN: %s\n' "$*" >&2; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

need() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

check_deps() {
  log "checking dependencies"
  need rustc
  need cargo
  need python3
  need ffmpeg
  need xclip
  need xdotool
  if ! command -v nvidia-smi >/dev/null 2>&1; then
    warn "nvidia-smi not found — CUDA loads may fail"
  fi
  if [[ -z "${DISPLAY:-}" ]]; then
    warn "DISPLAY unset — GUI/hotkeys need X11"
  fi
}

build_rust() {
  log "building yapper (release)"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "[dry-run] cargo build --release"
    return
  fi
  (cd "$ROOT" && cargo build --release)
  mkdir -p "$BIN_DIR"
  install -m 755 "$ROOT/target/release/yapper" "$BIN_DIR/yapper"
  log "installed $BIN_DIR/yapper"
}

setup_python() {
  log "Python venv at $VENV"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "[dry-run] create venv + install workers"
    return
  fi
  mkdir -p "$DATA"
  if [[ ! -d "$VENV" ]]; then
    # Reuse system torch/CUDA packages when present
    python3 -m venv --system-site-packages "$VENV"
  fi
  # shellcheck disable=SC1091
  source "$VENV/bin/activate"
  pip install -U pip setuptools wheel
  pip install -e "$ROOT/python[dev]" || pip install -e "$ROOT/python"
  # Best-effort ML deps (may already be system-site)
  pip install -r "$ROOT/python/requirements.txt" || warn "optional pip requirements incomplete"
  deactivate || true
}

install_voices() {
  log "installing Eve voice refs"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "[dry-run] scripts/install_voices.sh"
    return
  fi
  YAPPER_VOICES_DIR="$DATA/voices" bash "$ROOT/scripts/install_voices.sh" || warn "voice install failed"
}

download_models() {
  log "ensuring Whisper small model"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "[dry-run] download small"
    return
  fi
  local py="$VENV/bin/python"
  [[ -x "$py" ]] || py=python3
  PYTHONPATH="$ROOT/python" "$py" "$ROOT/scripts/download_models.py" small || warn "model download failed"
}

write_config() {
  log "config"
  if [[ "$DRY_RUN" == "1" ]]; then
    return
  fi
  mkdir -p "$CONFIG" "$DATA/models" "$DATA/logs"
  if [[ ! -f "$CONFIG/config.toml" ]]; then
    "$BIN_DIR/yapper" init-config || true
  fi
  # Point python paths at install
  if [[ -f "$CONFIG/config.toml" ]]; then
    # rewrite via a small python snippet for reliability
    PYTHONPATH="$ROOT/python" python3 - <<PY || true
from pathlib import Path
import re
p = Path("$CONFIG/config.toml")
text = p.read_text()
text = re.sub(r'python_root = ".*"', 'python_root = "$ROOT/python"', text)
text = re.sub(r'python_bin = ".*"', 'python_bin = "$VENV/bin/python"', text)
p.write_text(text)
print("updated python paths in", p)
PY
  fi
}

desktop_entry() {
  log "desktop entry"
  local apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
  local desktop="$apps/yapper.desktop"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "[dry-run] write $desktop"
    return
  fi
  mkdir -p "$apps"
  cat >"$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Yapper
Comment=Local STT + TTS (Whisper + Chatterbox)
Exec=$BIN_DIR/yapper
Icon=audio-input-microphone
Terminal=false
Categories=AudioVideo;Utility;
StartupNotify=true
EOF
  log "wrote $desktop"
}

prompt_autostart() {
  local mode="${YAPPER_AUTOSTART:-}"
  if [[ -z "$mode" ]]; then
    if [[ ! -t 0 ]]; then
      log "non-interactive: skip autostart (set YAPPER_AUTOSTART=user|all|no)"
      return
    fi
    echo "Start yapper on boot?"
    echo "  1) no"
    echo "  2) yes, this user only"
    echo "  3) yes, all users (needs sudo)"
    read -r -p "Choice [1]: " ans || ans=1
    case "${ans:-1}" in
      2) mode=user ;;
      3) mode=all ;;
      *) mode=no ;;
    esac
  fi
  case "$mode" in
    user)
      local ad="$HOME/.config/autostart"
      mkdir -p "$ad"
      cp "${XDG_DATA_HOME:-$HOME/.local/share}/applications/yapper.desktop" "$ad/yapper.desktop"
      log "autostart user: $ad/yapper.desktop"
      ;;
    all)
      if [[ "$DRY_RUN" == "1" ]]; then
        log "[dry-run] sudo install /etc/xdg/autostart/yapper.desktop"
        return
      fi
      sudo install -m 644 "${XDG_DATA_HOME:-$HOME/.local/share}/applications/yapper.desktop" \
        /etc/xdg/autostart/yapper.desktop
      log "autostart all users: /etc/xdg/autostart/yapper.desktop"
      ;;
    *)
      log "autostart: no"
      ;;
  esac
}

run_doctor() {
  log "doctor"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "[dry-run] yapper doctor"
    return
  fi
  export PATH="$BIN_DIR:$PATH"
  "$BIN_DIR/yapper" doctor || warn "doctor reported issues"
}

main() {
  log "Yapper install from $ROOT"
  check_deps
  build_rust
  setup_python
  install_voices
  download_models
  write_config
  desktop_entry
  prompt_autostart
  run_doctor
  log "done. Run: $BIN_DIR/yapper"
}

main "$@"
