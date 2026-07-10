# Yapper local TTS API

## Accepted product contract

Yapper exposes a small automation API **only while the tray application is running**. The API reuses Yapper's existing TTS worker, model lifecycle, sentence segmentation, restart/cancel behavior, tone/language settings, and audio transport. It must not launch a second Chatterbox model or bypass Yapper's VRAM policy.

## Security boundary

- Transport: HTTP/1.1 over a Unix-domain socket, never TCP.
- Socket: `$XDG_RUNTIME_DIR/yapper/tts-api.sock` (normally `/run/user/$UID/yapper/tts-api.sock`).
- Parent directory mode: `0700`; socket mode: `0600`.
- A pre-existing non-socket path is never deleted.
- Requests are bounded and processed through a bounded queue.
- No request text is written to logs.
- Remote use is through authenticated SSH to the host, running `scripts/yapper-tts` or `curl --unix-socket` **on that host**; the socket is not published as a LAN port.

## API

### `GET /health`

Returns `200` while the API listener is running. Unexpected request bodies are rejected.

```json
{"status":"ok"}
```

### `POST /v1/speak`

Queues text through the same restart-and-speak path as the GUI. The current Yapper language, tone, voice, and transport settings apply. A new request interrupts/restarts an existing utterance exactly like pressing **Speak** again.

Request:

```json
{"text":"Hello from Yapper."}
```

Requirements:

- `Content-Type: application/json`
- `Content-Length` required and must match the JSON body
- Unknown JSON fields are rejected
- Text is trimmed and sanitized (control characters and bidi overrides removed; newlines preserved)

Response: `202 Accepted` after the command enters Yapper's bounded UI queue. This acknowledges acceptance, not completed synthesis/playback.

Limits:

- JSON body: 64 KiB maximum.
- Spoken text: 10,000 Unicode scalar values maximum after sanitization.
- Empty or whitespace-only text: `400` with `code: empty_text`.
- Full command queue: `429` with `code: queue_full`.

### `POST /v1/stop`

Queues the same cancel/stop behavior as the GUI **Stop** action and returns `202`.

## Error shape

```json
{"error":"human-readable message","code":"machine_code"}
```

Common `code` values: `bad_request`, `empty_text`, `text_too_long`, `invalid_json`, `unsupported_media_type`, `queue_full`, `not_found`.

## Non-goals

- No LAN/public listener.
- No cloud synthesis.
- No second model process.
- No completion callbacks or persistent speech history in v1.
- No per-request tone/language override in v1; callers use the active Yapper settings.

## Example

On the Linux host where Yapper runs:

```bash
curl --unix-socket "$XDG_RUNTIME_DIR/yapper/tts-api.sock" \
  -H 'Content-Type: application/json' \
  -d '{"text":"Hello from Yapper."}' \
  http://localhost/v1/speak
```

See also `docs/agent-tts-tool.md` for the supported `scripts/yapper-tts` client and agent guidance.