# Reuse map (this machine)

Quick pointers for implementers. Prefer **copying ideas and small helpers**, not hard-coding absolute paths into the shipped product (installer may *discover* these paths on the dev machine).

## Eve tones

```
/home/maelguimet/projects/tts/clone/gold/eve_*.wav   # preferred refs
/home/maelguimet/projects/tts/clone/prompts/eve_*.wav
/home/maelguimet/projects/tts/clone/knobs.json       # exg, cfg, rate
/home/maelguimet/projects/tts/scripts/emotion_map.py # names + descriptions
```

Tones present: neutral, calm, caring, confused, excited, sad, angry, serious,
sensual, teasing, conspiratorial, motivational, romantic, unhinged, whisper.

## Chatterbox

```
# Multilingual class
~/.local/lib/python3.10/site-packages/chatterbox/mtl_tts.py
  ChatterboxMultilingualTTS
  SUPPORTED_LANGUAGES: en, fr, ...

# Working venv (reference only)
~/projects/grok-chat/aidra/services/tts/venv

# Load/unload + CUDA patterns
~/projects/supergemma-assistant/services/tts_server.py

# HF cache (~3G)
~/.cache/huggingface/hub/models--ResembleAI--chatterbox
```

## Whisper

```
~/.local/bin/whisper
~/.local/lib/python3.10/site-packages/whisper/
/mnt/lexar-ai/model-tools/whisper-cache/base.pt   # only base today
```

Yapper should download **small** + **medium** into `~/.local/share/yapper/models`.

## X11 / tray on this host

- Session: X11, Pop GNOME
- `xclip`, `xdotool` installed
- Ayatana AppIndicator + GNOME appindicators extension
- pkg-config: `gtk+-3.0`, `ayatana-appindicator3-0.1` (not gtk4.pc)
