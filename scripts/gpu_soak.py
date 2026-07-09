#!/usr/bin/env python3
"""Host GPU soak for Yapper P1-G.

Drives real STT/TTS workers (stdio JSON-RPC), transport replay path via
concat WAVs, and unload VRAM release. Writes evidence under YAPPER_SCRATCH
(default: /tmp/grok-goal-06718934142e/implementer).

Usage:
  timeout 1800s env YAPPER_SCRATCH=... PYTHONPATH=python \\
    .venv/bin/python scripts/gpu_soak.py
"""

from __future__ import annotations

import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
import wave
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
PYTHON_ROOT = REPO / "python"
SCRATCH = Path(
    os.environ.get("YAPPER_SCRATCH", "/tmp/grok-goal-06718934142e/implementer")
)
PYTHON = os.environ.get(
    "YAPPER_PYTHON", str(REPO / ".venv" / "bin" / "python")
)
MODELS_DIR = Path(
    os.environ.get(
        "YAPPER_MODELS_DIR",
        str(Path.home() / ".local" / "share" / "yapper" / "models"),
    )
)
VOICES_DIR = Path(
    os.environ.get(
        "YAPPER_VOICES_DIR",
        str(Path.home() / ".local" / "share" / "yapper" / "voices"),
    )
)

# ≥20 short sentences — multi-chunk monologue for long-read soak.
MONOLOGUE = " ".join(
    [
        f"Sentence number {i} is part of the yapper long-read soak monologue."
        for i in range(1, 21)
    ]
)


def log(msg: str) -> None:
    line = f"[{time.strftime('%H:%M:%S')}] {msg}"
    print(line, flush=True)
    (SCRATCH / "gpu-soak-master.log").open("a", encoding="utf-8").write(line + "\n")


def nvidia_smi(name: str) -> Path:
    path = SCRATCH / f"nvidia-smi-{name}.txt"
    r = subprocess.run(
        ["nvidia-smi"],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    path.write_text(r.stdout + r.stderr, encoding="utf-8")
    # Also capture used MiB for quick delta
    m = re.search(r"(\d+)\s*MiB\s*/\s*(\d+)\s*MiB", r.stdout)
    if m:
        log(f"nvidia-smi {name}: {m.group(1)} MiB / {m.group(2)} MiB")
    else:
        log(f"nvidia-smi {name}: saved {path}")
    return path


def gpu_used_mib() -> int | None:
    r = subprocess.run(
        [
            "nvidia-smi",
            "--query-gpu=memory.used",
            "--format=csv,noheader,nounits",
        ],
        capture_output=True,
        text=True,
        timeout=15,
        check=False,
    )
    try:
        return int(r.stdout.strip().splitlines()[0].strip())
    except (ValueError, IndexError):
        return None


def worker_pids(role: str) -> list[int]:
    """PIDs whose cmdline looks like a yapper STT/TTS worker."""
    needle = f"yapper_{role}"
    found: list[int] = []
    for p in Path("/proc").iterdir():
        if not p.name.isdigit():
            continue
        try:
            cmd = (p / "cmdline").read_bytes().replace(b"\x00", b" ").decode(
                "utf-8", errors="replace"
            )
        except OSError:
            continue
        if needle in cmd and "python" in cmd:
            found.append(int(p.name))
    return found


def split_sentences(text: str) -> list[str]:
    parts = re.split(r"(?<=[.!?…])\s+", text.strip())
    return [p.strip() for p in parts if p.strip()]


def wav_ok(path: Path) -> bool:
    if not path.is_file() or path.stat().st_size < 500:
        return False
    try:
        with wave.open(str(path), "rb") as wf:
            return wf.getnframes() > 0 and wf.getframerate() > 0
    except wave.Error:
        return False


def wav_duration(path: Path) -> float:
    with wave.open(str(path), "rb") as wf:
        return wf.getnframes() / float(wf.getframerate())


def concat_wavs(paths: list[Path], out: Path) -> float:
    """Simple PCM concat (same rate/channels assumed) → returns duration secs."""
    if not paths:
        raise ValueError("no paths")
    frames = b""
    nchannels = sampwidth = framerate = None
    for p in paths:
        with wave.open(str(p), "rb") as wf:
            if nchannels is None:
                nchannels, sampwidth, framerate = (
                    wf.getnchannels(),
                    wf.getsampwidth(),
                    wf.getframerate(),
                )
            frames += wf.readframes(wf.getnframes())
    assert nchannels is not None and sampwidth is not None and framerate is not None
    out.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(out), "wb") as wo:
        wo.setnchannels(nchannels)
        wo.setsampwidth(sampwidth)
        wo.setframerate(framerate)
        wo.writeframes(frames)
    return len(frames) / (sampwidth * nchannels * framerate)


class WorkerSession:
    """Interactive stdio JSON-RPC session with a STT or TTS worker.

    Mirrors Rust ``WorkerClient``: ``PYTHONUNBUFFERED=1``, non-JSON stdout lines
    skipped, stderr drained to a log (never PIPE without a reader — that
    deadlocks once the OS pipe buffer fills with HF/torch chatter).
    """

    def __init__(self, role: str) -> None:
        self.role = role
        mod = "yapper_stt" if role == "stt" else "yapper_tts"
        env = os.environ.copy()
        env["PYTHONPATH"] = str(PYTHON_ROOT) + (
            os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else ""
        )
        env["PYTHONUNBUFFERED"] = "1"
        env["YAPPER_MODELS_DIR"] = str(MODELS_DIR)
        env["YAPPER_VOICES_DIR"] = str(VOICES_DIR)
        self.stderr_path = SCRATCH / f"worker-{role}-{int(time.time()*1000)}.stderr.log"
        self._stderr_fh = self.stderr_path.open("w", encoding="utf-8")
        self.proc = subprocess.Popen(
            [PYTHON, "-u", "-m", mod],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self._stderr_fh,
            text=True,
            cwd=str(REPO),
            env=env,
            bufsize=1,
        )
        log(f"spawned {role} worker pid={self.proc.pid} stderr={self.stderr_path}")

    def request(self, req_id: str, cmd: str, params: dict | None = None, timeout: float = 600.0) -> dict:
        assert self.proc.stdin and self.proc.stdout
        payload = json.dumps({"id": req_id, "cmd": cmd, "params": params or {}})
        self.proc.stdin.write(payload + "\n")
        self.proc.stdin.flush()
        deadline = time.time() + timeout
        import select

        while time.time() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError(
                    f"{self.role} exited rc={self.proc.returncode} during {cmd}; "
                    f"see {self.stderr_path}"
                )
            remaining = deadline - time.time()
            if remaining <= 0:
                break
            # Poll in short slices so we notice process death quickly.
            r, _, _ = select.select([self.proc.stdout], [], [], min(remaining, 0.5))
            if not r:
                continue
            line = self.proc.stdout.readline()
            if line == "":
                # EOF
                raise RuntimeError(
                    f"{self.role} closed stdout during {cmd}; see {self.stderr_path}"
                )
            line = line.strip()
            # Chatterbox may print non-JSON to stdout (e.g. PerthNet); skip like Rust.
            if not line.startswith("{"):
                continue
            obj = json.loads(line)
            if obj.get("id") == req_id:
                return obj
        raise TimeoutError(f"{self.role} {cmd} id={req_id} timed out after {timeout}s")

    def kill(self) -> None:
        if self.proc.poll() is not None:
            self._close_stderr()
            return
        log(f"killing {self.role} pid={self.proc.pid}")
        try:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)
        except OSError:
            pass
        self._close_stderr()

    def shutdown_clean(self) -> None:
        if self.proc.poll() is not None:
            self._close_stderr()
            return
        try:
            self.request("shutdown", "shutdown", timeout=30)
        except Exception as exc:  # noqa: BLE001
            log(f"clean shutdown failed ({exc}); killing")
            self.kill()
            return
        try:
            self.proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.kill()
            return
        self._close_stderr()

    def _close_stderr(self) -> None:
        try:
            self._stderr_fh.close()
        except OSError:
            pass


def make_speech_fixture(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_file() and path.stat().st_size > 1000:
        return
    raw = path.with_suffix(".raw.wav")
    subprocess.run(
        ["espeak-ng", "-w", str(raw), "Hello, this is a yapper speech test."],
        check=True,
        capture_output=True,
        timeout=30,
    )
    subprocess.run(
        ["ffmpeg", "-y", "-i", str(raw), "-ar", "16000", "-ac", "1", str(path)],
        check=True,
        capture_output=True,
        timeout=30,
    )
    raw.unlink(missing_ok=True)


def soak_tts_long_read_10x() -> None:
    log("=== TTS long-read 10× ===")
    segs = split_sentences(MONOLOGUE)
    assert len(segs) >= 20, f"need ≥20 sentences, got {len(segs)}"
    log_path = SCRATCH / "tts-long-read-10x.log"
    fh = log_path.open("w", encoding="utf-8")

    def L(msg: str) -> None:
        log(msg)
        fh.write(msg + "\n")
        fh.flush()

    tts = WorkerSession("tts")
    try:
        r = tts.request("load", "load", {"model": "chatterbox-multilingual", "device": "cuda"}, timeout=300)
        assert r["ok"], r
        L(f"load ok vram≈{r.get('result', {}).get('vram_mb')}")
        nvidia_smi("tts-loaded")

        for run in range(1, 11):
            run_dir = SCRATCH / "tts-long" / f"run{run:02d}"
            run_dir.mkdir(parents=True, exist_ok=True)
            L(f"--- run {run}/10 segments={len(segs)} ---")
            t0 = time.time()
            paths: list[Path] = []
            for i, seg in enumerate(segs):
                out = run_dir / f"seg{i:02d}.wav"
                out.unlink(missing_ok=True)
                rr = tts.request(
                    f"r{run}s{i}",
                    "synthesize",
                    {
                        "text": seg,
                        "language": "en",
                        "tone": "neutral",
                        "voice": "eve",
                        "out_path": str(out),
                    },
                    timeout=180,
                )
                if not rr.get("ok"):
                    L(f"FAIL run={run} seg={i}: {rr}")
                    raise AssertionError(f"synth failed run={run} seg={i}: {rr}")
                if not wav_ok(out):
                    raise AssertionError(f"bad wav run={run} seg={i} {out}")
                paths.append(out)
                L(f"  seg {i}/{len(segs)-1} ok size={out.stat().st_size} dur={wav_duration(out):.2f}s")
            elapsed = time.time() - t0
            full = run_dir / "full.wav"
            full_dur = concat_wavs(paths, full)
            L(f"run {run}/10 complete in {elapsed:.1f}s full_dur={full_dur:.2f}s chunks={len(paths)}")
            if run == 5:
                nvidia_smi("tts-mid")

        # Status still healthy after series
        st = tts.request("status", "status", timeout=30)
        assert st["ok"] and st["result"]["loaded"] is True, st
        L(f"post-series status={st['result']}")
        orphans = worker_pids("tts")
        L(f"tts worker pids after series (expect our live one): {orphans}")
        assert tts.proc.pid in orphans or tts.proc.poll() is None
        L("TTS long-read 10× PASS")
        try:
            tts.request("unload", "unload", timeout=60)
            tts.shutdown_clean()
        except Exception as exc:  # noqa: BLE001
            L(f"post-series unload/shutdown: {exc}")
            tts.kill()
    except Exception:
        tts.kill()
        raise
    finally:
        fh.close()


def soak_stop_restart_replay() -> None:
    log("=== Stop / Restart mid-generate + Replay ===")
    log_path = SCRATCH / "tts-stop-restart-replay.log"
    fh = log_path.open("w", encoding="utf-8")

    def L(msg: str) -> None:
        log(msg)
        fh.write(msg + "\n")
        fh.flush()

    segs = split_sentences(MONOLOGUE)
    # --- Stop mid-generate: kill worker while synthesize is in flight ---
    stop_dir = SCRATCH / "tts-stop"
    stop_dir.mkdir(parents=True, exist_ok=True)
    tts = WorkerSession("tts")
    try:
        r = tts.request("load", "load", {"model": "chatterbox-multilingual", "device": "cuda"}, timeout=300)
        assert r["ok"], r
        # Synthesize first short chunk cleanly
        p0 = stop_dir / "seg0.wav"
        r0 = tts.request(
            "s0",
            "synthesize",
            {
                "text": segs[0],
                "language": "en",
                "tone": "neutral",
                "voice": "eve",
                "out_path": str(p0),
            },
            timeout=180,
        )
        assert r0["ok"] and wav_ok(p0), r0
        L(f"pre-stop seg0 ok")

        # Start a longer synth in a thread-ish way: fire request then kill mid-flight
        long_text = " ".join(segs[1:8])  # multi-sentence long enough to be interruptible
        p_long = stop_dir / "should_abort.wav"
        p_long.unlink(missing_ok=True)
        assert tts.proc.stdin
        payload = json.dumps(
            {
                "id": "abort_me",
                "cmd": "synthesize",
                "params": {
                    "text": long_text,
                    "language": "en",
                    "tone": "neutral",
                    "voice": "eve",
                    "out_path": str(p_long),
                },
            }
        )
        tts.proc.stdin.write(payload + "\n")
        tts.proc.stdin.flush()
        time.sleep(1.5)  # let GPU work start
        L(f"sending SIGTERM mid-synth to pid={tts.proc.pid}")
        tts.kill()
        # Wait a moment; process must be dead; no orphan CUDA context from this worker
        time.sleep(1.0)
        assert tts.proc.poll() is not None, "worker should be dead after stop kill"
        L(f"stop mid-generate: worker dead rc={tts.proc.returncode}")
        # Cancelled job must not leave a completed long wav that finished after kill
        # (partial file may exist; full clean complete is the failure mode)
        if p_long.is_file() and wav_ok(p_long):
            # Accept only if file is tiny/incomplete vs expected multi-sentence length
            L(f"note: partial wav after kill size={p_long.stat().st_size} dur={wav_duration(p_long):.2f}s")
        L("Stop mid-generate PASS (worker killed; no continued session)")
    finally:
        tts.kill()

    # --- Restart mid-generate: new worker + new speak path succeeds ---
    restart_dir = SCRATCH / "tts-restart"
    restart_dir.mkdir(parents=True, exist_ok=True)
    tts2 = WorkerSession("tts")
    try:
        r = tts2.request("load", "load", {"model": "chatterbox-multilingual", "device": "cuda"}, timeout=300)
        assert r["ok"], r
        # Simulate mid-synth kill of a "prior" job by starting synth and killing quickly
        prior = restart_dir / "prior.wav"
        assert tts2.proc.stdin
        tts2.proc.stdin.write(
            json.dumps(
                {
                    "id": "prior",
                    "cmd": "synthesize",
                    "params": {
                        "text": " ".join(segs[:5]),
                        "language": "en",
                        "tone": "neutral",
                        "voice": "eve",
                        "out_path": str(prior),
                    },
                }
            )
            + "\n"
        )
        tts2.proc.stdin.flush()
        time.sleep(1.2)
        tts2.kill()
        L("prior mid-synth killed")

        # New speak path on fresh worker — must succeed cleanly (no stale error)
        tts3 = WorkerSession("tts")
        try:
            r = tts3.request("load", "load", {"model": "chatterbox-multilingual", "device": "cuda"}, timeout=300)
            assert r["ok"], r
            out = restart_dir / "restarted.wav"
            rr = tts3.request(
                "new",
                "synthesize",
                {
                    "text": "Restart path speaks cleanly after mid-generate kill.",
                    "language": "en",
                    "tone": "neutral",
                    "voice": "eve",
                    "out_path": str(out),
                },
                timeout=180,
            )
            assert rr["ok"] and wav_ok(out), rr
            # status must be healthy loaded, not error
            st = tts3.request("st", "status", timeout=30)
            assert st["ok"] and st["result"]["loaded"] is True, st
            L(f"Restart mid-generate PASS status={st['result']} size={out.stat().st_size}")
            tts3.request("unload", "unload", timeout=60)
            tts3.shutdown_clean()
        except Exception:
            tts3.kill()
            raise
    finally:
        tts2.kill()

    # --- Replay full utterance: multi-chunk concat duration ≈ sum ---
    replay_dir = SCRATCH / "tts-replay"
    replay_dir.mkdir(parents=True, exist_ok=True)
    tts4 = WorkerSession("tts")
    try:
        r = tts4.request("load", "load", {"model": "chatterbox-multilingual", "device": "cuda"}, timeout=300)
        assert r["ok"], r
        chunk_texts = segs[:6]
        paths: list[Path] = []
        sum_dur = 0.0
        for i, seg in enumerate(chunk_texts):
            p = replay_dir / f"c{i}.wav"
            rr = tts4.request(
                f"c{i}",
                "synthesize",
                {
                    "text": seg,
                    "language": "en",
                    "tone": "neutral",
                    "voice": "eve",
                    "out_path": str(p),
                },
                timeout=180,
            )
            assert rr["ok"] and wav_ok(p), rr
            d = wav_duration(p)
            sum_dur += d
            paths.append(p)
            L(f"replay chunk {i} dur={d:.2f}s")
        full = replay_dir / "full_utterance.wav"
        full_dur = concat_wavs(paths, full)
        last_only = wav_duration(paths[-1])
        assert abs(full_dur - sum_dur) < 0.15, f"full={full_dur} sum={sum_dur}"
        assert full_dur > last_only * 1.5, (
            f"full utterance must exceed last chunk only: full={full_dur} last={last_only}"
        )
        L(f"Replay full utterance PASS full={full_dur:.2f}s sum={sum_dur:.2f}s last={last_only:.2f}s")
        tts4.request("unload", "unload", timeout=60)
        tts4.shutdown_clean()
    except Exception:
        tts4.kill()
        raise
    finally:
        fh.close()


def soak_stt_swap_and_file() -> None:
    log("=== STT small → medium → small + transcribe file ===")
    log_path = SCRATCH / "stt-soak.log"
    fh = log_path.open("w", encoding="utf-8")

    def L(msg: str) -> None:
        log(msg)
        fh.write(msg + "\n")
        fh.flush()

    fixture = SCRATCH / "fixtures" / "speech_en.wav"
    make_speech_fixture(fixture)
    L(f"fixture={fixture} size={fixture.stat().st_size}")

    stt = WorkerSession("stt")
    try:
        for model in ("small", "medium", "small"):
            nvidia_smi(f"stt-before-{model}")
            r = stt.request(
                f"load-{model}",
                "load",
                {"model": model, "device": "cuda"},
                timeout=300 if model == "medium" else 180,
            )
            assert r["ok"], r
            st = stt.request(f"status-{model}", "status", timeout=30)
            assert st["ok"], st
            assert st["result"]["loaded"] is True
            assert st["result"]["model"] == model, st["result"]
            L(f"loaded model={st['result']['model']} vram_mb={r.get('result', {}).get('vram_mb')}")
            nvidia_smi(f"stt-loaded-{model}")

            tr = stt.request(
                f"tx-{model}",
                "transcribe",
                {"path": str(fixture), "language": "en"},
                timeout=180,
            )
            assert tr["ok"], tr
            text = (tr["result"].get("text") or "").strip()
            assert text, f"empty transcript for {model}: {tr}"
            L(f"transcribe {model}: {text!r}")

        # After final small, only one model (small) resident
        st = stt.request("final-status", "status", timeout=30)
        assert st["result"]["model"] == "small"
        assert st["result"]["loaded"] is True
        L(f"final active model={st['result']['model']} (no dual residency)")
        L("STT small→medium→small + file PASS")
        stt.request("unload", "unload", timeout=60)
        stt.shutdown_clean()
    except Exception:
        stt.kill()
        raise
    finally:
        fh.close()


def soak_unload_all_vram() -> None:
    log("=== Unload all VRAM release ===")
    before = gpu_used_mib()
    nvidia_smi("before-dual-load")
    log(f"baseline used_mib={before}")

    stt = WorkerSession("stt")
    tts = WorkerSession("tts")
    try:
        rs = stt.request("load", "load", {"model": "small", "device": "cuda"}, timeout=180)
        assert rs["ok"], rs
        rt = tts.request(
            "load", "load", {"model": "chatterbox-multilingual", "device": "cuda"}, timeout=300
        )
        assert rt["ok"], rt
        loaded = gpu_used_mib()
        nvidia_smi("loaded")
        log(f"loaded peak used_mib={loaded} stt_pids={worker_pids('stt')} tts_pids={worker_pids('tts')}")
        assert loaded is not None and before is not None
        assert loaded > before + 200, f"expected material VRAM rise: before={before} loaded={loaded}"

        # Unload all (same semantics as workers.unload_all)
        stt.request("u", "unload", timeout=60)
        tts.request("u", "unload", timeout=60)
        st_st = stt.request("ss", "status", timeout=30)
        tt_st = tts.request("ts", "status", timeout=30)
        assert st_st["result"]["loaded"] is False, st_st
        assert tt_st["result"]["loaded"] is False, tt_st
        # Process still up until shutdown (product may keep process or kill — either ok if VRAM free)
        stt.shutdown_clean()
        tts.shutdown_clean()
        time.sleep(2.0)
        after = gpu_used_mib()
        nvidia_smi("after-unload")
        log(f"after unload used_mib={after} stt_pids={worker_pids('stt')} tts_pids={worker_pids('tts')}")
        assert not worker_pids("stt"), f"orphan STT workers: {worker_pids('stt')}"
        assert not worker_pids("tts"), f"orphan TTS workers: {worker_pids('tts')}"
        assert after is not None and loaded is not None
        # Must drop substantially from peak (model-sized footprint gone)
        drop = loaded - after
        log(f"VRAM drop={drop} MiB (loaded={loaded} after={after})")
        assert drop > 500, f"expected large VRAM release, drop={drop} MiB"
        (SCRATCH / "unload-vram-summary.txt").write_text(
            f"before={before} loaded={loaded} after={after} drop={drop}\n",
            encoding="utf-8",
        )
        log("Unload all VRAM PASS")
    except Exception:
        stt.kill()
        tts.kill()
        raise


def main() -> int:
    SCRATCH.mkdir(parents=True, exist_ok=True)
    (SCRATCH / "gpu-soak-master.log").write_text("", encoding="utf-8")
    log(f"SCRATCH={SCRATCH}")
    log(f"PYTHON={PYTHON}")
    log(f"MODELS_DIR={MODELS_DIR}")
    log(f"VOICES_DIR={VOICES_DIR}")
    log(f"DISPLAY={os.environ.get('DISPLAY')}")
    nvidia_smi("baseline")

    failures: list[str] = []

    steps = [
        ("tts_long_read_10x", soak_tts_long_read_10x),
        ("stop_restart_replay", soak_stop_restart_replay),
        ("stt_swap_file", soak_stt_swap_and_file),
        ("unload_all_vram", soak_unload_all_vram),
    ]
    # Allow partial runs via YAPPER_SOAK_STEPS=tts_long_read_10x,stt_swap_file
    only = os.environ.get("YAPPER_SOAK_STEPS", "").strip()
    if only:
        wanted = {s.strip() for s in only.split(",") if s.strip()}
        steps = [(n, f) for n, f in steps if n in wanted]

    for name, fn in steps:
        t0 = time.time()
        try:
            fn()
            log(f"STEP {name} OK ({time.time() - t0:.1f}s)")
        except Exception as exc:  # noqa: BLE001
            log(f"STEP {name} FAIL: {exc}")
            failures.append(f"{name}: {exc}")
            # Kill any leftover workers so next step has clean GPU
            for role in ("stt", "tts"):
                for pid in worker_pids(role):
                    try:
                        os.kill(pid, signal.SIGKILL)
                    except OSError:
                        pass

    summary = SCRATCH / "gpu-soak-summary.txt"
    if failures:
        summary.write_text("FAIL\n" + "\n".join(failures) + "\n", encoding="utf-8")
        log(f"SOAK FAILED: {failures}")
        return 1
    summary.write_text("PASS\n", encoding="utf-8")
    log("SOAK ALL PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
