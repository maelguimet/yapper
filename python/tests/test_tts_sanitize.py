"""B21/B25 sanitize fixtures for TTS text prep."""

from yapper_tts.sanitize import (
    B21_SOCIAL_FIXTURE,
    B25_TTS_FIXTURE,
    sanitize_for_tts,
)


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
