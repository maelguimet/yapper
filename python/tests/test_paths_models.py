"""Paths and model download helper.

Unit path tests never download or import whisper. Host download path is
marked integration so `pytest -m 'not gpu'` stays offline-safe.
"""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from yapper_common.models import ensure_whisper_model
from yapper_common.paths import data_dir, models_dir, whisper_models_dir


def test_paths_respect_yapper_data_dir(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("YAPPER_DATA_DIR", str(tmp_path / "data"))
    assert data_dir() == (tmp_path / "data").resolve()
    assert models_dir() == (tmp_path / "data" / "models").resolve()
    assert whisper_models_dir() == (tmp_path / "data" / "models" / "whisper").resolve()


def test_ensure_whisper_rejects_unsupported_size() -> None:
    with pytest.raises(ValueError, match="unsupported"):
        ensure_whisper_model("tiny")


def test_ensure_whisper_returns_existing_file_without_download(tmp_path: Path) -> None:
    """When weight is already large enough on disk, return it (no whisper import)."""
    root = tmp_path / "whisper"
    root.mkdir(parents=True)
    target = root / "small.pt"
    # Sparse file meets size gate without filling disk.
    with open(target, "wb") as fh:
        fh.truncate(450_000_000)
    path = ensure_whisper_model("small", download_root=root)
    assert path == target
    assert path.is_file()


@pytest.mark.integration
@pytest.mark.gpu
def test_ensure_whisper_small_returns_existing_or_downloads() -> None:
    """Host smoke: real small.pt cache or download (needs whisper + disk)."""
    try:
        import whisper  # noqa: F401
    except ImportError:
        pytest.skip("whisper not installed")
    path = ensure_whisper_model("small")
    assert path.is_file()
    assert path.stat().st_size > 1_000_000
