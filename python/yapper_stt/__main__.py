"""STT worker entrypoint — JSON-lines on stdin/stdout. Stub until Phase 1."""

from __future__ import annotations

import sys
from typing import NoReturn

from yapper_common.ipc import Response, dumps_response, loads_request


def main() -> NoReturn:
    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        try:
            req = loads_request(line)
        except Exception as exc:  # noqa: BLE001 — edge of process boundary
            sys.stdout.write(
                dumps_response(Response.failure("?", "bad_args", str(exc))) + "\n"
            )
            sys.stdout.flush()
            continue

        if req.cmd == "ping":
            resp = Response.success(
                req.id, {"role": "stt", "version": "0.1.0", "stub": True}
            )
        elif req.cmd == "shutdown":
            sys.stdout.write(dumps_response(Response.success(req.id)) + "\n")
            sys.stdout.flush()
            raise SystemExit(0)
        else:
            resp = Response.failure(
                req.id,
                "internal",
                f"stub worker: command {req.cmd!r} not implemented yet",
            )

        sys.stdout.write(dumps_response(resp) + "\n")
        sys.stdout.flush()

    raise SystemExit(0)


if __name__ == "__main__":
    main()
