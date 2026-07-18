# Yapper design

## Goal

A self-contained Linux **tray application** for local speech-to-text and text-to-speech using **Whisper** (STT) and **Chatterbox multilingual** (TTS). No paid/cloud STT or TTS APIs.

## Users

Primarily the machine owner on Pop!_OS / Ubuntu GNOME **X11** with an NVIDIA GPU. Installer should work on similar environments with documented prerequisites.

## Non-goals (v0.2 primary)

- LLM-directed multi-emotion narration (`projects/tts/narrate`)
- Wayland support (Phase 13 follow-on)
- Perfect multi-distro packaging
- Embedding multi-GB weights in git

## Always-on tray (v0.2)

- Window close / minimize → hide (`Visible(false)`); process stays up.
- Tray **Open** → show + focus; tray **Quit** → hard exit only.
- In-window **Exit…** requires confirmation (same as tray Quit).
- Desktop/session autostart runs `yapper --hidden`; manual launch still opens the window.

## Chunked TTS + transport (v0.2)

- Parent splits text (`segment::split_for_tts`) and loops `synthesize` per sentence.
- Playback via `transport::AudioTransport` (mpv IPC preferred): pause / seek / replay / volume.
- Status: idle | buffering | speaking | paused; queue length surfaced in UI.

## Components

### 1. Rust application (`yapper`)

Responsibilities:

- Process lifetime (single main process)
- Tray + main GUI
- Global hotkeys
- Clipboard / selection / key injection
- Mic capture coordination and audio playback orchestration
- Starting/stopping Python workers
- Config persistence
- Enforcing VRAM policy (both models if fit; else LRU keep)

Does **not** load torch models itself.

### 2. STT worker (`yapper-stt`)

Typed Python process.

- Models: Whisper **small**, **medium** (GUI selectable)
- Inputs: raw PCM/WAV path or file path (mp3/wav/… via ffmpeg or whisper loaders)
- Output: transcript string + optional segments
- Fully exits on unload

### 3. TTS worker (`yapper-tts`)

Typed Python process.

- Model: `ChatterboxMultilingualTTS`
- Inputs: text, `language_id` (`en`/`fr`/…), tone name → reference wav + knobs
- Output: audio path or bytes (WAV preferred internally; optional mp3 via ffmpeg)
- Default voice identity: **`default`** (`default_<tone>.wav`; install generates `default_neutral.wav` via Piper — see `assets/voices/ATTRIBUTION.md`)
- Fully exits on unload

## Tone system

Source of truth for v1:

| Piece | Source |
|-------|--------|
| Tone names | `emotion_map.EMOTIONS` keys / gold filenames |
| Reference audio | `{voice}_<tone>.wav` under voices dir; `default_neutral.wav` required for Speak |
| Knobs | optional `knobs.json` → exaggeration, cfg_weight, rate (defaults in code) |

GUI shows human-readable tone list (Neutral, Calm, Excited, …). Changing tone only changes ref+knobs for the next synth; no LLM director.

## Hotkey flows

### Read aloud

1. Hotkey pressed
2. If “read clipboard” toggle on → `CLIPBOARD`; else → `PRIMARY` selection (`xclip -selection primary`)
3. If empty, notify and stop
4. Ensure TTS loaded (may unload STT if needed)
5. Split into segments → synthesize first → play; continue queue (streaming path)
6. Do not steal focus aggressively

### Hold-to-talk insert

1. Hotkey **press** → start recording default mic
2. Hotkey **release** → stop, write temp wav
3. Ensure STT loaded (may unload TTS if needed)
4. Transcribe
5. Paste at focused app via CLIPBOARD + `ctrl+v` (Unicode-safe; **never** send Enter / submit)
6. **Copy transcript** toggle controls final CLIPBOARD only:
   - **On:** leave transcript in CLIPBOARD after paste
   - **Off:** restore previous CLIPBOARD after paste (best-effort; empty prior → empty)

Paste always **temporarily** writes the transcript to CLIPBOARD so `xdotool key ctrl+v` can inject. That is an implementation detail of the insert path, not “Copy transcript.”

#### X11 clipboard restore limits (best-effort)

Restoring prior CLIPBOARD after paste **cannot be hard-guaranteed** on X11:

- **Races:** another client may claim CLIPBOARD between paste and restore.
- **xclip ownership:** `xclip -i` may exit while a short-lived owner holds the selection; concurrent readers can race.
- **Empty selection:** `xclip -o` often exits non-zero on empty; we treat that as empty and restore-to-empty.
- **Target apps:** some apps re-write CLIPBOARD on paste/focus and overwrite our restore.
- **No Wayland path in v1.**

Product intent when Copy is off: do **not intentionally** leave the transcript in CLIPBOARD after a successful insert. Manual GUI Dictate (Record/Stop) never pastes; it only copies when the toggle is on.

## VRAM / memory policy

```
request load(role, model):
  if role already loaded with same model: ok
  if role loaded with different model: unload role, load new
  try load
  if OOM or estimated no-fit:
    unload other role if loaded
    retry load
  mark role as MRU
```

Unload = kill worker process (frees RAM + VRAM). Optional future: keep worker alive with weights on CPU — **not v1**.

## Config sketch (`~/.config/yapper/config.toml`)

```toml
[stt]
model = "small"          # small | medium
language = "auto"        # auto | en | fr
copy_transcript = true

[tts]
model = "chatterbox-multilingual"
language = "auto"        # auto | en | fr
tone = "neutral"
voice = "eve"

[read_aloud]
source = "selection"     # selection | clipboard

[hotkeys]
read_aloud = "Super+Shift+S"
push_to_talk = "Super+Shift+R"

[models]
dir = "~/.local/share/yapper/models"          # honored: YAPPER_MODELS_DIR for workers
voices_dir = "~/.local/share/yapper/voices"   # honored: YAPPER_VOICES_DIR for workers
```

Whisper weights live at `{models.dir}/whisper/{small,medium}.pt`. The Rust shell
passes `models.dir` / `models.voices_dir` into Python via env so path resolution is
shared by workers, `download_models.py`, `install_voices.sh`, and `yapper doctor`.


## Installer sketch

1. Detect OS / X11 / nvidia-smi / rustc / python3
2. Build `cargo build --release`
3. Create venv, install python deps
4. Install default voice references (`scripts/install_voices.sh`)
5. Optionally download whisper small/medium into models dir
6. Install binary to `~/.local/bin/yapper`
7. Write `.desktop` file
8. Ask boot options:
   - No
   - Yes, this user (`~/.config/autostart/`, tray-only)
   - Yes, all users (sudo → `/etc/xdg/autostart/` or systemd, tray-only)

## Security / privacy

- All audio stays local
- No network required after model download
- Workers only accept commands from the parent via local IPC

## Testing strategy

- Unit: config parse, tone map load, IPC encode/decode
- Integration: worker smoke with tiny fixtures
- Manual: hotkeys in gedit/terminal, `nvidia-smi` before/after unload
