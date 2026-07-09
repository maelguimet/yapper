"""Sanitize free-form text before Chatterbox synthesis (B21/B25).

Must stay aligned with Rust `src/textprep.rs` via shared golden fixtures.
"""

from __future__ import annotations

import re
import unicodedata

# B21 social/X fixture (must not crash pipeline).
B21_SOCIAL_FIXTURE = """ILIAS ISM
@illyism
This doesn't work in EU btw 🫡

Neither the GPT-Live from today

The permanent EUnderclass is already here 🇪🇺"""

B25_TTS_FIXTURE = "This is a test. Does the TTS work?"

# French lowercase fixture — `eu` must not expand.
FRENCH_EU_FIXTURE = "j'ai eu peur."

_HANDLE_ONLY = re.compile(r"^@\w+$")
_HANDLE_INLINE = re.compile(r"@\w+")
# Case-aware: TTS/STT/GPT any case; EU uppercase-only (French "eu" is a word).
_ACRONYMS = (
    (re.compile(r"\bTTS\b", re.IGNORECASE), "T T S"),
    (re.compile(r"\bSTT\b", re.IGNORECASE), "S T T"),
    (re.compile(r"\bGPT\b", re.IGNORECASE), "G P T"),
    (re.compile(r"\bEU\b"), "E U"),  # no IGNORECASE — preserve French "eu"
)
_MULTI_WS = re.compile(r"\s+")


def sanitize_for_tts(text: str) -> str:
    """Normalize messy paste for TTS. Never raises on ordinary Unicode input."""
    if not text or not str(text).strip():
        return ""
    parts: list[str] = []
    for raw in str(text).splitlines():
        line = _sanitize_line(raw)
        if line:
            parts.append(line)
    joined = " ".join(parts)
    return _MULTI_WS.sub(" ", joined).strip()


def _sanitize_line(line: str) -> str:
    trimmed = line.strip()
    if not trimmed:
        return ""
    if _HANDLE_ONLY.match(trimmed):
        return ""
    # Drop @handles inline
    s = _HANDLE_INLINE.sub("", trimmed)
    s = "".join(ch for ch in s if _keep_char(ch))
    s = _MULTI_WS.sub(" ", s).strip()
    for pattern, repl in _ACRONYMS:
        s = pattern.sub(repl, s)
    return s


def _keep_char(ch: str) -> bool:
    if ch.isascii():
        return not ch.isspace() or ch == " " or ch == "\t"
    # Keep letters/numbers (FR accents); drop emoji/symbols broadly.
    cat = unicodedata.category(ch)
    if cat.startswith("L") or cat.startswith("N"):
        return True
    if ch in "’‘“”–—…«»€£":
        return True
    # Drop So/Sk/Cf emoji-ish and marks used only for emoji composition.
    if cat in {"So", "Sk", "Cf", "Cs", "Co"}:
        return False
    if cat.startswith("M"):
        # Keep combining accents on Latin letters
        return True
    return cat.startswith("P")  # punctuation


__all__ = [
    "B21_SOCIAL_FIXTURE",
    "B25_TTS_FIXTURE",
    "FRENCH_EU_FIXTURE",
    "sanitize_for_tts",
]
