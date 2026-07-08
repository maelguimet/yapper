# IPC protocol (draft)

Parent = Rust `yapper`. Workers = Python processes.

**Transport (v1):** JSON Lines over **stdin/stdout**.  
Stderr = human logs only (never protocol).

Each request is one JSON object ending with `\n`.  
Each response is one JSON object ending with `\n`.

## Common envelope

### Request
```json
{
  "id": "uuid-or-counter",
  "cmd": "ping|load|unload|transcribe|synthesize|shutdown|status",
  "params": {}
}
```

### Response
```json
{
  "id": "same-as-request",
  "ok": true,
  "result": {}
}
```
or
```json
{
  "id": "same-as-request",
  "ok": false,
  "error": {
    "code": "not_loaded|oom|bad_args|internal",
    "message": "human readable"
  }
}
```

## STT worker commands

| cmd | params | result |
|-----|--------|--------|
| `ping` | `{}` | `{ "role": "stt", "version": "..." }` |
| `status` | `{}` | `{ "loaded": bool, "model": "small"\|"medium"\|null, "device": "cuda"\|"cpu" }` |
| `load` | `{ "model": "small"\|"medium", "device": "cuda" }` | `{ "model": "...", "vram_mb": n }` |
| `unload` | `{}` | `{}` then process may stay up empty **or** parent kills — prefer stay up empty for faster reload? **v1: unload drops weights; process can stay for ping** |
| `transcribe` | `{ "path": "/tmp/x.wav", "language": "auto"\|"en"\|"fr" }` | `{ "text": "...", "language": "en" }` |
| `shutdown` | `{}` | `{}` then exit 0 |

## TTS worker commands

| cmd | params | result |
|-----|--------|--------|
| `ping` | `{}` | `{ "role": "tts", "version": "..." }` |
| `status` | `{}` | `{ "loaded": bool, "model": "chatterbox-multilingual"\|null, "device": "..." }` |
| `load` | `{ "model": "chatterbox-multilingual", "device": "cuda" }` | `{ "model": "...", "vram_mb": n }` |
| `unload` | `{}` | `{}` |
| `list_tones` | `{}` | `{ "tones": ["neutral", "calm", ...] }` |
| `synthesize` | `{ "text": "...", "language": "en"\|"fr", "tone": "neutral", "voice": "eve", "out_path": "/tmp/out.wav" }` | `{ "path": "...", "sample_rate": 24000 }` |
| `shutdown` | `{}` | `{}` then exit 0 |

## Concurrency

- Workers handle **one command at a time** (single-threaded event loop).
- Parent queues if needed.
- `load`/`unload` must not race with `transcribe`/`synthesize` (parent serializes).

## Versioning

Add `"proto": 1` on first request after spawn; mismatch → parent errors and restarts worker.
