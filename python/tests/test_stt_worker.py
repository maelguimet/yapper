"""STT worker handlers: unit paths without requiring a loaded model."""

from __future__ import annotations

from pathlib import Path

from yapper_common.ipc import Request
from yapper_stt.worker import SttWorker


def test_ping_role_and_version() -> None:
    w = SttWorker()
    resp = w.handle(Request(id="1", cmd="ping"))
    assert resp.ok
    assert resp.result["role"] == "stt"
    assert "version" in resp.result


def test_status_unloaded_by_default() -> None:
    w = SttWorker()
    resp = w.handle(Request(id="2", cmd="status"))
    assert resp.ok
    assert resp.result["loaded"] is False
    assert resp.result["model"] is None


def test_transcribe_without_load_is_not_loaded() -> None:
    w = SttWorker()
    resp = w.handle(
        Request(id="3", cmd="transcribe", params={"path": "/tmp/nope.wav", "language": "en"})
    )
    assert not resp.ok
    assert resp.error is not None
    assert resp.error.code == "not_loaded"


def test_load_rejects_bad_model() -> None:
    w = SttWorker()
    resp = w.handle(Request(id="4", cmd="load", params={"model": "tiny", "device": "cpu"}))
    assert not resp.ok
    assert resp.error is not None
    assert resp.error.code == "bad_args"
    assert "small" in resp.error.message


def test_load_rejects_bad_device() -> None:
    w = SttWorker()
    resp = w.handle(Request(id="5", cmd="load", params={"model": "small", "device": "tpu"}))
    assert not resp.ok
    assert resp.error is not None
    assert resp.error.code == "bad_args"


def test_unload_when_empty_still_ok() -> None:
    w = SttWorker()
    resp = w.handle(Request(id="6", cmd="unload"))
    assert resp.ok
    status = w.handle(Request(id="7", cmd="status"))
    assert status.result["loaded"] is False


def test_unknown_cmd() -> None:
    w = SttWorker()
    resp = w.handle(Request(id="8", cmd="dance"))
    assert not resp.ok
    assert resp.error is not None
    assert resp.error.code == "bad_args"


def test_transcribe_missing_path() -> None:
    w = SttWorker()
    # Force loaded-looking state without real model for path validation order:
    # not_loaded is checked first — so we only get missing path when model set.
    w.state.model = object()
    w.state.model_name = "small"
    w.state.device = "cpu"
    resp = w.handle(Request(id="9", cmd="transcribe", params={}))
    assert not resp.ok
    assert resp.error is not None
    assert resp.error.code == "bad_args"
    assert "path" in resp.error.message


def test_transcribe_missing_file(tmp_path: Path) -> None:
    w = SttWorker()
    w.state.model = object()
    w.state.model_name = "small"
    w.state.device = "cpu"
    missing = tmp_path / "ghost.wav"
    resp = w.handle(
        Request(id="10", cmd="transcribe", params={"path": str(missing), "language": "en"})
    )
    assert not resp.ok
    assert resp.error is not None
    assert resp.error.code == "bad_args"
    assert "not found" in resp.error.message


def test_transcribe_bad_language() -> None:
    w = SttWorker()
    w.state.model = object()
    w.state.model_name = "small"
    w.state.device = "cpu"
    resp = w.handle(
        Request(
            id="11",
            cmd="transcribe",
            params={"path": "/etc/hosts", "language": "de"},
        )
    )
    # /etc/hosts exists so language is checked
    assert not resp.ok
    assert resp.error is not None
    assert resp.error.code == "bad_args"
    assert "language" in resp.error.message


def test_shutdown_marks_success() -> None:
    w = SttWorker()
    resp = w.handle(Request(id="12", cmd="shutdown"))
    assert resp.ok
    assert resp.result.get("shutdown") is True
