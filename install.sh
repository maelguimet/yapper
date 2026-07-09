#!/usr/bin/env bash
# Yapper installer — build binary, self-contained venv workers, voices, optional autostart.
# After install the app does not need the source checkout (user install).
# Dev editable install is separate — see README "Dev without install".
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${YAPPER_PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/yapper"
CONFIG="${XDG_CONFIG_HOME:-$HOME/.config}/yapper"
VENV="$DATA/venv"
DRY_RUN="${YAPPER_DRY_RUN:-0}"
# Dev-only: editable + [dev] extras into the app venv (not for normal install).
DEV_INSTALL="${YAPPER_DEV_INSTALL:-0}"

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
    log "[dry-run] cargo build --release → $BIN_DIR/yapper"
    return
  fi
  (cd "$ROOT" && cargo build --release)
  mkdir -p "$BIN_DIR"
  install -m 755 "$ROOT/target/release/yapper" "$BIN_DIR/yapper"
  log "installed $BIN_DIR/yapper"
}

setup_python() {
  log "Python venv at $VENV (self-contained workers)"
  if [[ "$DRY_RUN" == "1" ]]; then
    if [[ "$DEV_INSTALL" == "1" ]]; then
      log "[dry-run] create venv + pip install -e python[dev] (YAPPER_DEV_INSTALL=1)"
    else
      log "[dry-run] create venv + non-editable pip install python/ (no [dev])"
      log "[dry-run] workers land in $VENV — no runtime dependency on $ROOT"
    fi
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
  if [[ "$DEV_INSTALL" == "1" ]]; then
    log "dev install: editable python[dev] (requires checkout at $ROOT)"
    pip install -e "$ROOT/python[dev]" || pip install -e "$ROOT/python"
  else
    # Non-editable: package copied into venv site-packages; checkout may be deleted after.
    pip install "$ROOT/python"
  fi
  # Best-effort ML deps (may already be system-site)
  pip install -r "$ROOT/python/requirements.txt" || warn "optional pip requirements incomplete"
  deactivate || true
}

install_voices() {
  log "installing Eve voice refs"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "[dry-run] scripts/install_voices.sh → $DATA/voices"
    return
  fi
  YAPPER_VOICES_DIR="$DATA/voices" bash "$ROOT/scripts/install_voices.sh" || warn "voice install failed"
}

download_models() {
  log "ensuring Whisper small model"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "[dry-run] download small → $DATA/models"
    return
  fi
  local py="$VENV/bin/python"
  [[ -x "$py" ]] || py=python3
  # Package is installed into the venv; no PYTHONPATH=$ROOT needed.
  "$py" "$ROOT/scripts/download_models.py" small || warn "model download failed"
}

write_config() {
  log "config (stable paths under $DATA / $CONFIG)"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "[dry-run] python_bin=$VENV/bin/python"
    log "[dry-run] python_root= (empty → import from venv site-packages)"
    return
  fi
  mkdir -p "$CONFIG" "$DATA/models" "$DATA/logs"
  if [[ ! -f "$CONFIG/config.toml" ]]; then
    "$BIN_DIR/yapper" init-config || true
  fi
  # Point runtime at the install venv only — never at the source checkout.
  if [[ -f "$CONFIG/config.toml" ]]; then
    python3 - <<PY || true
from pathlib import Path
import re
p = Path("$CONFIG/config.toml")
text = p.read_text()
# Empty python_root: workers import from python_bin's site-packages.
text = re.sub(r'python_root = ".*"', 'python_root = ""', text)
text = re.sub(r'python_bin = ".*"', 'python_bin = "$VENV/bin/python"', text)
p.write_text(text)
print("updated python paths in", p)
print("  python_bin = $VENV/bin/python")
print("  python_root = (empty; venv site-packages)")
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
    log "[dry-run] yapper doctor (worker pings via installed venv)"
    return
  fi
  export PATH="$BIN_DIR:$PATH"
  "$BIN_DIR/yapper" doctor || warn "doctor reported issues"
}

main() {
  log "Yapper install from $ROOT"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "mode: dry-run (no mutations); plan is self-contained user install"
  else
    log "mode: user install → binary + venv workers under XDG data (checkout optional after)"
  fi
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
  log "checkout may be moved/deleted; runtime uses $VENV only"
}

main "$@"
