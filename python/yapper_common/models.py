"""Model download helpers for Whisper (and future assets)."""

from __future__ import annotations

import logging
from pathlib import Path

from yapper_common.paths import ensure_runtime_dirs, whisper_models_dir

log = logging.getLogger("yapper.models")

WHISPER_SIZES = ("small", "medium")


def ensure_whisper_model(size: str, download_root: Path | None = None) -> Path:
    """Download Whisper checkpoint into app models dir if missing. Returns file path."""
    if size not in WHISPER_SIZES:
        raise ValueError(f"unsupported whisper size: {size!r}")

    ensure_runtime_dirs()
    root = download_root or whisper_models_dir()
    root.mkdir(parents=True, exist_ok=True)
    target = root / f"{size}.pt"
    if target.is_file() and target.stat().st_size > 1_000_000:
        return target

    import whisper

    log.info("downloading whisper %s → %s", size, root)
    # load_model downloads into download_root; we immediately drop the model
    model = whisper.load_model(size, device="cpu", download_root=str(root))
    del model
    if not target.is_file():
        # whisper may use a hashed name in some versions; accept any recent .pt
        candidates = sorted(root.glob("*.pt"), key=lambda p: p.stat().st_mtime, reverse=True)
        if not candidates:
            raise FileNotFoundError(f"whisper download for {size} did not produce a .pt under {root}")
        return candidates[0]
    return target
