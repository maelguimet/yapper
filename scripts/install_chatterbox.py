#!/usr/bin/env python3
"""Install a verified Chatterbox 0.1.7 source with a safe PyTorch override."""

from __future__ import annotations

import argparse
import hashlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

CHATTERBOX_VERSION = "0.1.7"
CHATTERBOX_SDIST_URL = (
    "https://files.pythonhosted.org/packages/c4/b1/"
    "8f1203e868111a45b566a79a4f56cd7843c420dfda709b81cebee55afa10/"
    "chatterbox_tts-0.1.7.tar.gz"
)
CHATTERBOX_SDIST_SHA256 = (
    "ed8afae83819b40a25927c2ef3bcc67f928bdfcf434c1376c972e6039252a187"
)

DEPENDENCY_REPLACEMENTS = {
    '"torch==2.6.0; python_version < \'3.14\'"': (
        '"torch==2.10.0; python_version < \'3.14\'"'
    ),
    '"torch>=2.9.0; python_version >= \'3.14\'"': (
        '"torch==2.10.0; python_version >= \'3.14\'"'
    ),
    '"torchaudio==2.6.0; python_version < \'3.14\'"': (
        '"torchaudio==2.10.0; python_version < \'3.14\'"'
    ),
    '"torchaudio>=2.9.0; python_version >= \'3.14\'"': (
        '"torchaudio==2.10.0; python_version >= \'3.14\'"'
    ),
}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def patch_pyproject_text(text: str) -> str:
    """Apply the narrow dependency override, refusing unknown upstream input."""

    patched = text
    for old, new in DEPENDENCY_REPLACEMENTS.items():
        count = patched.count(old)
        if count != 1:
            raise RuntimeError(
                f"refusing to patch unexpected Chatterbox metadata: "
                f"wanted exactly one {old!r}, found {count}"
            )
        patched = patched.replace(old, new)
    return patched


def safe_extract_sdist(archive_path: Path, destination: Path) -> Path:
    """Extract only regular files/directories beneath ``destination``."""

    destination.mkdir(parents=True, exist_ok=True)
    resolved_destination = destination.resolve()
    with tarfile.open(archive_path, mode="r:gz") as archive:
        for member in archive.getmembers():
            target = (destination / member.name).resolve()
            if resolved_destination != target and resolved_destination not in target.parents:
                raise RuntimeError(f"unsafe path in Chatterbox sdist: {member.name!r}")
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            if not member.isfile():
                raise RuntimeError(
                    f"unsupported entry in Chatterbox sdist: {member.name!r}"
                )
            source = archive.extractfile(member)
            if source is None:
                raise RuntimeError(f"cannot read Chatterbox sdist entry: {member.name!r}")
            target.parent.mkdir(parents=True, exist_ok=True)
            with source, target.open("wb") as output:
                shutil.copyfileobj(source, output)
            target.chmod(member.mode & 0o777)

    source_root = destination / f"chatterbox_tts-{CHATTERBOX_VERSION}"
    if not source_root.is_dir():
        raise RuntimeError(f"Chatterbox sdist root is missing: {source_root}")
    return source_root


def install(requirements: Path) -> None:
    requirements = requirements.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="yapper-chatterbox-") as temp_raw:
        temp = Path(temp_raw)
        archive_path = temp / f"chatterbox_tts-{CHATTERBOX_VERSION}.tar.gz"
        with urllib.request.urlopen(CHATTERBOX_SDIST_URL, timeout=60) as response:
            with archive_path.open("wb") as output:
                shutil.copyfileobj(response, output)

        actual_hash = sha256_file(archive_path)
        if actual_hash != CHATTERBOX_SDIST_SHA256:
            raise RuntimeError(
                "Chatterbox sdist SHA-256 mismatch: "
                f"expected {CHATTERBOX_SDIST_SHA256}, got {actual_hash}"
            )

        source_root = safe_extract_sdist(archive_path, temp / "source")
        pyproject = source_root / "pyproject.toml"
        original = pyproject.read_text(encoding="utf-8")
        pyproject.write_text(patch_pyproject_text(original), encoding="utf-8")

        subprocess.run(
            [
                sys.executable,
                "-m",
                "pip",
                "install",
                "-r",
                str(requirements),
                str(source_root),
            ],
            check=True,
            timeout=900,
        )
        subprocess.run(
            [sys.executable, "-m", "pip", "check"],
            check=True,
            timeout=60,
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--requirements", type=Path, required=True)
    args = parser.parse_args()
    install(args.requirements)


if __name__ == "__main__":
    main()
