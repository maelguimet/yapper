# Security policy

## Supported versions

Only the current `main` branch is supported for security fixes.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security problems.

Contact the maintainer privately with:

- Description and impact
- Steps to reproduce
- Affected version/commit if known

We will acknowledge receipt and aim to respond within a reasonable timeframe.

## Design notes (local threat model)

- Yapper's automation API is a **Unix socket** under `$XDG_RUNTIME_DIR/yapper/` (mode `0600`), not a network listener.
- Request text is **not logged** by the API layer.
- Workers run as the **same user** as the GUI; any process running as that user can call the API.
- Do not expose the socket via TCP forwarding unless you understand the trust boundary.

See `docs/tts-api.md` for the API contract.