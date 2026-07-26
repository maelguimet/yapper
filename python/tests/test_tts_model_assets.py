from __future__ import annotations

import hashlib
import sys
from pathlib import Path
from types import ModuleType

import pytest

import yapper_tts.model_assets as model_assets
from yapper_tts.model_assets import (
    CHATTERBOX_MODEL_ASSETS,
    CHATTERBOX_MODEL_REVISION,
    MIN_SAFE_TORCH_VERSION,
    ModelAsset,
    ModelIntegrityError,
    require_safe_torch_version,
    verify_snapshot,
)


def test_torch_version_floor_rejects_vulnerable_releases() -> None:
    with pytest.raises(ModelIntegrityError, match="unsafe"):
        require_safe_torch_version("2.9.1+cu128")
    assert require_safe_torch_version("2.10.0+cu128") == MIN_SAFE_TORCH_VERSION
    assert require_safe_torch_version("2.13.0.dev20260701") == (2, 13, 0)


def test_torch_version_floor_rejects_unparseable_version() -> None:
    with pytest.raises(ModelIntegrityError, match="cannot parse"):
        require_safe_torch_version("unknown")


def test_model_manifest_is_immutable_and_complete() -> None:
    assert len(CHATTERBOX_MODEL_REVISION) == 40
    assert CHATTERBOX_MODEL_REVISION != "main"
    assert set(CHATTERBOX_MODEL_ASSETS) == {
        "Cangjie5_TC.json",
        "conds.pt",
        "grapheme_mtl_merged_expanded_v1.json",
        "s3gen.pt",
        "t3_mtl23ls_v2.safetensors",
        "ve.pt",
    }


def test_verify_snapshot_accepts_matching_file(tmp_path: Path) -> None:
    content = b"reviewed checkpoint"
    assets = {
        "model.pt": ModelAsset(
            size=len(content),
            sha256=hashlib.sha256(content).hexdigest(),
        )
    }
    (tmp_path / "model.pt").write_bytes(content)
    assert verify_snapshot(tmp_path, assets) == tmp_path.resolve()


def test_verify_snapshot_rejects_missing_or_changed_file(tmp_path: Path) -> None:
    assets = {"model.pt": ModelAsset(size=4, sha256=hashlib.sha256(b"good").hexdigest())}
    with pytest.raises(ModelIntegrityError, match="missing"):
        verify_snapshot(tmp_path, assets)

    (tmp_path / "model.pt").write_bytes(b"evil")
    with pytest.raises(ModelIntegrityError, match="SHA-256 mismatch"):
        verify_snapshot(tmp_path, assets)


def test_download_uses_only_pinned_revision(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    captured: dict[str, object] = {}
    huggingface_hub = ModuleType("huggingface_hub")

    def fake_snapshot_download(**kwargs: object) -> str:
        captured.update(kwargs)
        return str(tmp_path)

    huggingface_hub.snapshot_download = fake_snapshot_download  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "huggingface_hub", huggingface_hub)
    monkeypatch.setattr(model_assets, "verify_snapshot", lambda path: path.resolve())

    assert model_assets.download_verified_snapshot() == tmp_path.resolve()
    assert captured["revision"] == CHATTERBOX_MODEL_REVISION
    assert captured["revision"] != "main"
    assert set(captured["allow_patterns"]) == set(CHATTERBOX_MODEL_ASSETS)  # type: ignore[arg-type]
