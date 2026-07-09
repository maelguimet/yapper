#!/usr/bin/env python3
"""Download Whisper models into the configured models root.

Default root: ``$YAPPER_MODELS_DIR/whisper`` if set, else
``~/.local/share/yapper/models/whisper`` (or ``$YAPPER_DATA_DIR/models/whisper``).

The Rust shell sets ``YAPPER_MODELS_DIR`` from ``config.toml`` ``[models] dir``
when spawning workers; use the same env (or ``--models-dir``) for standalone runs.
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

# Allow running without install
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "python"))

from yapper_common.models import WHISPER_SIZES, ensure_whisper_model  # noqa: E402
from yapper_common.paths import ensure_runtime_dirs, whisper_models_dir  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "sizes",
        nargs="*",
        default=list(WHISPER_SIZES),
        choices=list(WHISPER_SIZES),
        help="Whisper sizes to download (default: small medium)",
    )
    parser.add_argument(
        "--models-dir",
        default=None,
        help="Models root (sets YAPPER_MODELS_DIR; Whisper files go under <dir>/whisper)",
    )
    args = parser.parse_args()
    if args.models_dir:
        os.environ["YAPPER_MODELS_DIR"] = str(Path(args.models_dir).expanduser())
    ensure_runtime_dirs()
    print(f"download root: {whisper_models_dir()}")
    for size in args.sizes:
        path = ensure_whisper_model(size)
        print(f"ok {size}: {path} ({path.stat().st_size // (1024 * 1024)} MiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
