"""TTS worker handlers without loading the full model."""

from __future__ import annotations

from pathlib import Path

from yapper_common.ipc import Request
from yapper_tts.worker import TtsWorker


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
