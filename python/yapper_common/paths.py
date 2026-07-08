"""XDG-style paths for yapper data and models."""

from __future__ import annotations

import os
from pathlib import Path


def home() -> Path:
    return Path.home()


def data_dir() -> Path:
    override = os.environ.get("YAPPER_DATA_DIR")
    if override:
        return Path(override).expanduser().resolve()
    xdg = os.environ.get("XDG_DATA_HOME")
    base = Path(xdg) if xdg else home() / ".local" / "share"
    return (base / "yapper").resolve()


def config_dir() -> Path:
    override = os.environ.get("YAPPER_CONFIG_DIR")
    if override:
        return Path(override).expanduser().resolve()
    xdg = os.environ.get("XDG_CONFIG_HOME")
    base = Path(xdg) if xdg else home() / ".config"
    return (base / "yapper").resolve()


def models_dir() -> Path:
    return data_dir() / "models"


def whisper_models_dir() -> Path:
    return models_dir() / "whisper"


def voices_dir() -> Path:
    return data_dir() / "voices"


def logs_dir() -> Path:
    return data_dir() / "logs"


def ensure_runtime_dirs() -> None:
    for path in (models_dir(), whisper_models_dir(), voices_dir(), logs_dir(), config_dir()):
        path.mkdir(parents=True, exist_ok=True)
