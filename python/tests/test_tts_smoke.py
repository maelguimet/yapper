"""GPU smoke for TTS worker (Chatterbox multilingual)."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import wave
from pathlib import Path

import pytest

pytestmark = pytest.mark.gpu

REPO = Path(__file__).resolve().parents[2]
PYTHON_ROOT = REPO / "python"
SCRATCH = Path(os.environ.get("YAPPER_SCRATCH", "/tmp/grok-goal-29cc0bace209/implementer"))


def _cuda_and_chatterbox() -> bool:
    try:
        import torch
        from chatterbox.mtl_tts import ChatterboxMultilingualTTS  # noqa: F401

        return bool(torch.cuda.is_available())
    except ImportError:
        return False


def _run_worker(commands: list[dict]) -> list[dict]:
    payload = "".join(json.dumps(c) + "\n" for c in commands)
    env = os.environ.copy()
    env["PYTHONPATH"] = str(PYTHON_ROOT) + (
        os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else ""
    )
    proc = subprocess.run(
        [sys.executable, "-m", "yapper_tts"],
        input=payload,
        text=True,
        capture_output=True,
        cwd=str(REPO),
        env=env,
        timeout=900,
        check=False,
    )
    if proc.returncode != 0 and not proc.stdout.strip():
        raise AssertionError(
            f"worker failed rc={proc.returncode}\nstdout={proc.stdout}\nstderr={proc.stderr[-4000:]}"
        )
    # Filter protocol lines only (JSON objects)
    out: list[dict] = []
    for ln in proc.stdout.splitlines():
        ln = ln.strip()
        if not ln.startswith("{"):
            continue
        out.append(json.loads(ln))
    if not out:
        raise AssertionError(f"no JSON responses\nstderr={proc.stderr[-4000:]}")
    return out


def _wav_ok(path: Path) -> bool:
    if not path.is_file() or path.stat().st_size < 1000:
        return False
    with wave.open(str(path), "rb") as wf:
        return wf.getnframes() > 0 and wf.getframerate() > 0


@pytest.mark.skipif(not _cuda_and_chatterbox(), reason="CUDA+chatterbox required")
def test_list_load_synth_en_fr_unload() -> None:
    SCRATCH.mkdir(parents=True, exist_ok=True)
    out_en = SCRATCH / "tts_en.wav"
    out_fr = SCRATCH / "tts_fr.wav"
    for p in (out_en, out_fr):
        p.unlink(missing_ok=True)

    responses = _run_worker(
        [
            {"id": "1", "cmd": "list_tones"},
            {"id": "2", "cmd": "load", "params": {"model": "chatterbox-multilingual", "device": "cuda"}},
            {
                "id": "3",
                "cmd": "synthesize",
                "params": {
                    "text": "Hello from yapper.",
                    "language": "en",
                    "tone": "neutral",
                    "voice": "eve",
                    "out_path": str(out_en),
                },
            },
            {
                "id": "4",
                "cmd": "synthesize",
                "params": {
                    "text": "Bonjour depuis yapper.",
                    "language": "fr",
                    "tone": "calm",
                    "voice": "eve",
                    "out_path": str(out_fr),
                },
            },
            {"id": "5", "cmd": "unload"},
            {"id": "6", "cmd": "status"},
            {"id": "7", "cmd": "ping"},
            {"id": "8", "cmd": "shutdown"},
        ]
    )
    by_id = {r["id"]: r for r in responses}
    assert by_id["1"]["ok"] is True
    assert len(by_id["1"]["result"]["tones"]) > 0
    assert by_id["2"]["ok"] is True, by_id["2"]
    assert by_id["3"]["ok"] is True, by_id["3"]
    assert by_id["4"]["ok"] is True, by_id["4"]
    assert _wav_ok(out_en), f"bad EN wav {out_en} size={out_en.stat().st_size if out_en.exists() else 0}"
    assert _wav_ok(out_fr), f"bad FR wav {out_fr}"
    assert by_id["5"]["ok"] is True
    assert by_id["6"]["result"]["loaded"] is False
    assert by_id["7"]["result"]["role"] == "tts"

    (SCRATCH / "tts-smoke-summary.txt").write_text(
        f"en={out_en.stat().st_size} fr={out_fr.stat().st_size} tones={by_id['1']['result']['tones']}\n",
        encoding="utf-8",
    )
