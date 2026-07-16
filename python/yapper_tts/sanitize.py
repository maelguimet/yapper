"""Sanitize free-form text before Chatterbox synthesis (B21/B25).

Must stay aligned with Rust `src/textprep.rs` via shared golden fixtures.

Rules:
- Replace http(s)://… and www.… with the short speakable placeholder ``link``.
- Drop standalone ``#hashtag`` tokens and ``@handle`` lines/tokens (prose kept).
- Cap unbroken tokens at ``MAX_UNBROKEN_TOKEN_CHARS`` by inserting spaces.
- Expand common initialisms such as AI/RLHF/TTS into spoken letters.
- Turn parenthetical delimiters into comma pauses while keeping their text.
- Keep FR letters/accents; strip emoji and other unsupported glyphs.
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

# Max Unicode code points per unbroken token (aligned with Rust).
MAX_UNBROKEN_TOKEN_CHARS = 80
URL_PLACEHOLDER = "link"

_HANDLE_ONLY = re.compile(r"^@\w+$")
_HANDLE_INLINE = re.compile(r"@\w+")
_HASHTAG_TOKEN = re.compile(r"^#\w+$")
_URL_TRAIL_PUNCT = frozenset(".,;:!?)]'\"»")
# Explicit list: do not spell every uppercase word (for example, names such as ILIAS).
_ACRONYMS = (
    (re.compile(r"\bRLHF\b"), "R L H F"),
    (re.compile(r"\bVRAM\b"), "V R A M"),
    (re.compile(r"\bTTS\b", re.IGNORECASE), "T T S"),
    (re.compile(r"\bSTT\b", re.IGNORECASE), "S T T"),
    (re.compile(r"\bGPT\b", re.IGNORECASE), "G P T"),
    (re.compile(r"\bLLM\b"), "L L M"),
    (re.compile(r"\bNLP\b"), "N L P"),
    (re.compile(r"\bAPI\b"), "A P I"),
    (re.compile(r"\bCPU\b"), "C P U"),
    (re.compile(r"\bGPU\b"), "G P U"),
    (re.compile(r"\bRAM\b"), "R A M"),
    (re.compile(r"\bAI\b"), "A I"),
    (re.compile(r"\bML\b"), "M L"),
    (re.compile(r"\bUI\b"), "U I"),
    (re.compile(r"\bUX\b"), "U X"),
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
    s = _replace_urls(s)
    s = "".join(ch for ch in s if _keep_char(ch))
    s = _MULTI_WS.sub(" ", s).strip()
    s = _drop_standalone_hashtags(s)
    s = _normalize_parenthetical_pauses(s)
    s = _cap_unbroken_tokens(s, MAX_UNBROKEN_TOKEN_CHARS)
    for pattern, repl in _ACRONYMS:
        s = pattern.sub(repl, s)
    return s


def _normalize_parenthetical_pauses(s: str) -> str:
    """Keep parenthetical prose, replacing delimiters with speakable pauses."""
    chars = list(s)
    out: list[str] = []
    normalized_stack: list[bool] = []
    pause_or_terminal = frozenset(",;:.!?…—–")

    for i, ch in enumerate(chars):
        if ch == "(":
            # Whitespace-delimited parentheses are prose asides. Preserve
            # attached pairs such as main() and println!(...) as code.
            normalize = i == 0 or chars[i - 1].isspace()
            normalized_stack.append(normalize)
            if not normalize:
                out.append(ch)
                continue
            while out and out[-1].isspace():
                out.pop()
            if out and out[-1] not in pause_or_terminal:
                out.append(",")
            if out:
                out.append(" ")
            continue
        if ch == ")":
            normalize = normalized_stack.pop() if normalized_stack else False
            if not normalize:
                out.append(ch)
                continue
            while out and out[-1].isspace():
                out.pop()
            following = next((c for c in chars[i + 1 :] if not c.isspace()), None)
            if following is not None and following not in pause_or_terminal:
                if out and out[-1] not in pause_or_terminal:
                    out.append(",")
            if following is not None and following not in pause_or_terminal and out:
                out.append(" ")
            continue
        out.append(ch)
    return _MULTI_WS.sub(" ", "".join(out)).strip()


def _replace_urls(s: str) -> str:
    """Mirror Rust char-scan: http(s):// and www. → link; leave trailing punct."""
    chars = list(s)
    out: list[str] = []
    i = 0
    n = len(chars)
    while i < n:
        end = _match_url_at(chars, i)
        if end is not None:
            out.append(URL_PLACEHOLDER)
            i = end
            continue
        out.append(chars[i])
        i += 1
    return "".join(out)


def _match_url_at(chars: list[str], i: int) -> int | None:
    scheme_len = 0
    if _starts_with_ci(chars, i, "https://"):
        scheme_len = 8
    elif _starts_with_ci(chars, i, "http://"):
        scheme_len = 7
    elif _starts_with_ci(chars, i, "www."):
        scheme_len = 4
    else:
        return None
    j = i + scheme_len
    if j >= len(chars) or chars[j].isspace():
        return None
    while j < len(chars) and not chars[j].isspace():
        j += 1
    while j > i + scheme_len and chars[j - 1] in _URL_TRAIL_PUNCT:
        j -= 1
    if j <= i + scheme_len:
        return None
    return j


def _starts_with_ci(chars: list[str], i: int, prefix: str) -> bool:
    if i + len(prefix) > len(chars):
        return False
    for k, want in enumerate(prefix):
        got = chars[i + k]
        if got.lower() != want.lower():
            return False
    return True


def _drop_standalone_hashtags(s: str) -> str:
    return " ".join(tok for tok in s.split() if not _HASHTAG_TOKEN.match(tok))


def _cap_unbroken_tokens(s: str, max_len: int) -> str:
    max_len = max(max_len, 8)
    parts: list[str] = []
    for token in s.split():
        chars = list(token)
        if len(chars) <= max_len:
            parts.append(token)
            continue
        for i in range(0, len(chars), max_len):
            parts.append("".join(chars[i : i + max_len]))
    return " ".join(parts)


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
    "MAX_UNBROKEN_TOKEN_CHARS",
    "sanitize_for_tts",
]
