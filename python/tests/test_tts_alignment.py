"""Regression coverage for Chatterbox alignment-tail trimming."""

from __future__ import annotations

import wave
from pathlib import Path
from types import SimpleNamespace

import numpy as np
import pytest

from yapper_common.ipc import Request
from yapper_tts.alignment import trim_alignment_tail
from yapper_tts.worker import TtsWorker, trailing_pad_samples


def _tone(samples: int) -> np.ndarray:
    return np.sin(np.linspace(0, 200, samples, dtype=np.float32)) * 0.2


def test_trim_alignment_tail_removes_playback_confirmed_overrun() -> None:
    sample_rate = 24_000
    wav = _tone(round(34.42 * sample_rate))

    cleaned = trim_alignment_tail(
        wav,
        sample_rate,
        completed_at=797,
        total_frames=862,
    )

    samples_per_frame = wav.size / 862
    assert cleaned.size == round((797 + 3) * samples_per_frame)
    assert wav.size - cleaned.size > 2 * sample_rate
    assert cleaned[-1] == pytest.approx(0.0)
    assert np.max(np.abs(cleaned[-480:])) <= 0.2


def test_trim_alignment_tail_keeps_normal_release() -> None:
    sample_rate = 24_000
    wav = _tone(10 * sample_rate)

    cleaned = trim_alignment_tail(
        wav,
        sample_rate,
        completed_at=240,
        total_frames=250,
    )

    assert np.array_equal(cleaned, wav)


@pytest.mark.parametrize(
    ("sample_rate", "completed_at", "total_frames"),
    [
        (0, 10, 20),
        (24_000, 0, 20),
        (24_000, 20, 0),
        (24_000, 20, 20),
        (24_000, 21, 20),
    ],
)
def test_trim_alignment_tail_ignores_invalid_metadata(
    sample_rate: int,
    completed_at: int,
    total_frames: int,
) -> None:
    wav = _tone(24_000)

    cleaned = trim_alignment_tail(
        wav,
        sample_rate,
        completed_at=completed_at,
        total_frames=total_frames,
    )

    assert np.array_equal(cleaned, wav)


class _AlignmentModel:
    sr = 24_000

    def __init__(self, wav: np.ndarray, *, completed_at: int, frames: int) -> None:
        self.wav = wav
        analyzer = SimpleNamespace(
            complete=True,
            completed_at=completed_at,
            curr_frame_pos=frames,
        )
        self.t3 = SimpleNamespace(
            patched_model=SimpleNamespace(alignment_stream_analyzer=analyzer)
        )

    def generate(self, text: str, **kwargs: object) -> np.ndarray:
        return self.wav


class _RetryAlignmentModel:
    sr = 24_000

    def __init__(self, waves: list[np.ndarray]) -> None:
        self.waves = list(waves)
        self.calls = 0
        self.analyzer = SimpleNamespace(
            complete=False,
            completed_at=None,
            curr_frame_pos=None,
        )
        self.t3 = SimpleNamespace(
            patched_model=SimpleNamespace(
                alignment_stream_analyzer=self.analyzer
            )
        )

    def generate(self, text: str, **kwargs: object) -> np.ndarray:
        wav = self.waves[self.calls]
        self.calls += 1
        if self.calls == 2:
            self.analyzer.complete = True
            self.analyzer.completed_at = 797
            self.analyzer.curr_frame_pos = 862
        return wav


def test_worker_trims_alignment_tail_before_silence_pad(tmp_path: Path) -> None:
    (tmp_path / "eve_neutral.wav").write_bytes(b"RIFF....WAVE")
    sample_rate = 24_000
    wav = _tone(round(34.42 * sample_rate))
    model = _AlignmentModel(wav, completed_at=797, frames=862)
    worker = TtsWorker(voices_root=tmp_path)
    worker.state.model = model
    worker.state.model_name = "chatterbox-multilingual"
    worker.state.sample_rate = sample_rate
    output = tmp_path / "alignment-trimmed.wav"

    response = worker.handle(
        Request(
            id="alignment-tail",
            cmd="synthesize",
            params={
                "text": (
                    "This sentence is long enough for the generated duration "
                    "to pass the existing sanity bounds."
                ),
                "language": "en",
                "tone": "neutral",
                "voice": "eve",
                "out_path": str(output),
            },
        )
    )

    assert response.ok, getattr(response.error, "message", None)
    expected_audio_frames = round((797 + 3) * (wav.size / 862))
    expected_file_frames = expected_audio_frames + trailing_pad_samples(sample_rate)
    with wave.open(str(output), "rb") as wav_file:
        assert wav_file.getnchannels() == 1
        assert wav_file.getsampwidth() == 2
        assert wav_file.getframerate() == sample_rate
        assert wav_file.getnframes() == expected_file_frames
        pcm = np.frombuffer(
            wav_file.readframes(wav_file.getnframes()),
            dtype=np.int16,
        )
    assert np.max(np.abs(pcm[-trailing_pad_samples(sample_rate) :])) == 0
    assert float(response.result["duration_secs"]) == pytest.approx(
        expected_file_frames / sample_rate
    )


def test_worker_uses_alignment_from_accepted_retry(tmp_path: Path) -> None:
    (tmp_path / "eve_neutral.wav").write_bytes(b"RIFF....WAVE")
    sample_rate = 24_000
    bad = _tone(round(0.2 * sample_rate))
    accepted = _tone(round(34.42 * sample_rate))
    model = _RetryAlignmentModel([bad, accepted])
    worker = TtsWorker(voices_root=tmp_path)
    worker.state.model = model
    worker.state.model_name = "chatterbox-multilingual"
    worker.state.sample_rate = sample_rate
    output = tmp_path / "alignment-retry.wav"

    response = worker.handle(
        Request(
            id="alignment-retry",
            cmd="synthesize",
            params={
                "text": (
                    "This sentence is long enough for the generated duration "
                    "to pass the existing sanity bounds."
                ),
                "language": "en",
                "tone": "neutral",
                "voice": "eve",
                "out_path": str(output),
            },
        )
    )

    assert response.ok, getattr(response.error, "message", None)
    assert model.calls == 2
    expected_audio_frames = round((797 + 3) * (accepted.size / 862))
    with wave.open(str(output), "rb") as wav_file:
        assert wav_file.getnframes() == (
            expected_audio_frames + trailing_pad_samples(sample_rate)
        )
