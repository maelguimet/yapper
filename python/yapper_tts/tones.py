"""Eve tone list + knobs from installed voices dir or tts/clone sources."""

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


def resolve_voices_root() -> Path:
    """Prefer installed voices; fall back to clone gold for dev."""
    installed = voices_dir()
    if (installed / "knobs.json").is_file() or any(installed.glob("eve_*.wav")):
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


def list_tone_names(voices_root: Path | None = None) -> list[str]:
    root = voices_root or resolve_voices_root()
    found = sorted(
        p.stem.removeprefix("eve_")
        for p in root.glob("eve_*.wav")
        if p.is_file()
    )
    if found:
        return found
    return list(DEFAULT_TONES)


def neutral_voice_present(voices_root: Path | None = None) -> bool:
    """True when eve_neutral.wav exists under the resolved voices root."""
    root = voices_root if voices_root is not None else resolve_voices_root()
    return (root / "eve_neutral.wav").is_file()


def resolve_tone(name: str, voices_root: Path | None = None, voice: str = "eve") -> ToneSpec:
    root = voices_root or resolve_voices_root()
    tone = (name or "neutral").strip().lower()
    knobs = load_knobs(root)
    if tone not in knobs and tone not in DEFAULT_TONES:
        raise KeyError(f"unknown tone: {tone!r}")
    ref = root / f"{voice}_{tone}.wav"
    if not ref.is_file():
        # try gold path under clone when voices root is gold already handled;
        # also try prompts/
        clone = clone_source_dir()
        if clone is not None:
            for sub in ("gold", "prompts"):
                alt = clone / sub / f"{voice}_{tone}.wav"
                if alt.is_file():
                    ref = alt
                    break
    if not ref.is_file():
        raise FileNotFoundError(f"missing reference wav for tone {tone}: {ref}")
    k = knobs.get(tone, DEFAULT_KNOBS.get(tone, {"exg": 0.5, "cfg": 0.5, "rate": 1.0}))
    return ToneSpec(
        name=tone,
        ref_wav=ref,
        exaggeration=float(k.get("exg", 0.5)),
        cfg_weight=float(k.get("cfg", 0.5)),
        rate=float(k.get("rate", 1.0)),
    )
