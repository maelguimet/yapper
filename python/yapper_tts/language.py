"""Small deterministic EN/FR detector for Chatterbox's ``language_id``."""

from __future__ import annotations

import re

_WORD = re.compile(r"[^\W\d_]+", re.UNICODE)
_FRENCH_MARKS = frozenset("àâæçéèêëîïôœùûüÿ")
_FRENCH_WORDS = frozenset(
    {
        "au",
        "aux",
        "avec",
        "bonjour",
        "ça",
        "ce",
        "ces",
        "cette",
        "ceci",
        "comme",
        "dans",
        "de",
        "des",
        "du",
        "elle",
        "elles",
        "en",
        "est",
        "et",
        "français",
        "il",
        "ils",
        "je",
        "la",
        "le",
        "les",
        "mais",
        "nous",
        "où",
        "pas",
        "pour",
        "que",
        "qui",
        "sur",
        "très",
        "tu",
        "un",
        "une",
        "vous",
    }
)
_ENGLISH_WORDS = frozenset(
    {
        "a",
        "and",
        "are",
        "as",
        "at",
        "be",
        "for",
        "from",
        "hello",
        "in",
        "is",
        "it",
        "of",
        "on",
        "that",
        "the",
        "this",
        "to",
        "we",
        "with",
        "you",
    }
)

# Upstream suggests zero for cross-language prompts. With Chatterbox 0.1.7 and
# Yapper's short chunks, zero can become unintelligible; a CUDA A/B found 0.2
# retained French language confidence while reducing English prompt guidance.
_CROSS_LANGUAGE_CFG_MAX = 0.2


def resolve_language(requested: str, text: str) -> str:
    """Return an explicit Chatterbox language id (``en`` or ``fr``)."""
    if requested in {"en", "fr"}:
        return requested
    if requested != "auto":
        raise ValueError(f"unsupported language: {requested!r}")

    lowered = text.casefold()
    words = _WORD.findall(lowered)
    french_score = sum(1 for word in words if word in _FRENCH_WORDS)
    english_score = sum(1 for word in words if word in _ENGLISH_WORDS)
    french_score += 2 * sum(1 for char in lowered if char in _FRENCH_MARKS)
    return "fr" if french_score > english_score else "en"


def effective_cfg_weight(
    language: str, reference_language: str | None, configured: float
) -> float:
    """Limit accent transfer when a generic English prompt is used for French."""
    if language == "fr" and reference_language != "fr":
        return min(configured, _CROSS_LANGUAGE_CFG_MAX)
    return configured


def retry_cfg_weight(
    language: str, reference_language: str | None, effective: float
) -> float:
    """Keep the accent-safe cap on retries; otherwise use the stability range."""
    if language == "fr" and reference_language != "fr":
        return effective
    return min(max(effective, 0.3), 0.5)
