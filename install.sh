#!/usr/bin/env bash
# Yapper installer — build binary, self-contained venv workers, voices, optional autostart.
# After install the app does not need the source checkout (user install).
# Dev editable install is separate — see README "Dev without install".
#
# Env flags:
#   YAPPER_DRY_RUN=1          plan only; no mutations (no cargo/pip/network models)
#   YAPPER_MODELS=small       Whisper sizes to ensure (default: small)
#   YAPPER_MODELS=small,medium
#   YAPPER_DEV_INSTALL=1      editable python[dev] into app venv (dev only)
#   YAPPER_AUTOSTART=user|all|no
#   YAPPER_PREFIX=~/.local    binary install prefix
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

# Default install fetches Whisper small only (~500 MiB). Medium (~1.5 GiB) is
# optional via YAPPER_MODELS; first UI load of a missing size may still
# network-download via openai-whisper (documented in README).
DEFAULT_YAPPER_MODELS="small"
ALLOWED_WHISPER_SIZES="small medium"

# Hard build/runtime tools: "tool|what breaks without it"
# Shared by live install and dry-run; tests assert coverage against README.
HARD_DEPS=(
  "rustc|cannot build the yapper binary"
  "cargo|cannot build the yapper binary"
  "python3|cannot create STT/TTS worker venv"
  "ffmpeg|audio decode/helpers used by workers and tooling"
  "arecord|mic capture for dictation/hold-to-talk (install alsa-utils)"
  "xclip|clipboard/selection for paste-at-cursor and read-aloud"
  "xdotool|paste-at-cursor injection (ctrl+v) for hold-to-talk"
)

# Preferred/optional tools: "tool|what you lose if missing"
OPTIONAL_DEPS=(
  "mpv|TTS pause/seek and multi-chunk playlist fall back to per-file ffplay/paplay"
  "ffplay|last-resort TTS player when mpv is missing (usually with ffmpeg package)"
  "pactl|mic source listing/refresh degraded (PulseAudio/PipeWire control)"
)

log() { printf '==> %s\n' "$*"; }
warn() { printf 'WARN: %s\n' "$*" >&2; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

# Overridable for unit tests (see scripts/test_install_truth.sh).
have_cmd() {
  command -v "$1" >/dev/null 2>&1
}

# Resolve Whisper sizes from YAPPER_MODELS (comma and/or whitespace separated).
# Default: small. Allowed: small, medium. Prints unique sizes in canonical order.
# Dies on empty or unknown tokens.
parse_yapper_models() {
  # Unset → default. Explicit empty string → die (do not silently re-default).
  local raw="${YAPPER_MODELS-$DEFAULT_YAPPER_MODELS}"
  raw="${raw//,/ }"
  # shellcheck disable=SC2086
  set -- $raw
  if [[ $# -eq 0 ]]; then
    die "YAPPER_MODELS is empty — use e.g. YAPPER_MODELS=small or small,medium"
  fi

  local want_small=0 want_medium=0
  local tok
  for tok in "$@"; do
    case "$tok" in
      small) want_small=1 ;;
      medium) want_medium=1 ;;
      *)
        die "invalid YAPPER_MODELS size: ${tok@Q} (allowed: ${ALLOWED_WHISPER_SIZES// /, })"
        ;;
    esac
  done

  local out=()
  [[ "$want_small" -eq 1 ]] && out+=(small)
  [[ "$want_medium" -eq 1 ]] && out+=(medium)
  printf '%s\n' "${out[*]}"
}

check_hard_deps() {
  local entry tool impact
  local missing=0
  for entry in "${HARD_DEPS[@]}"; do
    tool="${entry%%|*}"
    impact="${entry#*|}"
    if have_cmd "$tool"; then
      log "  hard ok    $tool"
    else
      printf 'ERROR: missing required tool: %s — %s\n' "$tool" "$impact" >&2
      missing=1
    fi
  done
  if [[ "$missing" -ne 0 ]]; then
    die "install cannot continue: fix missing hard tools above (see README hard requirements)"
  fi
}

check_optional_deps() {
  local entry tool impact
  for entry in "${OPTIONAL_DEPS[@]}"; do
    tool="${entry%%|*}"
    impact="${entry#*|}"
    if have_cmd "$tool"; then
      log "  optional ok $tool"
    else
      warn "missing optional tool: $tool — $impact"
    fi
  done
}

check_cuda_and_x11() {
  if have_cmd nvidia-smi; then
    log "  host ok    nvidia-smi (CUDA GPU path)"
  else
    warn "nvidia-smi not found — CUDA model loads will fail or fall back to CPU (very slow)"
  fi
  if [[ -n "${DISPLAY:-}" ]]; then
    log "  host ok    DISPLAY=${DISPLAY} (X11)"
  else
    warn "DISPLAY unset — GUI, tray, global hotkeys, and paste injection need an X11 session"
  fi
}

check_tray_host() {
  local found=0
  local paths=(
    /usr/lib/x86_64-linux-gnu/libayatana-appindicator3.so.1
    /usr/lib/libayatana-appindicator3.so.1
    /usr/lib/x86_64-linux-gnu/libappindicator3.so.1
    /usr/share/gnome-shell/extensions/ubuntu-appindicators@ubuntu.com
    /usr/share/gnome-shell/extensions/appindicatorsupport@rgcjonas.gmail.com
  )
  local p
  for p in "${paths[@]}"; do
    if [[ -e "$p" ]]; then
      found=1
      break
    fi
  done
  if [[ "$found" -eq 1 ]]; then
    log "  host ok    AppIndicator/SNI tray host bits present"
  else
    warn "AppIndicator/SNI tray host not detected — always-on tray UX may break (install gnome-shell-extension-appindicator / ayatana)"
  fi
}

check_deps() {
  log "checking dependencies (hard fail / optional warn with impact)"
  check_hard_deps
  check_optional_deps
  check_cuda_and_x11
  check_tray_host
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
  log "installing default voice references"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "[dry-run] scripts/install_voices.sh → $DATA/voices (YAPPER_VOICES_DIR)"
    return
  fi
  # Same env workers use (config models.voices_dir → YAPPER_VOICES_DIR).
  YAPPER_VOICES_DIR="$DATA/voices" bash "$ROOT/scripts/install_voices.sh" || warn "voice install failed"
}

download_models() {
  local sizes
  sizes="$(parse_yapper_models)"
  # shellcheck disable=SC2086
  set -- $sizes
  log "ensuring Whisper model(s): $*"
  log "  (YAPPER_MODELS=${YAPPER_MODELS:-$DEFAULT_YAPPER_MODELS}; default is small only)"
  if [[ " $* " != *" medium "* ]]; then
    warn "Whisper medium not in install set — first UI load of medium may network-download ~1.5 GiB into $DATA/models/whisper (or re-run: YAPPER_MODELS=small,medium ./install.sh)"
  fi
  if [[ "$DRY_RUN" == "1" ]]; then
    local s
    for s in "$@"; do
      log "[dry-run] download $s → $DATA/models (YAPPER_MODELS_DIR)"
    done
    return
  fi
  local py="$VENV/bin/python"
  [[ -x "$py" ]] || py=python3
  # Same env workers use (config models.dir → YAPPER_MODELS_DIR).
  # shellcheck disable=SC2086
  YAPPER_MODELS_DIR="$DATA/models" "$py" "$ROOT/scripts/download_models.py" "$@" \
    || warn "model download failed"
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
      if [[ "$DRY_RUN" == "1" ]]; then
        log "[dry-run] write $ad/yapper.desktop (user autostart)"
        return
      fi
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
  log "Whisper sizes to ensure: $(parse_yapper_models)  (set YAPPER_MODELS to change)"
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

# Allow scripts/test_install_truth.sh to source functions without running install.
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
