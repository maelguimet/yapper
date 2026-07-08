"""TTS worker entrypoint — JSON-lines on stdin/stdout."""

from __future__ import annotations

import logging
import sys
from typing import NoReturn

from yapper_common.ipc import Response, dumps_response, loads_request
from yapper_tts.worker import TtsWorker

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [yapper-tts] %(levelname)s %(message)s",
    stream=sys.stderr,
)


def main() -> NoReturn:
    worker = TtsWorker()
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

        resp = worker.handle(req)
        sys.stdout.write(dumps_response(resp) + "\n")
        sys.stdout.flush()

        if req.cmd == "shutdown" and resp.ok:
            raise SystemExit(0)

    raise SystemExit(0)


if __name__ == "__main__":
    main()
