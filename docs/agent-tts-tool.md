# Agent / automation: Yapper TTS

Use this when an agent (or script) should speak through **Yapper on the same Linux user session** while the tray app is running.

## Preconditions

1. `yapper gui` (or tray autostart) is running.
2. Speak is enabled: a neutral reference WAV exists under the configured voices directory (see `scripts/install_voices.sh`).
3. You run commands **as the same UID** as Yapper (Unix socket, mode `0600`).

## Do not send

- Markdown, JSON wrappers, tool traces, or stage directions unless that text should be spoken aloud.
- Secrets, tokens, or private paths in the spoken string (they are not logged, but they would be heard).
- Rapid-fire `/v1/speak` spam; `accepted` means **queued**, not finished. Use `/v1/stop` before a new utterance if you need clean interrupts.

## Socket

```text
$XDG_RUNTIME_DIR/yapper/tts-api.sock
```

## Client (recommended)

From the repo or install tree:

```bash
scripts/yapper-tts health
scripts/yapper-tts speak "Hello."
scripts/yapper-tts stop
```

Environment overrides:

| Variable | Meaning |
|----------|---------|
| `YAPPER_TTS_SOCKET` | Override socket path |
| `YAPPER_TTS_MAX_CHARS` | Client-side guard (default 10000) |

## HTTP shape (if you call curl yourself)

```bash
curl --unix-socket "${XDG_RUNTIME_DIR}/yapper/tts-api.sock" \
  -H 'Content-Type: application/json' \
  -d '{"text":"Hello."}' \
  http://localhost/v1/speak
```

- `GET /health` → `200 {"status":"ok"}`
- `POST /v1/speak` → `202 {"status":"accepted"}` when queued
- `POST /v1/stop` → `202 {"status":"accepted"}`

Errors return JSON `{"error":"...","code":"..."}` with `4xx` as documented in `docs/tts-api.md`.

## Remote host (SSH)

The API is **not** a network port. From another machine, run the client **on the PC** over SSH:

```bash
ssh user@host 'XDG_RUNTIME_DIR=/run/user/UID scripts/yapper-tts speak "Hello."'
```

Do not expose the socket via TCP reverse forwarding unless you fully understand the trust boundary.

## Hermes / tool definition sketch

```yaml
name: yapper_speak
description: Queue text for local Yapper TTS on the user's PC (tray app must be running).
parameters:
  text:
    type: string
    description: Plain speech text only; no markdown or metadata.
```

Implementation: invoke `scripts/yapper-tts` over SSH to the configured host, then verify `health` before `speak` when diagnosing failures.