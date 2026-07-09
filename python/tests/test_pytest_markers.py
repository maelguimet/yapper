"""Markers used by GPU/integration smokes must be registered in pytest config."""

from __future__ import annotations

from pathlib import Path

import pytest


def test_gpu_and_integration_markers_registered(pytestconfig: pytest.Config) -> None:
    """Drive real pytest ini: unknown marks would warn / fail --strict-markers."""
    registered = {
        line.split(":", 1)[0].strip() for line in pytestconfig.getini("markers")
    }
    assert "gpu" in registered
    assert "integration" in registered


def test_root_pytest_ini_registers_same_markers() -> None:
    """Repo-root pytest.ini (when present) must declare gpu + integration."""
    root_ini = Path(__file__).resolve().parents[2] / "pytest.ini"
    assert root_ini.is_file(), f"missing root pytest.ini at {root_ini}"
    text = root_ini.read_text(encoding="utf-8")
    assert "gpu:" in text or "gpu " in text
    assert "integration:" in text or "integration " in text
    assert "markers" in text
