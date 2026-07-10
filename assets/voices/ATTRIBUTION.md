# Default voice references (Chatterbox)

Yapper uses **Chatterbox** with a short reference WAV per tone. For open installs we ship a **default** voice generated from a redistributable Piper model.

## Piper model (reference generation only)

- Voice: `en_US-ljspeech-medium`
- Source: [rhasspy/piper-voices](https://huggingface.co/rhasspy/piper-voices/tree/main/en/en_US/ljspeech/medium)
- Dataset: LJ Speech — public domain (see that voice's `MODEL_CARD`)

Download into `$YAPPER_MODELS_DIR/piper/` (or `~/.local/share/yapper/models/piper/`):

- `en_US-ljspeech-medium.onnx`
- `en_US-ljspeech-medium.onnx.json`

Then run:

```bash
YAPPER_VOICES_DIR=~/.local/share/yapper/voices \
  scripts/generate_default_reference.sh
```

This writes `default_neutral.wav`. Other tones reuse that reference with emotion knobs from `knobs.json` defaults.

## Not shipped

- Proprietary **Eve** voice assets
- Pre-trained Chatterbox weights (downloaded at runtime via Hugging Face)
- Any personal clone trees under `~/projects/tts/clone`

## Developer override (local only)

`YAPPER_TTS_CLONE` may point at a private clone tree for development. Do not commit or redistribute those WAVs.