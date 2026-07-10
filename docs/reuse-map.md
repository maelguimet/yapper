# Public reuse map (open source)

Yapper does **not** ship proprietary voice banks or model weights in git.

## Default Chatterbox reference audio

| What | Where |
|------|--------|
| Install script | `scripts/install_voices.sh` |
| Piper-generated neutral ref | `scripts/generate_default_reference.sh` |
| Attribution | `assets/voices/ATTRIBUTION.md` |
| Runtime layout | `$YAPPER_VOICES_DIR` or `~/.local/share/yapper/voices/` |

Files use the pattern `{voice}_{tone}.wav` (default voice id: `default`). A single `default_neutral.wav` is enough for all tones (emotion knobs in code / optional `knobs.json`).

## Chatterbox + Whisper weights

Downloaded at runtime into `YAPPER_MODELS_DIR` (see `scripts/download_models.py`). Chatterbox pulls from Hugging Face on first TTS load.

## Optional dev-only clone tree

`YAPPER_TTS_CLONE` may point at a **private** local tree for development. Never commit or redistribute those WAVs.

## Tone names

Canonical list: `python/yapper_tts/tones.py` (`DEFAULT_TONES`).