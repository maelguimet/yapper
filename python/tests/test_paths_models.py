"""Paths and model download helper (uses already-cached small when present)."""

from __future__ import annotations

import os
from pathlib import Path

from yapper_common.models import ensure_whisper_model
from yapper_common.paths import data_dir, models_dir, whisper_models_dir


def test_paths_respect_yapper_data_dir(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("YAPPER_DATA_DIR", str(tmp_path / "data"))
    assert data_dir() == (tmp_path / "data").resolve()
    assert models_dir() == (tmp_path / "data" / "models").resolve()
    assert whisper_models_dir() == (tmp_path / "data" / "models" / "whisper").resolve()


def test_ensure_whisper_small_returns_existing_or_downloads() -> None:
    # Real download path into app models dir (small already present after smoke).
    path = ensure_whisper_model("small")
    assert path.is_file()
    assert path.stat().st_size > 1_000_000
