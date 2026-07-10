"""Paths and model download helper.

Unit path tests never download or import whisper. Host download path is
marked integration so `pytest -m 'not gpu'` stays offline-safe.

Config contract: Rust injects ``YAPPER_MODELS_DIR`` / ``YAPPER_VOICES_DIR`` from
``[models] dir`` / ``voices_dir``; workers and scripts resolve the same roots.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from yapper_common.models import ensure_whisper_model
from yapper_common.paths import (
    data_dir,
    models_dir,
    voices_dir,
    whisper_models_dir,
)
from yapper_stt.worker import SttWorker
from yapper_tts.tones import list_tone_names, resolve_tone
from yapper_tts.worker import TtsWorker


def test_paths_respect_yapper_data_dir(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.delenv("YAPPER_MODELS_DIR", raising=False)
    monkeypatch.delenv("YAPPER_VOICES_DIR", raising=False)
    monkeypatch.setenv("YAPPER_DATA_DIR", str(tmp_path / "data"))
    assert data_dir() == (tmp_path / "data").resolve()
    assert models_dir() == (tmp_path / "data" / "models").resolve()
    assert whisper_models_dir() == (tmp_path / "data" / "models" / "whisper").resolve()
    assert voices_dir() == (tmp_path / "data" / "voices").resolve()


def test_models_dir_env_overrides_data_dir(tmp_path: Path, monkeypatch) -> None:
    """Configured models.dir (via YAPPER_MODELS_DIR) wins over YAPPER_DATA_DIR."""
    monkeypatch.setenv("YAPPER_DATA_DIR", str(tmp_path / "data"))
    custom = tmp_path / "custom_models"
    monkeypatch.setenv("YAPPER_MODELS_DIR", str(custom))
    assert models_dir() == custom.resolve()
    assert whisper_models_dir() == (custom / "whisper").resolve()


def test_voices_dir_env_overrides_data_dir(tmp_path: Path, monkeypatch) -> None:
    """Configured models.voices_dir (via YAPPER_VOICES_DIR) wins over YAPPER_DATA_DIR."""
    monkeypatch.setenv("YAPPER_DATA_DIR", str(tmp_path / "data"))
    custom = tmp_path / "custom_voices"
    monkeypatch.setenv("YAPPER_VOICES_DIR", str(custom))
    assert voices_dir() == custom.resolve()


def test_stt_worker_uses_configured_whisper_root(tmp_path: Path, monkeypatch) -> None:
    """STT default download/load root follows YAPPER_MODELS_DIR (config models.dir)."""
    models = tmp_path / "cfg_models"
    monkeypatch.setenv("YAPPER_MODELS_DIR", str(models))
    monkeypatch.delenv("YAPPER_DATA_DIR", raising=False)
    worker = SttWorker()  # no explicit download_root — same as process entrypoint
    assert worker.download_root is None
    assert worker.whisper_root() == (models / "whisper").resolve()
    assert worker.whisper_root() == whisper_models_dir()


def test_stt_worker_explicit_download_root_wins(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("YAPPER_MODELS_DIR", str(tmp_path / "env_models"))
    explicit = tmp_path / "explicit_whisper"
    worker = SttWorker(download_root=explicit)
    assert worker.whisper_root() == explicit


def test_tts_worker_list_tones_uses_configured_voices_root(
    tmp_path: Path, monkeypatch
) -> None:
    """TTS tone listing follows YAPPER_VOICES_DIR when voices_root is unset."""
    voices = tmp_path / "cfg_voices"
    voices.mkdir()
    (voices / "default_neutral.wav").write_bytes(b"RIFF....WAVE")
    (voices / "default_calm.wav").write_bytes(b"RIFF....WAVE")
    monkeypatch.setenv("YAPPER_VOICES_DIR", str(voices))
    # Avoid accidental clone-gold pollution when installed dir is empty — ours has wavs.
    monkeypatch.delenv("YAPPER_TTS_CLONE", raising=False)
    monkeypatch.setenv("YAPPER_DATA_DIR", str(tmp_path / "unused_data"))

    worker = TtsWorker()  # process entrypoint shape: no voices_root arg
    assert worker.voices_root is None
    tones = list_tone_names(worker.voices_root)
    assert tones == ["calm", "neutral"]


def test_tts_resolve_tone_uses_configured_voices_root(
    tmp_path: Path, monkeypatch
) -> None:
    import json

    voices = tmp_path / "cfg_voices"
    voices.mkdir()
    (voices / "default_neutral.wav").write_bytes(b"RIFF....WAVE")
    (voices / "knobs.json").write_text(
        json.dumps({"neutral": {"exg": 0.42, "cfg": 0.5, "rate": 1.0}}),
        encoding="utf-8",
    )
    monkeypatch.setenv("YAPPER_VOICES_DIR", str(voices))
    monkeypatch.delenv("YAPPER_TTS_CLONE", raising=False)

    tone = resolve_tone("neutral")  # no voices_root → paths.voices_dir()
    assert tone.ref_wav.is_file()
    assert tone.ref_wav.name == "default_neutral.wav"
    assert tone.ref_wav.parent.resolve() == voices.resolve()
    assert tone.exaggeration == pytest.approx(0.42)


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


def test_ensure_whisper_uses_configured_models_dir(
    tmp_path: Path, monkeypatch
) -> None:
    """ensure_whisper_model without download_root uses whisper under YAPPER_MODELS_DIR."""
    models = tmp_path / "models"
    root = models / "whisper"
    root.mkdir(parents=True)
    target = root / "small.pt"
    with open(target, "wb") as fh:
        fh.truncate(450_000_000)
    monkeypatch.setenv("YAPPER_MODELS_DIR", str(models))
    path = ensure_whisper_model("small")
    assert path == target.resolve() or path == target
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
