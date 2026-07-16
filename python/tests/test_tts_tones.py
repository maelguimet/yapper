"""Tone map / knobs load — pure tests + optional host asset check."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from yapper_tts.tones import (
    DEFAULT_TONES,
    list_tone_names,
    load_knobs,
    neutral_voice_present,
    resolve_tone,
)


def test_list_tones_non_empty() -> None:
    tones = list_tone_names()
    assert len(tones) >= 5
    assert "neutral" in tones


def test_resolve_neutral_from_temp_voices(tmp_path: Path) -> None:
    """Pure: resolve_tone works when a fake WAV exists under voices_root."""
    wav = tmp_path / "eve_neutral.wav"
    wav.write_bytes(b"RIFF....WAVE")
    knobs = {"neutral": {"exg": 0.4, "cfg": 0.5, "rate": 1.0}}
    (tmp_path / "knobs.json").write_text(json.dumps(knobs), encoding="utf-8")
    tone = resolve_tone("neutral", voices_root=tmp_path)
    assert tone.name == "neutral"
    assert tone.ref_wav.is_file()
    assert 0.0 < tone.exaggeration <= 1.5
    assert 0.0 < tone.cfg_weight <= 1.5


@pytest.mark.integration
def test_resolve_neutral_has_wav_and_knobs() -> None:
    """Host asset check: real Eve neutral under app voices dir."""
    try:
        tone = resolve_tone("neutral")
    except FileNotFoundError as exc:
        pytest.skip(f"Eve voice assets missing: {exc}")
    if not tone.ref_wav.is_file():
        pytest.skip(f"missing ref wav: {tone.ref_wav}")
    assert tone.name == "neutral"
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


def test_neutral_voice_present_false_when_missing(tmp_path: Path) -> None:
    assert not neutral_voice_present(tmp_path)
    (tmp_path / "eve_calm.wav").write_bytes(b"RIFF")
    assert not neutral_voice_present(tmp_path)


def test_neutral_voice_present_true_with_file(tmp_path: Path) -> None:
    (tmp_path / "eve_neutral.wav").write_bytes(b"RIFF....WAVE")
    assert neutral_voice_present(tmp_path)


def test_french_reference_is_preferred_and_language_not_listed_as_tone(
    tmp_path: Path,
) -> None:
    generic = tmp_path / "eve_neutral.wav"
    french = tmp_path / "eve_fr_neutral.wav"
    generic.write_bytes(b"generic")
    french.write_bytes(b"french")

    tone = resolve_tone("neutral", voices_root=tmp_path, voice="eve", language="fr")

    assert tone.ref_wav == french
    assert tone.reference_language == "fr"
    assert list_tone_names(tmp_path, voice="eve") == ["neutral"]


def test_french_reference_falls_back_to_generic(tmp_path: Path) -> None:
    generic = tmp_path / "default_neutral.wav"
    generic.write_bytes(b"generic")

    tone = resolve_tone("neutral", voices_root=tmp_path, language="fr")

    assert tone.ref_wav == generic
    assert tone.reference_language is None
