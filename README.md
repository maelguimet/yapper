# Yapper

Local **speech-to-text** and **text-to-speech** tray app for Linux.  
Whisper (STT) + Chatterbox multilingual (TTS). No cloud STT/TTS APIs.

> Status: **v0.2 (X11)** — always-on tray, rebindable hotkeys, streaming TTS, transport controls.

## Features

- **Always-on system tray** (Open / Load-Unload STT-TTS / Quit). Close or minimize **hides to tray**; process and hotkeys stay alive. **Quit only** via tray → Quit (or confirmed Exit in Settings).
- GUI tabs: **Dictate**, **Speak**, **Settings** (dark theme, status chips + cards)
- Load / unload models (free VRAM **and** RAM when unloaded)
- Model selectors (Whisper small/medium; Chatterbox multilingual)
- **Default voice** + tone picker (Chatterbox reference WAVs + emotion knobs; see `assets/voices/ATTRIBUTION.md`)
- **Streaming / chunked TTS**: long text is split into sentences; first audio starts after the first segment
- **Playback transport**: Play / Pause / Resume / Stop / Replay, progress scrubber, volume (mpv IPC; falls back to ffplay/paplay)
- Global hotkeys (rebindable; Capture picker + Apply):
  - **Super+Shift+S** — read selected text aloud (optional: clipboard)
  - **Super+Shift+R** — hold-to-talk → insert transcript at cursor (no auto-send)
- Installer with optional start-on-boot (current user or all users)

## You need these things

### Hard requirements (v1)

The installer **fails** if any hard tool below is missing (message names the tool and what breaks).

| Requirement | Why (what breaks without it) |
|-------------|------------------------------|
| Linux **x86_64** | Supported target |
| **X11** session (`DISPLAY`) | GUI, tray, global hotkeys, paste injection |
| **GNOME** (or DE with AppIndicator/SNI tray) | Always-on tray icon (StatusNotifier) |
| **NVIDIA GPU + CUDA-capable drivers** (`nvidia-smi`) | Fast local models (CPU is unusably slow) |
| **Rust** toolchain (`rustc`, `cargo`) | Build the app |
| **Python 3.10+** | STT/TTS workers |
| **ffmpeg** | Audio decode/helpers for workers and tooling |
| **arecord** (`alsa-utils`) | Mic capture (dictation / hold-to-talk) |
| **xclip**, **xdotool** | Clipboard/selection and paste-at-cursor |
| **PulseAudio or PipeWire** (Pulse compat) | Mic + playback |
| Disk: **~5–15 GB** free for models + venv | Whisper + Chatterbox weights |

`nvidia-smi` and `DISPLAY` are checked with a clear **WARN** (install continues) because some headless/CI dry-runs still need the script to plan; live GUI+CUDA use still requires them.

### Preferred / optional (installer warns with impact)

| Tool | What you lose if missing |
|------|--------------------------|
| **mpv** (preferred) | TTS pause/seek and multi-chunk playlist; falls back to per-file **ffplay**/paplay |
| **ffplay** | Last-resort TTS player when mpv is missing (usually ships with ffmpeg) |
| **pactl** | Mic source listing/refresh degraded (Pulse/PipeWire control) |
| AppIndicator / SNI host | Always-on tray UX may break (see tray section below) |

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

### Voice references (Chatterbox)

Install creates a **default neutral reference** (Piper `ljspeech` — public-domain dataset; see `assets/voices/ATTRIBUTION.md`). Optional `YAPPER_TTS_CLONE` is for **local dev only** (do not redistribute proprietary WAVs).

- Extra VRAM headroom (12 GB class GPU is comfortable if you unload when using other GPU apps)
- Whisper **medium** (~1.5 GiB) — not downloaded by default; see install flags below

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
# non-interactive examples:
#   YAPPER_AUTOSTART=user ./install.sh
#   YAPPER_AUTOSTART=no ./install.sh
#   YAPPER_DRY_RUN=1 ./install.sh              # plan only; no cargo/pip/model network
#   YAPPER_MODELS=small ./install.sh           # default: Whisper small only
#   YAPPER_MODELS=small,medium ./install.sh    # also pre-fetch medium (~1.5 GiB)
```

| What | Where |
|------|--------|
| Binary | `~/.local/bin/yapper` |
| Python venv + workers | `~/.local/share/yapper/venv` |
| Models / voices / logs | `~/.local/share/yapper/` |
| Config | `~/.config/yapper/config.toml` (`python_bin` = install venv; `python_root` empty) |

#### Installer env flags

| Variable | Default | Effect |
|----------|---------|--------|
| `YAPPER_DRY_RUN=1` | off | Print plan only: dep checks + which Whisper sizes would be ensured. **No** cargo build, pip, model download, or writes under XDG install paths. |
| `YAPPER_MODELS` | `small` | Comma/space list of Whisper sizes to ensure at install: `small`, `medium`, or `small,medium`. Invalid sizes abort install. |
| `YAPPER_AUTOSTART` | prompt / skip if non-TTY | `user` \| `all` \| `no` |
| `YAPPER_DEV_INSTALL=1` | off | Editable `python[dev]` into app venv (dev only; not for daily-driver) |
| `YAPPER_PREFIX` | `~/.local` | Binary install prefix (`$PREFIX/bin/yapper`) |

#### Whisper model policy

- **Default install** ensures **small only** (faster, less disk). The UI still offers **medium**.
- **Pre-fetch both:** `YAPPER_MODELS=small,medium ./install.sh` (or re-run download: `YAPPER_MODELS_DIR=~/.local/share/yapper/models python scripts/download_models.py small medium`).
- **Lazy first use:** if you select a size that is not on disk, the STT worker’s `whisper.load_model(..., download_root=…)` may **network-download** that checkpoint into the models dir on first load. Prefer pre-fetching offline or with `YAPPER_MODELS` if you need a hermetic install.
- Chatterbox multilingual weights come from Hugging Face cache / first TTS load (same local-only runtime; no cloud TTS API).

```bash
yapper doctor   # host + worker ping checks (import from venv)
yapper          # launch GUI + tray + hotkeys
```

Self-contained smoke (isolated temp venv, no checkout on `PYTHONPATH`):

```bash
timeout 180s env YAPPER_SCRATCH=/tmp/yapper-smoke ./scripts/smoke_self_contained_install.sh
```

Installer honesty unit tests (dep classification + `YAPPER_MODELS` parsing, no network):

```bash
timeout 30s ./scripts/test_install_truth.sh
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
# non-GPU Python suite (from repo root; markers registered in root pytest.ini)
PYTHONPATH=python pytest -q -m 'not gpu'
# equivalent: PYTHONPATH=python pytest -q python/tests -m 'not gpu'
# optional GPU smokes: PYTHONPATH=python pytest -q -m gpu
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
| Hold-to-talk | `Super+Shift+R` | Press start / release stop → insert via clipboard+ctrl+v; restores prior CLIPBOARD when **Copy transcript** is off (best-effort on X11) |

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

## Local TTS API

While `yapper gui` is running, agents and scripts on the **same user session** can queue speech via a Unix socket (not TCP). See `docs/tts-api.md` and `docs/agent-tts-tool.md`. Client: `scripts/yapper-tts speak "Hello"`.

## Config & data

| Path | Purpose |
|------|---------|
| `~/.config/yapper/config.toml` | Settings, hotkeys, mic source, model/voice dirs |
| `~/.local/share/yapper/models/` | Default Whisper weights (`[models] dir`) |
| `~/.local/share/yapper/voices/` | Chatterbox reference WAVs (`default_*.wav`, optional `knobs.json`) |
| `~/.local/share/yapper/venv/` | Installer Python env |
| `~/.local/share/yapper/logs/` | Logs |

### Model and voice directories

`config.toml` `[models]` is **honored** at runtime (not decorative):

```toml
[models]
dir = "/home/you/.local/share/yapper/models"       # Whisper under <dir>/whisper/
voices_dir = "/home/you/.local/share/yapper/voices"  # e.g. default_neutral.wav + knobs.json
```

The shell injects these as `YAPPER_MODELS_DIR` and `YAPPER_VOICES_DIR` when spawning
STT/TTS workers. Standalone tools use the same env:

```bash
YAPPER_MODELS_DIR=/path/to/models python scripts/download_models.py small
# or: python scripts/download_models.py --models-dir /path/to/models small
YAPPER_VOICES_DIR=/path/to/voices ./scripts/install_voices.sh
```

Defaults match XDG under `~/.local/share/yapper/`. `yapper doctor` reports the
configured roots and checks Whisper/voice files there.

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

## Contributing & security

- [CONTRIBUTING.md](CONTRIBUTING.md)
- [SECURITY.md](SECURITY.md)

## License

MIT — see [LICENSE](LICENSE).
