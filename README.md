# Yapper

Local **speech-to-text** and **text-to-speech** tray app for Linux.  
Whisper (STT) + Chatterbox multilingual (TTS). No cloud STT/TTS APIs.

> Status: **early scaffolding**. See `TODO.md` and `AGENTS.md`.

## Features (target)

- System tray icon (lives with Proton / Steam / MEGA)
- GUI: STT ↔ copyable text, paste/type → TTS, file STT, file TTS
- Load / unload models (free VRAM **and** RAM when unloaded)
- Model selectors (Whisper small/medium; Chatterbox multilingual)
- Eve voice + tone picker
- Global hotkeys:
  - Read selected text aloud (optional: clipboard)
  - Hold-to-talk → insert transcript at cursor (no auto-send)
- Installer with optional start-on-boot (current user or all users)

## You need these things

### Hard requirements (v1)

| Requirement | Why |
|-------------|-----|
| Linux **x86_64** | Supported target |
| **X11** session | Global hotkeys, selection, paste injection |
| **GNOME** (or DE with AppIndicator/SNI tray) | Tray icon |
| **NVIDIA GPU + CUDA-capable drivers** (`nvidia-smi`) | Fast local models (CPU possible later, not first-class) |
| **Rust** toolchain (`rustc`, `cargo`) | Build the app |
| **Python 3.10+** | STT/TTS workers |
| **ffmpeg** | Audio decode/encode helpers |
| **xclip**, **xdotool** | Clipboard/selection and paste-at-cursor |
| **PulseAudio or PipeWire** (Pulse compat) | Mic + playback |
| Disk: **~5–15 GB** free for models + venv | Whisper medium + Chatterbox weights |

### Recommended packages (Debian/Ubuntu/Pop)

```bash
sudo apt update
sudo apt install -y \
  build-essential pkg-config \
  libgtk-3-dev libayatana-appindicator3-dev \
  libx11-dev libxi-dev libxtst-dev \
  ffmpeg xclip xdotool \
  python3 python3-venv python3-dev \
  portaudio19-dev
# NVIDIA driver already working: nvidia-smi
# Rust: https://rustup.rs
```

### Optional

- Existing Eve voice bank at `~/projects/tts/clone` (installer can copy tones)
- Extra VRAM headroom (RTX 4070 12 GB class is fine if you unload when using other GPU apps)

### Not supported in v1

- Wayland-only sessions
- macOS / Windows
- Running without a tray/status-notifier host

## Install (planned)

```bash
git clone <repo-url> yapper
cd yapper
./install.sh
# prompts: start on boot? this user / all users / no
```

One-liner (once published):

```bash
curl -fsSL https://raw.githubusercontent.com/<user>/yapper/main/install.sh | bash
```

## Dev layout

```
yapper/
  AGENTS.md          # agent + architecture rules
  TODO.md            # task board
  HANDOFF.md         # cold-start for a fresh session
  README.md
  docs/
    design.md
    ipc.md
  # (upcoming) src/  python/  install.sh  assets/
```

## Config & data

| Path | Purpose |
|------|---------|
| `~/.config/yapper/config.toml` | Settings, hotkeys |
| `~/.local/share/yapper/models/` | Whisper + TTS weights |
| `~/.local/share/yapper/voices/` | Eve tone references |
| `~/.local/share/yapper/logs/` | Logs |

## License

TBD.
