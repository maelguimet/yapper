"""GPU/integration smoke for STT worker entry (skips without CUDA/whisper)."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

pytestmark = pytest.mark.gpu

REPO = Path(__file__).resolve().parents[2]
PYTHON_ROOT = REPO / "python"
SCRATCH = Path(os.environ.get("YAPPER_SCRATCH", "/tmp/grok-goal-29cc0bace209/implementer"))


def _cuda_and_whisper_available() -> bool:
    try:
        import torch
        import whisper  # noqa: F401

        return bool(torch.cuda.is_available())
    except ImportError:
        return False


def _make_speech_wav(path: Path, text: str = "Hello, this is a yapper speech test.") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if shutil.which("espeak-ng"):
        wav_raw = path.with_suffix(".raw.wav")
        subprocess.run(
            ["espeak-ng", "-w", str(wav_raw), text],
            check=True,
            capture_output=True,
        )
        # Whisper prefers 16k mono
        subprocess.run(
            [
                "ffmpeg",
                "-y",
                "-i",
                str(wav_raw),
                "-ar",
                "16000",
                "-ac",
                "1",
                str(path),
            ],
            check=True,
            capture_output=True,
        )
        wav_raw.unlink(missing_ok=True)
        return
    raise RuntimeError("espeak-ng required to build speech fixture")


def _run_worker(commands: list[dict]) -> list[dict]:
    payload = "".join(json.dumps(c) + "\n" for c in commands)
    env = os.environ.copy()
    env["PYTHONPATH"] = str(PYTHON_ROOT) + (
        os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else ""
    )
    proc = subprocess.run(
        [sys.executable, "-m", "yapper_stt"],
        input=payload,
        text=True,
        capture_output=True,
        cwd=str(REPO),
        env=env,
        timeout=600,
        check=False,
    )
    if proc.returncode != 0 and not proc.stdout.strip():
        raise AssertionError(
            f"worker failed rc={proc.returncode}\nstdout={proc.stdout}\nstderr={proc.stderr}"
        )
    lines = [ln for ln in proc.stdout.splitlines() if ln.strip()]
    return [json.loads(ln) for ln in lines]


@pytest.mark.skipif(not _cuda_and_whisper_available(), reason="CUDA+whisper required")
def test_load_transcribe_unload_via_stdio(tmp_path: Path) -> None:
    fixture = SCRATCH / "fixtures" / "speech_en.wav"
    _make_speech_wav(fixture)

    download_root = Path.home() / ".local" / "share" / "yapper" / "models" / "whisper"
    download_root.mkdir(parents=True, exist_ok=True)

    # Pre-download via handler API so stdio test is deterministic
    from yapper_stt.worker import SttWorker
    from yapper_common.ipc import Request

    pre = SttWorker(download_root=download_root)
    load_resp = pre.handle(
        Request(id="pre", cmd="load", params={"model": "small", "device": "cuda"})
    )
    assert load_resp.ok, load_resp
    pre.handle(Request(id="pre2", cmd="unload"))

    responses = _run_worker(
        [
            {"id": "1", "cmd": "ping"},
            {"id": "2", "cmd": "load", "params": {"model": "small", "device": "cuda"}},
            {
                "id": "3",
                "cmd": "transcribe",
                "params": {"path": str(fixture), "language": "en"},
            },
            {"id": "4", "cmd": "unload"},
            {"id": "5", "cmd": "status"},
            {"id": "6", "cmd": "ping"},
            {"id": "7", "cmd": "shutdown"},
        ]
    )
    by_id = {r["id"]: r for r in responses}
    assert by_id["1"]["ok"] is True
    assert by_id["2"]["ok"] is True, by_id["2"]
    assert by_id["3"]["ok"] is True, by_id["3"]
    text = by_id["3"]["result"]["text"].strip()
    assert text, "expected non-empty transcript"
    assert by_id["4"]["ok"] is True
    assert by_id["5"]["ok"] is True
    assert by_id["5"]["result"]["loaded"] is False
    assert by_id["6"]["ok"] is True
    assert by_id["6"]["result"]["role"] == "stt"

    SCRATCH.mkdir(parents=True, exist_ok=True)
    (SCRATCH / "stt-smoke-transcript.txt").write_text(text + "\n", encoding="utf-8")


@pytest.mark.skipif(not _cuda_and_whisper_available(), reason="CUDA+whisper required")
def test_stt_model_swap_small_medium_small_active_model() -> None:
    """P1-G: selected model is the active one across small → medium → small.

    One worker process; status.model matches each load; no dual residency
    (reload drops previous weights). File transcribe yields non-empty text.
    """
    SCRATCH.mkdir(parents=True, exist_ok=True)
    fixture = SCRATCH / "fixtures" / "speech_en_swap.wav"
    _make_speech_wav(fixture)
    download_root = Path.home() / ".local" / "share" / "yapper" / "models" / "whisper"
    download_root.mkdir(parents=True, exist_ok=True)

    from yapper_common.ipc import Request
    from yapper_stt.worker import SttWorker

    worker = SttWorker(download_root=download_root)
    transcripts: list[str] = []
    try:
        for model in ("small", "medium", "small"):
            load = worker.handle(
                Request(id=f"load-{model}", cmd="load", params={"model": model, "device": "cuda"})
            )
            assert load.ok, load
            status = worker.handle(Request(id=f"st-{model}", cmd="status"))
            assert status.ok and status.result.get("loaded") is True
            assert status.result.get("model") == model, status.result
            tx = worker.handle(
                Request(
                    id=f"tx-{model}",
                    cmd="transcribe",
                    params={"path": str(fixture), "language": "en"},
                )
            )
            assert tx.ok, tx
            text = str(tx.result.get("text", "")).strip()
            assert text, f"empty transcript for {model}"
            transcripts.append(f"{model}: {text}")
        final = worker.handle(Request(id="final", cmd="status"))
        assert final.result.get("model") == "small"
        assert final.result.get("loaded") is True
    finally:
        worker.handle(Request(id="unload", cmd="unload"))

    (SCRATCH / "stt-swap-transcripts.txt").write_text(
        "\n".join(transcripts) + "\n", encoding="utf-8"
    )
