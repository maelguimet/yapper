# Yapper

Local **speech-to-text** and **text-to-speech** tray app for Linux.  
Whisper (STT) + Chatterbox multilingual (TTS). No cloud STT/TTS APIs.

> Status: **v0.1.0** — workers, GUI, hotkeys, installer on Pop/Ubuntu X11 + NVIDIA CUDA.

## Features

- System tray icon (Open / Load-Unload STT-TTS / Quit)
- GUI: STT ↔ copyable text, paste/type → TTS, file STT, file TTS
- Load / unload models (free VRAM **and** RAM when unloaded; workers stay up)
- Model selectors (Whisper small/medium; Chatterbox multilingual)
- Eve voice + tone picker (from `tts/clone` gold + knobs)
- Global hotkeys (rebindable):
  - **Super+Shift+S** — read selected text aloud (optional: clipboard)
  - **Super+Shift+R** — hold-to-talk → insert transcript at cursor (no auto-send)
- Installer with optional start-on-boot (current user or all users)

## You need these things

### Hard requirements (v1)

| Requirement | Why |
|-------------|-----|
| Linux **x86_64** | Supported target |
| **X11** session | Global hotkeys, selection, paste injection |
| **GNOME** (or DE with AppIndicator/SNI tray) | Tray icon |
| **NVIDIA GPU + CUDA-capable drivers** (`nvidia-smi`) | Fast local models |
| **Rust** toolchain (`rustc`, `cargo`) | Build the app |
| **Python 3.10+** | STT/TTS workers |
| **ffmpeg** | Audio decode/encode helpers |
| **arecord** (`alsa-utils`) | Mic capture (Pulse/PipeWire via ALSA plugin) |
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
  ffmpeg alsa-utils xclip xdotool \
  python3 python3-venv python3-dev \
  portaudio19-dev
# NVIDIA driver already working: nvidia-smi
# Rust: https://rustup.rs
```

### Optional

- Existing Eve voice bank at `~/projects/tts/clone` (installer copies/symlinks tones)
- Extra VRAM headroom (RTX 4070 12 GB class is fine if you unload when using other GPU apps)

### Not supported in v1

- Wayland-only sessions
- macOS / Windows
- Running without a tray/status-notifier host (GUI still works)

## Install

```bash
git clone <repo-url> yapper
cd yapper
./install.sh
# prompts: start on boot? this user / all users / no
# non-interactive:
#   YAPPER_AUTOSTART=user ./install.sh
#   YAPPER_AUTOSTART=no ./install.sh
#   YAPPER_DRY_RUN=1 ./install.sh
```

Installs binary to `~/.local/bin/yapper`, venv under `~/.local/share/yapper/venv`,
voices/models under `~/.local/share/yapper/`, desktop entry for the app menu.

```bash
yapper doctor   # host + worker ping checks
yapper          # launch GUI + tray + hotkeys
```

### Dev without install

```bash
cd ~/projects/yapper
python3 -m venv --system-site-packages .venv
.venv/bin/pip install -U pip setuptools wheel
.venv/bin/pip install -e 'python[dev]'
./scripts/install_voices.sh
.venv/bin/python scripts/download_models.py small medium
cargo run -- doctor
cargo run -- gui
.venv/bin/python -m pytest python/tests -m "not gpu"
.venv/bin/python -m pytest python/tests -m gpu   # needs CUDA
cargo test
```

Worker smokes:

```bash
echo '{"id":"1","cmd":"ping"}' | PYTHONPATH=python .venv/bin/python -m yapper_stt
echo '{"id":"1","cmd":"list_tones"}' | PYTHONPATH=python .venv/bin/python -m yapper_tts
```

## Hotkey defaults

| Action | Default | Notes |
|--------|---------|--------|
| Read aloud | `Super+Shift+S` | Primary selection by default; toggle clipboard in GUI |
| Hold-to-talk | `Super+Shift+R` | Press start / release stop → insert via clipboard+ctrl+v |

Rebind in the GUI (persists to config). Grab failures show a yellow warning in the window.

## Config & data

| Path | Purpose |
|------|---------|
| `~/.config/yapper/config.toml` | Settings, hotkeys, mic source |
| `~/.local/share/yapper/models/` | Whisper weights |
| `~/.local/share/yapper/voices/` | Eve tone references |
| `~/.local/share/yapper/venv/` | Installer Python env |
| `~/.local/share/yapper/logs/` | Logs |

### Microphone (Pulse / PipeWire)

Capture uses **arecord** via the Pulse ALSA device (`-D pulse` or `-D pulse:<source>`). Continuous ffmpeg pulse capture writes empty files when stopped without `-t` on PipeWire hosts, so it is not used for hold-to-talk. Empty `audio.mic_source` in config means the **system default** source.

```toml
[audio]
# Empty = Pulse default. Or a full source name from `pactl list sources short`:
# mic_source = "alsa_input.usb-…_TONOR_TC30_….mono-fallback"
mic_source = ""
```

Tips:

- List sources: `pactl list sources short`
- Default source: `pactl get-default-source` / set with `pactl set-default-source <name>`
- Prefer a real **input** (mic) over `*.monitor` sinks (those capture playback)
- On this class of host, USB mics like **TONOR TC30** should appear as `alsa_input.usb-…TONOR…`
- GUI: microphone dropdown + **Refresh**, live level while recording, device name in status
- `yapper doctor` prints the default source and runs a ~1s energy probe (non-silence / silence / skip reason)
- Flatpak sandboxes often block Pulse device access — run the native binary for mic capture

## Layout

```
yapper/
  AGENTS.md
  TODO.md
  HANDOFF.md
  README.md
  install.sh
  Cargo.toml
  src/                 # Rust shell (GUI, tray, hotkeys, IPC client)
  python/              # STT/TTS workers + shared IPC
  scripts/             # download_models.py, install_voices.sh
  docs/
```

## License

MIT
