"""B21/B25 sanitize fixtures for TTS text prep (aligned with Rust textprep)."""

from pathlib import Path

from yapper_tts.sanitize import (
    B21_SOCIAL_FIXTURE,
    B25_TTS_FIXTURE,
    FRENCH_EU_FIXTURE,
    MAX_UNBROKEN_TOKEN_CHARS,
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


def test_ai_rlhf_and_common_technical_initialisms_are_spelled():
    out = sanitize_for_tts("AI improves RLHF. An API can use a GPU.")
    assert out == "A I improves R L H F. An A P I can use a G P U."


def test_uppercase_names_are_not_spelled():
    assert sanitize_for_tts("ILIAS uses AI") == "ILIAS uses A I"


def test_parentheticals_become_spoken_pauses_without_losing_content():
    assert sanitize_for_tts(
        "AI (artificial intelligence) and RLHF (training)."
    ) == "A I, artificial intelligence, and R L H F, training."
    assert sanitize_for_tts(
        "Use AI (artificial intelligence)."
    ) == "Use A I, artificial intelligence."


def test_urls_become_short_placeholder():
    out = sanitize_for_tts("See https://example.com/path?q=1 now")
    assert "link" in out
    assert "http" not in out.lower()
    assert "example.com" not in out
    out2 = sanitize_for_tts("also WWW.Example.ORG/foo and http://a.co/x end.")
    assert "link" in out2
    assert "http" not in out2.lower()
    assert "www." not in out2.lower()


def test_standalone_hashtags_dropped():
    assert sanitize_for_tts("Love rust #cool #stuff today") == "Love rust today"


def test_long_unbroken_token_capped():
    blob = "q" * 1000
    out = sanitize_for_tts(f"before {blob} after")
    assert out.startswith("before ")
    assert out.endswith(" after")
    for tok in out.split():
        assert len(list(tok)) <= MAX_UNBROKEN_TOKEN_CHARS
    assert out.count("q") == 1000


def test_code_ish_paste():
    raw = 'fn main() { println!("hi"); } https://github.com/foo/bar.git #rust'
    out = sanitize_for_tts(raw)
    assert "main" in out or "fn" in out
    assert "link" in out
    assert "http" not in out
    assert "#rust" not in out
    for tok in out.split():
        assert len(list(tok)) <= MAX_UNBROKEN_TOKEN_CHARS


def test_french_accents_kept():
    out = sanitize_for_tts("Café déjà vu — très bien. ça va?")
    assert "Café" in out
    assert "é" in out
    assert "ç" in out
    assert "Ã" not in out


def test_golden_fixture_files_when_present():
    """Shared golden files: Python must match the same contract as Rust."""
    cases = _FIXTURES / "cases.txt"
    expected = _FIXTURES / "expected.txt"
    if not expected.is_file():
        expected = _FIXTURES / "expected_python.txt"
    if not cases.is_file() or not expected.is_file():
        return
    rust_side = _FIXTURES / "expected_rust.txt"
    py_side = _FIXTURES / "expected_python.txt"
    if rust_side.is_file() and py_side.is_file():
        assert rust_side.read_text(encoding="utf-8") == py_side.read_text(
            encoding="utf-8"
        ), "expected_rust.txt and expected_python.txt must be byte-identical"
    inputs = cases.read_text(encoding="utf-8").split("\n---\n")
    wants = expected.read_text(encoding="utf-8").split("\n---\n")
    assert len(inputs) == len(wants), "fixture input/output count mismatch"
    for raw, want in zip(inputs, wants, strict=True):
        assert sanitize_for_tts(raw) == want.strip(), repr(raw[:80])
