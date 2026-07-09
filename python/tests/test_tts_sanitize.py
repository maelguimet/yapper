"""B21/B25 sanitize fixtures for TTS text prep (aligned with Rust textprep)."""

from pathlib import Path

from yapper_tts.sanitize import (
    B21_SOCIAL_FIXTURE,
    B25_TTS_FIXTURE,
    FRENCH_EU_FIXTURE,
    sanitize_for_tts,
)

# Shared golden fixtures under repo fixtures/ (mirrored cases with Rust).
_FIXTURES = Path(__file__).resolve().parents[2] / "fixtures" / "sanitize"


def test_b21_social_speakable_no_handles_or_emoji():
    out = sanitize_for_tts(B21_SOCIAL_FIXTURE)
    assert out
    assert "@" not in out
    assert "🫡" not in out
    assert "doesn't work" in out.lower() or "doesn" in out.lower()
    assert "G P T" in out or "g p t" in out.lower()


def test_b25_tts_spelled_out():
    out = sanitize_for_tts(B25_TTS_FIXTURE)
    assert "T T S" in out
    assert not any(w.lower() == "tts" for w in out.split())
    assert "river" not in out.lower()


def test_empty_safe():
    assert sanitize_for_tts("") == ""
    assert sanitize_for_tts("  \n\n  ") == ""


def test_handle_only_line_dropped():
    assert sanitize_for_tts("@illyism\nHello") == "Hello"


def test_french_lowercase_eu_not_expanded():
    out = sanitize_for_tts(FRENCH_EU_FIXTURE)
    assert out == "j'ai eu peur."
    assert "E U" not in out


def test_uppercase_eu_expanded():
    out = sanitize_for_tts("EU rules")
    assert "E U" in out
    assert "EU" not in out.split()


def test_lowercase_tts_still_expanded():
    out = sanitize_for_tts("try tts please")
    assert "T T S" in out


def test_golden_fixture_files_when_present():
    """If shared golden files exist, Python must match expected outputs."""
    cases = _FIXTURES / "cases.txt"
    expected = _FIXTURES / "expected_python.txt"
    if not cases.is_file() or not expected.is_file():
        return
    inputs = cases.read_text(encoding="utf-8").split("\n---\n")
    wants = expected.read_text(encoding="utf-8").split("\n---\n")
    assert len(inputs) == len(wants), "fixture input/output count mismatch"
    for raw, want in zip(inputs, wants, strict=True):
        assert sanitize_for_tts(raw) == want.strip()
