"""TTS worker handlers without loading the full model."""

from __future__ import annotations

from pathlib import Path

import numpy as np

from yapper_common.ipc import Request
from yapper_tts.worker import (
    TtsWorker,
    _output_sane,
    _to_float_array,
    _write_wav,
)


def test_ping_role() -> None:
    w = TtsWorker()
    resp = w.handle(Request(id="1", cmd="ping"))
    assert resp.ok
    assert resp.result["role"] == "tts"


def test_status_unloaded() -> None:
    w = TtsWorker()
    resp = w.handle(Request(id="2", cmd="status"))
    assert resp.ok
    assert resp.result["loaded"] is False


def test_list_tones_returns_list() -> None:
    w = TtsWorker()
    resp = w.handle(Request(id="3", cmd="list_tones"))
    assert resp.ok
    assert isinstance(resp.result["tones"], list)
    assert len(resp.result["tones"]) > 0


def test_synthesize_not_loaded() -> None:
    w = TtsWorker()
    resp = w.handle(
        Request(
            id="4",
            cmd="synthesize",
            params={
                "text": "hi",
                "language": "en",
                "tone": "neutral",
                "out_path": "/tmp/x.wav",
            },
        )
    )
    assert not resp.ok
    assert resp.error is not None
    assert resp.error.code == "not_loaded"


def test_load_rejects_unknown_model() -> None:
    w = TtsWorker()
    resp = w.handle(
        Request(id="5", cmd="load", params={"model": "other-tts", "device": "cuda"})
    )
    assert not resp.ok
    assert resp.error is not None
    assert resp.error.code == "bad_args"


def test_synthesize_empty_text() -> None:
    w = TtsWorker()
    w.state.model = object()
    w.state.model_name = "chatterbox-multilingual"
    resp = w.handle(
        Request(
            id="6",
            cmd="synthesize",
            params={"text": "  ", "language": "en", "out_path": "/tmp/x.wav"},
        )
    )
    assert not resp.ok
    assert resp.error is not None
    assert resp.error.code == "bad_args"


def test_output_sane_rejects_empty_and_silent() -> None:
    silent = np.zeros(24000, dtype=np.float32)
    assert not _output_sane("hello world", 0.0, silent)
    assert not _output_sane("hello world", 1.0, silent)
    tone = np.sin(np.linspace(0, 100, 24000)).astype(np.float32) * 0.2
    assert _output_sane("hi", 1.0, tone)


def test_write_wav_nan_safe(tmp_path: Path) -> None:
    arr = np.array([0.0, float("nan"), 0.5, float("inf"), -0.25], dtype=np.float32)
    out = tmp_path / "n.wav"
    _write_wav(out, arr, 24000)
    assert out.is_file()
    assert out.stat().st_size > 44
    cleaned = _to_float_array(arr)
    assert not np.isnan(cleaned).any()
    assert not np.isinf(cleaned).any()


def test_synthesize_bad_language(tmp_path: Path) -> None:
    w = TtsWorker()
    w.state.model = object()
    w.state.model_name = "chatterbox-multilingual"
    out = tmp_path / "o.wav"
    resp = w.handle(
        Request(
            id="7",
            cmd="synthesize",
            params={"text": "bonjour", "language": "de", "out_path": str(out)},
        )
    )
    assert not resp.ok
    assert resp.error is not None
    assert resp.error.code == "bad_args"


def test_unload_ok() -> None:
    w = TtsWorker()
    assert w.handle(Request(id="8", cmd="unload")).ok
