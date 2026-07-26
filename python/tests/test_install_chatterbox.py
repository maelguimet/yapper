from __future__ import annotations

import importlib.util
import io
import tarfile
from pathlib import Path
from types import ModuleType

import pytest


def _load_installer() -> ModuleType:
    script = Path(__file__).resolve().parents[2] / "scripts" / "install_chatterbox.py"
    spec = importlib.util.spec_from_file_location("install_chatterbox", script)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


INSTALLER = _load_installer()


def test_dependency_patch_replaces_only_vulnerable_torch_pins() -> None:
    original = "\n".join(INSTALLER.DEPENDENCY_REPLACEMENTS)
    patched = INSTALLER.patch_pyproject_text(original)
    assert "torch==2.6.0" not in patched
    assert "torchaudio==2.6.0" not in patched
    assert patched.count("torch==2.10.0") == 2
    assert patched.count("torchaudio==2.10.0") == 2


def test_dependency_patch_refuses_changed_upstream_metadata() -> None:
    with pytest.raises(RuntimeError, match="unexpected Chatterbox metadata"):
        INSTALLER.patch_pyproject_text("torch==some-new-version")


def test_safe_extract_rejects_path_traversal(tmp_path: Path) -> None:
    archive_path = tmp_path / "bad.tar.gz"
    with tarfile.open(archive_path, "w:gz") as archive:
        payload = b"nope"
        member = tarfile.TarInfo("../escape")
        member.size = len(payload)
        archive.addfile(member, io.BytesIO(payload))

    with pytest.raises(RuntimeError, match="unsafe path"):
        INSTALLER.safe_extract_sdist(archive_path, tmp_path / "extract")
