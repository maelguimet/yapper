# Yapper

Local **speech-to-text** and **text-to-speech** tray app for Linux.  
Whisper (STT) + Chatterbox multilingual (TTS). No cloud STT/TTS APIs.

> Status: **v0.2 (X11)** — always-on tray, rebindable hotkeys, streaming TTS, transport controls.

## Features

- **Always-on system tray** (Open / Load-Unload STT-TTS / Quit). Close or minimize **hides to tray**; process and hotkeys stay alive. **Quit only** via tray → Quit (or confirmed Exit in Settings).
- GUI tabs: **Dictate**, **Speak**, **Settings** (dark theme, status chips + cards)
- Load / unload models (free VRAM **and** RAM when unloaded)
- Model selectors (Whisper small/medium; Chatterbox multilingual)
- Eve voice + tone picker (from `tts/clone` gold + knobs)
- **Streaming / chunked TTS**: long text is split into sentences; first audio starts after the first segment
- **Playback transport**: Play / Pause / Resume / Stop / Replay, progress scrubber, volume (mpv IPC; falls back to ffplay/paplay)
- Global hotkeys (rebindable; Capture picker + Apply):
  - **Super+Shift+S** — read selected text aloud (optional: clipboard)
  - **Super+Shift+R** — hold-to-talk → insert transcript at cursor (no auto-send)
- Installer with optional start-on-boot (current user or all users)

## You need these things

### Hard requirements (v1)

| Requirement | Why |
|-------------|-----|
| Linux **x86_64** | Supported target |
| **X11** session | Global hotkeys, selection, paste injection |
| **GNOME** (or DE with AppIndicator/SNI tray) | Always-on tray icon (StatusNotifier) |
| **NVIDIA GPU + CUDA-capable drivers** (`nvidia-smi`) | Fast local models |
| **Rust** toolchain (`rustc`, `cargo`) | Build the app |
| **Python 3.10+** | STT/TTS workers |
| **ffmpeg** (+ **ffplay** optional fallback) | Audio helpers |
| **mpv** (preferred) | Controllable TTS playback (pause/seek) |
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
  ffmpeg mpv alsa-utils xclip xdotool \
  python3 python3-venv python3-dev \
  portaudio19-dev
# NVIDIA driver already working: nvidia-smi
# Rust: https://rustup.rs
# GNOME tray: gnome-shell-extension-appindicator (often preinstalled on Pop/Ubuntu)
```

### Optional

- Existing Eve voice bank at `~/projects/tts/clone` (installer copies/symlinks tones)
- Extra VRAM headroom (RTX 4070 12 GB class is fine if you unload when using other GPU apps)

### Always-on tray (required for ship UX)

Yapper is a **tray app**, not a document window:

1. Launch → tray icon appears (GNOME needs **AppIndicator / StatusNotifier** host).
2. Window **close (X)** or **minimize** → window hides; tray + hotkeys + models stay.
3. Tray **Open** → show + focus window.
4. Tray **Quit** → only hard exit (unloads models).

If the tray icon is missing, `yapper doctor` reports host/lib status and the GUI shows a loud error. Install/enable `gnome-shell-extension-appindicator` (package) / `ubuntu-appindicators@ubuntu.com` on GNOME.

### Not supported in v0.2 primary

- Wayland-only sessions (Phase 13 follow-on)
- macOS / Windows
- Silent success without a tray host (GUI still runs, but always-on UX is broken — doctor warns)

## Install (user — self-contained)

Normal install copies the binary and **non-editable** Python workers into stable
XDG locations. After install you may **move or delete the source checkout**; the
app keeps working via `~/.local/share/yapper/venv` (workers in site-packages).
Runtime config does **not** point workers at the git tree.

```bash
git clone <repo-url> yapper
cd yapper
./install.sh
# prompts: start on boot? this user / all users / no
# non-interactive:
#   YAPPER_AUTOSTART=user ./install.sh
#   YAPPER_AUTOSTART=no ./install.sh
#   YAPPER_DRY_RUN=1 ./install.sh   # plan only; no mutations
```

| What | Where |
|------|--------|
| Binary | `~/.local/bin/yapper` |
| Python venv + workers | `~/.local/share/yapper/venv` |
| Models / voices / logs | `~/.local/share/yapper/` |
| Config | `~/.config/yapper/config.toml` (`python_bin` = install venv; `python_root` empty) |

```bash
yapper doctor   # host + worker ping checks (import from venv)
yapper          # launch GUI + tray + hotkeys
```

Self-contained smoke (isolated temp venv, no checkout on `PYTHONPATH`):

```bash
timeout 180s env YAPPER_SCRATCH=/tmp/yapper-smoke ./scripts/smoke_self_contained_install.sh
```

### Dev without install (editable / repo)

**Dev-only.** Editable install and `[dev]` extras stay in the checkout `.venv`.
This path is for hacking on workers; it is **not** what `./install.sh` does for
users. Prefer `cargo run` + repo `python/` on `PYTHONPATH`.

```bash
cd ~/projects/yapper
python3 -m venv --system-site-packages .venv
.venv/bin/pip install -U pip setuptools wheel
.venv/bin/pip install -e 'python[dev]'   # editable + pytest/mypy/ruff — dev only
./scripts/install_voices.sh
.venv/bin/python scripts/download_models.py small medium
cargo run -- doctor
cargo run -- gui
cd python && PYTHONPATH=. pytest -q -m 'not gpu'
# optional GPU smokes:
# cd python && PYTHONPATH=. pytest -q -m gpu
cargo test --locked
```

Optional: force the installer into editable mode (still not recommended for
daily-driver installs): `YAPPER_DEV_INSTALL=1 ./install.sh`.

Worker smokes from the repo:

```bash
echo '{"id":"1","cmd":"ping"}' | PYTHONPATH=python .venv/bin/python -m yapper_stt
echo '{"id":"1","cmd":"list_tones"}' | PYTHONPATH=python .venv/bin/python -m yapper_tts
```

## Hotkey defaults

| Action | Default | Notes |
|--------|---------|--------|
| Read aloud | `Super+Shift+S` | Primary selection by default; toggle clipboard in GUI |
| Hold-to-talk | `Super+Shift+R` | Press start / release stop → insert via clipboard+ctrl+v |

Rebind in **Settings → Hotkeys**: Capture a combo (or type advanced), then **Apply hotkeys**. Apply drops previous X11 grabs and registers the new ones live. Grab failures stay as a yellow banner until fixed (DE conflict or bad combo). Defaults and host examples (`Alt+Shift+S` / `Alt+Shift+Q`) parse the same way.

### Streaming TTS & transport

Long text is split into sentences (EN/FR-aware, abbreviation-safe) and synthesized **one segment at a time**. Playback starts when the first segment is ready. Controls on the **Speak** tab:

| Control | Behavior |
|---------|----------|
| Speak | Chunk + synth + play queue |
| Pause / Resume | mpv pause (when available) |
| Stop | Clear queue, kill player |
| Replay | Last successful chunk without re-synth |
| Seek scrubber | Seek within current segment |
| Volume | App-level mpv volume |

Read-aloud hotkey uses the same streaming path.

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
