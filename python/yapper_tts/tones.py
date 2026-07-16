"""Tone list + knobs from installed voices dir or optional dev clone sources."""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path

from yapper_common.paths import voices_dir

# Canonical tone set (matches tts/clone gold / emotion_map keys).
DEFAULT_TONES: tuple[str, ...] = (
    "neutral",
    "calm",
    "caring",
    "confused",
    "excited",
    "sad",
    "angry",
    "serious",
    "sensual",
    "teasing",
    "conspiratorial",
    "motivational",
    "romantic",
    "unhinged",
    "whisper",
)

DEFAULT_KNOBS: dict[str, dict[str, float]] = {
    "neutral": {"exg": 0.55, "cfg": 0.5, "rate": 1.12},
    "calm": {"exg": 0.32, "cfg": 0.4, "rate": 1.02},
    "caring": {"exg": 0.4, "cfg": 0.45, "rate": 1.02},
    "confused": {"exg": 0.5, "cfg": 0.5, "rate": 0.96},
    "excited": {"exg": 0.62, "cfg": 0.5, "rate": 0.94},
    "sad": {"exg": 0.45, "cfg": 0.3, "rate": 1.0},
    "angry": {"exg": 0.72, "cfg": 0.3, "rate": 0.99},
    "serious": {"exg": 0.45, "cfg": 0.3, "rate": 0.94},
    "sensual": {"exg": 0.55, "cfg": 0.4, "rate": 0.95},
    "teasing": {"exg": 0.58, "cfg": 0.45, "rate": 1.0},
    "conspiratorial": {"exg": 0.5, "cfg": 0.45, "rate": 0.98},
    "motivational": {"exg": 0.6, "cfg": 0.45, "rate": 1.0},
    "romantic": {"exg": 0.48, "cfg": 0.4, "rate": 0.96},
    "unhinged": {"exg": 0.75, "cfg": 0.35, "rate": 1.05},
    "whisper": {"exg": 0.35, "cfg": 0.4, "rate": 0.9},
}


@dataclass(frozen=True)
class ToneSpec:
    name: str
    ref_wav: Path
    exaggeration: float
    cfg_weight: float
    rate: float
    reference_language: str | None = None


def clone_source_dir() -> Path | None:
    env = os.environ.get("YAPPER_TTS_CLONE")
    if env:
        p = Path(env).expanduser()
        if p.is_dir():
            return p
    candidate = Path.home() / "projects" / "tts" / "clone"
    if candidate.is_dir():
        return candidate
    return None


DEFAULT_VOICE = "default"
REFERENCE_LANGUAGES = frozenset({"en", "fr"})


def _voice_prefix(voice: str) -> str:
    v = (voice or DEFAULT_VOICE).strip().lower()
    return v if v else DEFAULT_VOICE


def resolve_voices_root() -> Path:
    """Prefer installed voices; fall back to clone gold for dev."""
    installed = voices_dir()
    if (installed / "knobs.json").is_file():
        return installed
    for pattern in ("default_*.wav", "eve_*.wav"):
        if any(installed.glob(pattern)):
            return installed
    clone = clone_source_dir()
    if clone is not None:
        gold = clone / "gold"
        if gold.is_dir():
            return gold
    return installed


def load_knobs(voices_root: Path | None = None) -> dict[str, dict[str, float]]:
    root = voices_root or resolve_voices_root()
    knobs_path = root / "knobs.json"
    # knobs often live next to gold in clone/
    if not knobs_path.is_file():
        clone = clone_source_dir()
        if clone is not None and (clone / "knobs.json").is_file():
            knobs_path = clone / "knobs.json"
    merged = {k: dict(v) for k, v in DEFAULT_KNOBS.items()}
    if knobs_path.is_file():
        data = json.loads(knobs_path.read_text(encoding="utf-8"))
        if isinstance(data, dict):
            for name, vals in data.items():
                if not isinstance(vals, dict):
                    continue
                base = merged.get(str(name), {"exg": 0.5, "cfg": 0.5, "rate": 1.0})
                if "exg" in vals:
                    base["exg"] = float(vals["exg"])
                if "cfg" in vals:
                    base["cfg"] = float(vals["cfg"])
                if "rate" in vals:
                    base["rate"] = float(vals["rate"])
                merged[str(name)] = base
    return merged


def list_tone_names(voices_root: Path | None = None, voice: str = DEFAULT_VOICE) -> list[str]:
    root = voices_root or resolve_voices_root()
    prefix = _voice_prefix(voice)
    found = _tone_names_for_prefix(root, prefix)
    if not found:
        found = _tone_names_for_prefix(root, "eve")
    if found:
        return found
    return list(DEFAULT_TONES)


def _tone_names_for_prefix(root: Path, prefix: str) -> list[str]:
    """List tones without exposing the optional language filename component."""
    found: set[str] = set()
    for path in root.glob(f"{prefix}_*.wav"):
        if not path.is_file():
            continue
        suffix = path.stem.removeprefix(f"{prefix}_")
        language, separator, language_tone = suffix.partition("_")
        tone = language_tone if separator and language in REFERENCE_LANGUAGES else suffix
        if tone in DEFAULT_TONES:
            found.add(tone)
    return sorted(found)


def neutral_voice_present(
    voices_root: Path | None = None, voice: str = DEFAULT_VOICE
) -> bool:
    """True when `{voice}_neutral.wav` exists (or legacy eve_neutral.wav)."""
    root = voices_root if voices_root is not None else resolve_voices_root()
    prefix = _voice_prefix(voice)
    if (root / f"{prefix}_neutral.wav").is_file():
        return True
    return (root / "eve_neutral.wav").is_file()


def resolve_tone(
    name: str,
    voices_root: Path | None = None,
    voice: str = DEFAULT_VOICE,
    language: str | None = None,
) -> ToneSpec:
    root = voices_root or resolve_voices_root()
    tone = (name or "neutral").strip().lower()
    prefix = _voice_prefix(voice)
    knobs = load_knobs(root)
    if tone not in knobs and tone not in DEFAULT_TONES:
        raise KeyError(f"unknown tone: {tone!r}")
    requested_language = (language or "").strip().lower() or None
    language_stems: list[tuple[str, str | None]] = []
    if requested_language in REFERENCE_LANGUAGES:
        language_stems.extend(
            (stem, requested_language)
            for stem in _candidate_stems(prefix, tone, requested_language)
        )
        if tone != "neutral":
            language_stems.extend(
                (stem, requested_language)
                for stem in _candidate_stems(prefix, "neutral", requested_language)
            )
    generic_stems: list[tuple[str, str | None]] = [
        (stem, None) for stem in _candidate_stems(prefix, tone)
    ]
    if tone != "neutral":
        generic_stems.extend(
            (stem, None) for stem in _candidate_stems(prefix, "neutral")
        )
    ref, reference_language = _first_reference(root, language_stems + generic_stems)
    if ref is None:
        expected = root / f"{prefix}_{tone}.wav"
        raise FileNotFoundError(f"missing reference wav for tone {tone}: {expected}")
    k = knobs.get(tone, DEFAULT_KNOBS.get(tone, {"exg": 0.5, "cfg": 0.5, "rate": 1.0}))
    return ToneSpec(
        name=tone,
        ref_wav=ref,
        exaggeration=float(k.get("exg", 0.5)),
        cfg_weight=float(k.get("cfg", 0.5)),
        rate=float(k.get("rate", 1.0)),
        reference_language=reference_language,
    )


def _candidate_stems(prefix: str, tone: str, language: str | None = None) -> list[str]:
    middle = f"{language}_" if language else ""
    return list(dict.fromkeys((f"{prefix}_{middle}{tone}", f"eve_{middle}{tone}")))


def _first_reference(
    root: Path, candidates: list[tuple[str, str | None]]
) -> tuple[Path | None, str | None]:
    clone = clone_source_dir()
    for stem, reference_language in candidates:
        installed = root / f"{stem}.wav"
        if installed.is_file():
            return installed, reference_language
        if clone is not None:
            for sub in ("gold", "prompts"):
                private = clone / sub / f"{stem}.wav"
                if private.is_file():
                    return private, reference_language
    return None, None
