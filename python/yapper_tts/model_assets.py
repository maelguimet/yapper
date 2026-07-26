"""Pinned, integrity-checked Chatterbox model assets.

The upstream loader follows the mutable Hugging Face ``main`` branch. Yapper
instead downloads one reviewed revision and verifies every file before any
checkpoint deserialization happens.
"""

from __future__ import annotations

import hashlib
import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

CHATTERBOX_REPO_ID = "ResembleAI/chatterbox"
# Multilingual v2: the revision used by chatterbox-tts 0.1.7 before the
# repository moved to v3 weights in April/June 2026.
CHATTERBOX_MODEL_REVISION = "05e904af2b5c7f8e482687a9d7336c5c824467d9"
MIN_SAFE_TORCH_VERSION = (2, 10, 0)


@dataclass(frozen=True)
class ModelAsset:
    size: int
    sha256: str


CHATTERBOX_MODEL_ASSETS: Mapping[str, ModelAsset] = {
    "Cangjie5_TC.json": ModelAsset(
        size=1_920_163,
        sha256="7073fd9de919443ae88e0bd2449917a65fe54898a4413ed1edcc4b67f28bce8c",
    ),
    "conds.pt": ModelAsset(
        size=107_374,
        sha256="6552d70568833628ba019c6b03459e77fe71ca197d5c560cef9411bee9d87f4e",
    ),
    "grapheme_mtl_merged_expanded_v1.json": ModelAsset(
        size=70_011,
        sha256="df81a7ca7c31796cbe97f7a7142d5a53b12e88e12417ebe98f66602cafaf0461",
    ),
    "s3gen.pt": ModelAsset(
        size=1_057_165_844,
        sha256="9b9ff07e60b20c136e2b1b3d7563a24604e8d2c4c267888d1ee929dd0151d2a3",
    ),
    "t3_mtl23ls_v2.safetensors": ModelAsset(
        size=2_143_989_752,
        sha256="b1237586127ce98e7800a68e49938eb5092846862aabcb6e17b2fda7889a6c75",
    ),
    "ve.pt": ModelAsset(
        size=5_698_626,
        sha256="4b16d836bc598509860f6fa068165a8bb5e9ac84f05582dfcf278a5a372879f1",
    ),
}


class ModelIntegrityError(RuntimeError):
    """A pinned model snapshot is absent, changed, or otherwise unsafe."""


def require_safe_torch_version(version: str) -> tuple[int, int, int]:
    """Reject PyTorch releases affected by CVE-2026-24747."""

    match = re.match(r"^(\d+)\.(\d+)(?:\.(\d+))?", version)
    if match is None:
        raise ModelIntegrityError(f"cannot parse PyTorch version: {version!r}")
    parsed = tuple(int(part or 0) for part in match.groups())
    if parsed < MIN_SAFE_TORCH_VERSION:
        required = ".".join(str(part) for part in MIN_SAFE_TORCH_VERSION)
        raise ModelIntegrityError(
            f"PyTorch {version} is unsafe for remote checkpoints; require >= {required}"
        )
    return parsed


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def verify_snapshot(
    snapshot_dir: Path,
    assets: Mapping[str, ModelAsset] = CHATTERBOX_MODEL_ASSETS,
) -> Path:
    """Verify all expected snapshot files and return the resolved directory."""

    root = snapshot_dir.expanduser().resolve(strict=True)
    if not root.is_dir():
        raise ModelIntegrityError(f"model snapshot is not a directory: {root}")

    for name, expected in assets.items():
        path = root / name
        if not path.is_file():
            raise ModelIntegrityError(f"model asset is missing: {name}")
        actual_size = path.stat().st_size
        if actual_size != expected.size:
            raise ModelIntegrityError(
                f"model asset size mismatch for {name}: "
                f"expected {expected.size}, got {actual_size}"
            )
        actual_hash = _sha256(path)
        if actual_hash != expected.sha256:
            raise ModelIntegrityError(
                f"model asset SHA-256 mismatch for {name}: "
                f"expected {expected.sha256}, got {actual_hash}"
            )
    return root


def download_verified_snapshot() -> Path:
    """Download the immutable Chatterbox snapshot and verify it locally."""

    from huggingface_hub import snapshot_download

    snapshot = snapshot_download(
        repo_id=CHATTERBOX_REPO_ID,
        repo_type="model",
        revision=CHATTERBOX_MODEL_REVISION,
        allow_patterns=list(CHATTERBOX_MODEL_ASSETS),
        token=os.getenv("HF_TOKEN"),
    )
    return verify_snapshot(Path(snapshot))
