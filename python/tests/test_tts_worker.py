"""TTS worker handlers without loading the full model."""

from __future__ import annotations

import time
import wave
from pathlib import Path

import numpy as np

from yapper_common.ipc import Request
from yapper_tts.worker import (
    TRAILING_PAD_MS,
    TtsWorker,
    _output_sane,
    _to_float_array,
    _write_wav,
    min_sane_duration_secs,
    trailing_pad_samples,
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


def _tone(n: int, amp: float = 0.2) -> np.ndarray:
    return (np.sin(np.linspace(0, 100, n)).astype(np.float32) * amp)


def test_output_sane_rejects_empty_and_silent() -> None:
    silent = np.zeros(24000, dtype=np.float32)
    assert not _output_sane("hello world", 0.0, silent)
    assert not _output_sane("hello world", 1.0, silent)
    tone = _tone(24000)
    assert _output_sane("hi", 1.0, tone)


def test_output_sane_rejects_near_zero_peak() -> None:
    almost_silent = np.full(24000, 1e-5, dtype=np.float32)
    assert not _output_sane("hello world", 1.0, almost_silent)


def test_output_sane_long_text_rejects_tiny_clip() -> None:
    """~200-char prose must not pass with a near-instant non-silent WAV."""
    text = _long_prose()
    assert len(text) >= 200
    min_needed = min_sane_duration_secs(text)
    assert min_needed >= 3.0  # word/char floors must be well above "instant"
    sr = 24000
    tiny_secs = 0.25
    tiny = _tone(int(sr * tiny_secs))
    assert not _output_sane(text, tiny_secs, tiny)
    # Still clearly truncated relative to text length, but above old loose floors.
    short_secs = min(1.5, min_needed * 0.4)
    short = _tone(int(sr * short_secs))
    assert short_secs < min_needed
    assert not _output_sane(text, short_secs, short)


def test_output_sane_short_hi_passes() -> None:
    tone = _tone(24000)  # 1s at 24 kHz
    assert _output_sane("hi", 1.0, tone)


def test_write_wav_nan_safe(tmp_path: Path) -> None:
    arr = np.array([0.0, float("nan"), 0.5, float("inf"), -0.25], dtype=np.float32)
    out = tmp_path / "n.wav"
    duration = _write_wav(out, arr, 24000)
    assert out.is_file()
    assert out.stat().st_size > 44
    cleaned = _to_float_array(arr)
    assert not np.isnan(cleaned).any()
    assert not np.isinf(cleaned).any()
    # Finite samples + pad; PCM must be readable and finite.
    with wave.open(str(out), "rb") as wf:
        frames = wf.readframes(wf.getnframes())
    pcm = np.frombuffer(frames, dtype=np.int16).astype(np.float32)
    assert pcm.size > 0
    assert np.isfinite(pcm).all()
    assert duration > 0.0


def test_write_wav_trailing_pad_increases_duration(tmp_path: Path) -> None:
    sr = 24000
    n = sr  # 1.0s raw
    tone = _tone(n)
    out = tmp_path / "pad.wav"
    written_secs = _write_wav(out, tone, sr)
    pad_n = trailing_pad_samples(sr)
    assert pad_n == int(sr * TRAILING_PAD_MS / 1000.0)
    assert pad_n > 0
    expected = (n + pad_n) / float(sr)
    assert abs(written_secs - expected) < 1e-6
    with wave.open(str(out), "rb") as wf:
        assert wf.getnframes() == n + pad_n
        assert wf.getframerate() == sr
        data = np.frombuffer(wf.readframes(wf.getnframes()), dtype=np.int16)
    # Tail is silence (pad), head is not.
    assert int(np.max(np.abs(data[-pad_n:]))) == 0
    assert int(np.max(np.abs(data[:n]))) > 0


def test_write_wav_skips_pad_when_near_silent(tmp_path: Path) -> None:
    sr = 24000
    silent = np.zeros(sr, dtype=np.float32)
    out = tmp_path / "silent.wav"
    written_secs = _write_wav(out, silent, sr)
    with wave.open(str(out), "rb") as wf:
        assert wf.getnframes() == sr  # no pad
    assert abs(written_secs - 1.0) < 1e-6


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


class _ScriptedModel:
    """Fake Chatterbox-like model: returns queued wav arrays from generate()."""

    def __init__(self, waves: list[np.ndarray], delay_s: float = 0.0) -> None:
        self._waves = list(waves)
        self.sr = 24000
        self.calls = 0
        self.delay_s = delay_s

    def generate(self, text: str, **kwargs: object) -> np.ndarray:
        self.calls += 1
        if self.delay_s > 0:
            time.sleep(self.delay_s)
        if not self._waves:
            raise RuntimeError("no more scripted waves")
        return self._waves.pop(0)


def _long_prose() -> str:
    return (
        "This is a longer paragraph meant to exercise duration bounds for "
        "obvious truncation. Two hundred characters of normal prose should "
        "never accept a tiny near-instant clip as valid speech audio output. "
        "Done."
    )


def test_synthesize_retry_updates_gen_ms_and_duration(
    tmp_path: Path, monkeypatch
) -> None:
    """Sanity fail then ok: one retry; metadata reflects total work and padded wav."""
    # Isolate from host Eve assets — worker only needs a stub ref wav path.
    (tmp_path / "eve_neutral.wav").write_bytes(b"RIFF....WAVE")
    monkeypatch.setenv("YAPPER_VOICES_DIR", str(tmp_path))
    monkeypatch.delenv("YAPPER_TTS_CLONE", raising=False)
    text = _long_prose()
    sr = 24000
    bad = _tone(int(sr * 0.2))  # too short for long text
    good_secs = max(min_sane_duration_secs(text) + 0.5, 6.0)
    good = _tone(int(sr * good_secs))
    model = _ScriptedModel([bad, good], delay_s=0.02)
    w = TtsWorker(voices_root=tmp_path)
    w.state.model = model
    w.state.model_name = "chatterbox-multilingual"
    w.state.sample_rate = sr
    out = tmp_path / "retry.wav"
    resp = w.handle(
        Request(
            id="9",
            cmd="synthesize",
            params={
                "text": text,
                "language": "en",
                "tone": "neutral",
                "out_path": str(out),
            },
        )
    )
    assert resp.ok, getattr(resp.error, "message", None)
    assert model.calls == 2
    assert out.is_file()
    gen_ms = float(resp.result["gen_ms"])
    # Two generate calls each slept 20ms → well above single-attempt floor.
    assert gen_ms >= 30.0
    pad_n = trailing_pad_samples(sr)
    expected_dur = good_secs + (pad_n / float(sr))
    assert abs(float(resp.result["duration_secs"]) - expected_dur) < 0.02
    with wave.open(str(out), "rb") as wf:
        assert wf.getnframes() == int(sr * good_secs) + pad_n


def test_synthesize_double_bad_output_skips_with_code(
    tmp_path: Path, monkeypatch
) -> None:
    """Two insane outputs → bad_output so host can skip the segment."""
    (tmp_path / "eve_neutral.wav").write_bytes(b"RIFF....WAVE")
    monkeypatch.setenv("YAPPER_VOICES_DIR", str(tmp_path))
    monkeypatch.delenv("YAPPER_TTS_CLONE", raising=False)
    text = _long_prose()
    sr = 24000
    bad = _tone(int(sr * 0.2))
    model = _ScriptedModel([bad, bad])
    w = TtsWorker(voices_root=tmp_path)
    w.state.model = model
    w.state.model_name = "chatterbox-multilingual"
    w.state.sample_rate = sr
    out = tmp_path / "fail.wav"
    resp = w.handle(
        Request(
            id="10",
            cmd="synthesize",
            params={
                "text": text,
                "language": "en",
                "tone": "neutral",
                "out_path": str(out),
            },
        )
    )
    assert not resp.ok
    assert resp.error is not None
    assert resp.error.code == "bad_output"
    assert "sanity" in resp.error.message.lower() or "duration" in resp.error.message.lower()
    assert model.calls == 2
    assert not out.exists()
