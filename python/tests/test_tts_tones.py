"""Tone map / knobs load from real gold bank when present."""

from __future__ import annotations

import json
from pathlib import Path

from yapper_tts.tones import (
    DEFAULT_TONES,
    list_tone_names,
    load_knobs,
    resolve_tone,
)


def test_list_tones_non_empty() -> None:
    tones = list_tone_names()
    assert len(tones) >= 5
    assert "neutral" in tones


def test_resolve_neutral_has_wav_and_knobs() -> None:
    tone = resolve_tone("neutral")
    assert tone.name == "neutral"
    assert tone.ref_wav.is_file()
    assert 0.0 < tone.exaggeration <= 1.5
    assert 0.0 < tone.cfg_weight <= 1.5


def test_load_knobs_merges_file(tmp_path: Path) -> None:
    (tmp_path / "eve_neutral.wav").write_bytes(b"RIFF")
    knobs = {"neutral": {"exg": 0.11, "cfg": 0.22, "rate": 0.9}}
    (tmp_path / "knobs.json").write_text(json.dumps(knobs), encoding="utf-8")
    loaded = load_knobs(tmp_path)
    assert loaded["neutral"]["exg"] == 0.11
    assert loaded["neutral"]["cfg"] == 0.22
    # defaults still present for other tones
    assert "calm" in loaded


def test_unknown_tone_raises() -> None:
    try:
        resolve_tone("not_a_real_tone_xyz")
        assert False, "expected KeyError"
    except KeyError as exc:
        assert "unknown tone" in str(exc)


def test_default_tones_cover_gold_set() -> None:
    assert "excited" in DEFAULT_TONES
    assert "whisper" in DEFAULT_TONES
