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

## Chunked TTS (parent-side, v0.2)

Worker protocol is unchanged: still one `synthesize` per request with a single `text` string.

The **parent** (`segment::split_for_tts`) splits long monologues into sentences and issues sequential `synthesize` calls (one segment → one temp WAV → `AudioTransport` play). Status text surfaces `synthesizing i/n`. Cancel clears the parent queue and stops the player; in-flight synthesize may still finish but is not played if cancelled before queue pump.

No new worker commands are required for v0.2 streaming.

## Versioning

Add `"proto": 1` on first request after spawn; mismatch → parent errors and restarts worker.
