"""Trim anomalous Chatterbox output after text alignment has completed."""

from __future__ import annotations

from typing import TypeAlias

import numpy as np
from numpy.typing import NDArray

FloatArray: TypeAlias = NDArray[np.float32]

_MIN_ALIGNMENT_TAIL_MS = 600
_ALIGNMENT_RELEASE_FRAMES = 3
_ALIGNMENT_FADE_MS = 20


def trim_alignment_tail(
    wav: FloatArray,
    sample_rate: int,
    *,
    completed_at: int,
    total_frames: int,
) -> FloatArray:
    """Remove a long post-completion tail while retaining a natural release."""
    arr: FloatArray = np.asarray(wav, dtype=np.float32).reshape(-1)
    if (
        sample_rate <= 0
        or completed_at <= 0
        or total_frames <= 0
        or completed_at >= total_frames
        or arr.size == 0
    ):
        return arr

    samples_per_frame = arr.size / total_frames
    tail_ms = (
        (total_frames - completed_at)
        * samples_per_frame
        * 1_000.0
        / sample_rate
    )
    if tail_ms < _MIN_ALIGNMENT_TAIL_MS:
        return arr

    release_frame = min(total_frames, completed_at + _ALIGNMENT_RELEASE_FRAMES)
    cut_at = min(arr.size, round(release_frame * samples_per_frame))
    if cut_at <= 0 or cut_at >= arr.size:
        return arr

    cleaned = arr[:cut_at].copy()
    fade_samples = min(
        cleaned.size,
        max(1, round(sample_rate * _ALIGNMENT_FADE_MS / 1_000.0)),
    )
    cleaned[-fade_samples:] *= np.linspace(
        1.0,
        0.0,
        fade_samples,
        dtype=np.float32,
    )
    return cleaned
