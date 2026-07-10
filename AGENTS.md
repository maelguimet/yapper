# Yapper — agent instructions

Local tray app for **STT + TTS** on Linux (no cloud APIs). Hybrid: **Rust** shell (GUI/tray/hotkeys) + **typed Python** workers (Whisper + Chatterbox).

**Repo:** clone anywhere (e.g. `~/projects/yapper`)
**After `/clear`:** read this file, then `TODO.md`, then `docs/design.md`.

---

## Product decisions (locked)

| # | Decision |
|---|----------|
| 1 | **Hybrid OK.** Rust app + typed Python backends. |
| 2 | **Simple TTS only** (text → audio). Not the heavy `narrate` director/LLM pipeline. |
| 3 | **One multilingual TTS model:** `ChatterboxMultilingualTTS` (`chatterbox.mtl_tts`) — includes `en` + `fr`. |
| 4 | **STT sizes:** Whisper **small** and **medium**, selectable in GUI. |
| 5 | **Hold-to-talk** for dictation. Always insert at cursor; **also copy to clipboard** with a GUI toggle. |
| 6 | **Read-aloud default = primary selection** (highlighted text). Toggle for “read clipboard”. |
| 7 | **Default voice `default`** — install `default_neutral.wav` (Piper ljspeech ref). Tone picker + knobs. Optional `YAPPER_TTS_CLONE` for private dev only. |
| 8 | Models under `~/.local/share/yapper/models`. **Unload frees RAM+VRAM** (kill/drop worker). Both loaded if they fit; if not, **keep the most recently used**, unload the other. |
| 9 | v1 targets: **GNOME-class Linux, X11, AppIndicator, NVIDIA CUDA recommended**. Wayland later. |
| 10 | Project path: **`~/projects/yapper`**. |
| 11 | v1 = tray + GUI + hotkeys + installer + README. No director, no Wayland. |
| 12 | **Always-on tray.** Window close/minimize **hides to top-bar tray**; process + hotkeys stay alive. **Quit only** via tray right-click → Quit (or explicit Exit). No tray icon = broken. |

---

## Architecture (target)

```
┌─────────────────────────────────────────────────────────┐
│  yapper (Rust binary)                                   │
│  - tray icon (Ayatana AppIndicator / StatusNotifier)    │
│  - main window (egui or GTK3)                           │
│  - global hotkeys (X11)                                 │
│  - clipboard / selection / paste inject (xclip+xdotool) │
│  - model lifecycle (load/unload, LRU dual-model policy) │
│  - spawns/kills Python workers over stdio JSON-RPC      │
└───────────────┬─────────────────────┬───────────────────┘
                │                     │
     ┌──────────▼──────────┐  ┌───────▼──────────────┐
     │ yapper-stt (Python) │  │ yapper-tts (Python)  │
     │ openai-whisper or   │  │ ChatterboxMultilingual│
     │ whisper.cpp wrap    │  │ + ref wav + tone knobs│
     │ small | medium      │  │ en | fr language_id   │
     └─────────────────────┘  └──────────────────────┘
```

**Prefer:** keep workers **not running** when models are unloaded. Load = start process + load weights; unload = graceful quit + `gc` (process exit frees all memory).

### VRAM policy
- Typical desktop GPU **8–12 GB**. Other apps may compete.
- Allow STT + TTS both loaded **if free VRAM allows**.
- If load would fail / not fit: unload the **least recently used** of the other role, retry.
- “Unload all” button for full free.

### Paths
| What | Where |
|------|--------|
| Config | `~/.config/yapper/config.toml` |
| Models | `~/.local/share/yapper/models/` |
| Voices (reference WAVs) | `~/.local/share/yapper/voices/` |
| Logs | `~/.local/share/yapper/logs/` |
| Autostart (user) | `~/.config/autostart/yapper.desktop` |
| Systemd user unit (optional) | `~/.config/systemd/user/yapper.service` |

---

## Assets & reuse

See **`docs/reuse-map.md`** for voice install, model paths, and tone lists. Do not commit weights or proprietary WAVs.

**Do not** depend on the full `narrate` pipeline (Ollama director). Ship simple TTS only.

### Chatterbox multilingual API (installed)
```python
from chatterbox.mtl_tts import ChatterboxMultilingualTTS
# SUPPORTED_LANGUAGES includes "en", "fr", ...
model = ChatterboxMultilingualTTS.from_pretrained(device=torch.device("cuda"))
# generate(..., language_id="en"|"fr", audio_prompt_path=ref_wav, exaggeration=..., cfg_weight=...)
```
HF weights live under `~/.cache/huggingface/hub/models--ResembleAI--chatterbox` (~3 GB). App may symlink or re-download into `~/.local/share/yapper/models`.

---

## GUI requirements

- **Tray:** always-on icon (like Proton/MEGA/Steam). Menu: Open, Load/Unload STT, Load/Unload TTS, Quit.
- **Main window:**
  - STT: record / stop (or hold), transcript box, **Copy**, clear
  - STT from **audio file**
  - TTS: paste/type text, **Speak**, stop
  - TTS from **text file**
  - **Load / Unload** STT and TTS (separate + unload all)
  - **Model selectors:** STT = small | medium; TTS = multilingual Chatterbox (slot for future swaps)
  - **Tone picker** (named emotions + knobs)
  - **Language** for TTS: auto / en / fr (and STT language auto/en/fr)
  - Toggles: copy transcript to clipboard on dictation; read clipboard vs selection for read-aloud
  - Hotkey rebind UI (persist to config)
- **Global hotkeys (defaults TBD, rebindable):**
  1. **Read selection (or clipboard) out loud**
  2. **Hold-to-talk → insert at cursor** (no auto-send/Enter)

### X11 helpers (already on this machine)
- Selection/clipboard: `xclip`
- Type/paste at cursor: `xdotool` (prefer clipboard paste `ctrl+v` over fake key-per-char for Unicode)
- Mic/playback: PipeWire/Pulse (`pactl`, cpal/rodio, or `ffmpeg`)

---

## Installer & boot

- One-command install from git, e.g.  
  `curl -fsSL https://raw.githubusercontent.com/<user>/yapper/main/install.sh | bash`  
  or `./install.sh` after clone.
- Installer:
  - Check deps (see README)
  - Build Rust release binary
  - Create Python venv + install STT/TTS deps
  - Install desktop entry + tray autostart optional
  - Prompt **yes/no start on boot**
  - If yes: **active user only** *or* **all users** (system unit / `/etc/xdg/autostart` with sudo)
- README: hard requirements list (distro assumptions OK)

---

## Stack preferences

| Layer | Choice | Notes |
|-------|--------|--------|
| Language | Rust 2021 + Python 3.10+ typed | `mypy`/`pyright`-friendly; use `typing`, dataclasses/pydantic |
| GUI | Prefer **egui + tray-icon** *or* **GTK3 + ayatana-appindicator** | This machine has GTK3 + ayatana; gtk4-dev missing. Pick one and stick. |
| Hotkeys | X11 global grab (e.g. `global-hotkey` crate) or GNOME custom keybindings writing to our IPC | Prefer in-app rebind without requiring GNOME Settings |
| STT | openai-whisper in Python worker first (already installed stack); optional later whisper.cpp | Defaults: small + medium |
| TTS | `ChatterboxMultilingualTTS` only for v1 | Reference WAV + tone knobs |
| IPC | JSON lines over stdio or Unix socket | Stable schema in `docs/ipc.md` |
| Config | TOML | |
| Packaging | `install.sh` + optional `.deb` later | Avoid snap/flatpak for v1 (CUDA pain) |

**Minimize deps**, but torch/CUDA/whisper/chatterbox are required for quality.

---

## Coding conventions

- Rust: small modules, explicit errors (`thiserror`/`anyhow`), no `unwrap` in production paths without comment.
- Python: typed functions, no bare `except:`, workers must exit cleanly on unload and free GPU (`torch.cuda.empty_cache()` before exit is optional if process dies).
- Do not commit model weights or large WAVs; install copies/symlinks Eve assets from `tts/clone` at setup.
- No cloud STT/TTS APIs.
- Prefer fixing root causes over skipping checks.

### File size (soft / hard caps)

Keep modules readable — **do not grow god-files**.

| Cap | Lines (rough) | Rule |
|-----|----------------|------|
| **Soft** | **~300** | Prefer splitting when a file approaches this. New logic goes into a focused module, not “just one more fn” on the pile. |
| **Hard** | **~500** | Do not land new work that leaves a non-generated source file **over 500 lines** without splitting in the same change set. Rare exception: a single data table or pure test module, noted in the PR/commit. |

**Known offenders (clean up under TODO B28 before more feature work):** `src/app.rs` (~1800), then `audio.rs` / `transport.rs` / `x11util.rs` / `hotkeys.rs` if still over hard cap after app split.

- `cargo build` / install should not scream about **unused functions** — wire them or delete them; avoid papering with `#[allow(dead_code)]` unless the symbol is part of a stable public surface used by tests only (then `#[cfg(test)]` or document why).
- Prefer **thin `eframe::App` / `main`** orchestration; UI tabs, theme, transport queue, tray lifecycle as separate modules.

---

## Agent / test discipline (mandatory)

**Always use a hard timeout when running anything that can hang.** Never wait unbounded on tests, workers, GUI, model load, or smokes.

| Kind | Default max | How |
|------|-------------|-----|
| Unit tests (CPU) | **60s** total | `timeout 60s cargo test …` / `timeout 60s pytest …` |
| X11 helper tests | **30s** | Prefer **Xvfb** isolation; never paste into the user’s live session |
| STT GPU smoke (small) | **180s** | `timeout 180s …` or `subprocess.run(..., timeout=180)` |
| STT GPU smoke (medium) | **300s** | same |
| TTS GPU smoke | **300s** | same |
| GUI launch smoke | **10s** | `timeout 10s yapper` / `timeout 10s cargo run -- gui` |
| Install dry-run | **120s** | `timeout 120s env YAPPER_DRY_RUN=1 ./install.sh` |
| Full install / model download | **900s** | still wrap with `timeout`; kill + log if exceeded |

Rules:

1. Shell: wrap with `timeout <N>s <cmd>` (GNU coreutils). On timeout, treat as failure, capture what you have, fix or report — do not spin.
2. Python: every `subprocess.run` / worker pipe must pass `timeout=…` (seconds).
3. Rust tests that talk to X11, spawn processes, or wait on channels must bound waits (`Duration` + kill child); no infinite `recv` without a test-level deadline.
4. Prefer **Xvfb** for paste/xdotool tests so focus-steal cannot hang waiting on the user’s terminal.
5. Do not pipeline long smokes into `tail` without timeouts (can look “stuck”); write logs with `tee` and bound the whole pipeline: `timeout 300s bash -c '… | tee log'`.
6. If a command hits the timeout twice after a fix attempt, stop and record a blocker — do not re-run open-ended.

---

## Machine facts (dev host)

- Pop!_OS, GNOME, **X11** (`DISPLAY=:1`)
- GPU: NVIDIA GeForce RTX 4070 12 GB, driver 580.x
- Mic: TONOR TC30 (Pulse default source)
- Rust: 1.92 / cargo present
- Python: 3.10, torch 2.6+cu124 available

---

## Out of scope (v1)

- Full emotion director / Ollama / beat pipeline
- Wayland / `wtype` / portal capture
- Cloud providers (edge-tts, ElevenLabs, xAI voice)
- Multi-speaker conversation UI
- Shipping multi-GB weights inside the git repo
