#!/usr/bin/env python3
"""Download Whisper models into ~/.local/share/yapper/models/whisper."""

from __future__ import annotations

import argparse
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
    args = parser.parse_args()
    ensure_runtime_dirs()
    print(f"download root: {whisper_models_dir()}")
    for size in args.sizes:
        path = ensure_whisper_model(size)
        print(f"ok {size}: {path} ({path.stat().st_size // (1024 * 1024)} MiB)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
