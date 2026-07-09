"""TTS worker command handlers (Chatterbox multilingual). Process stays up across unload."""

from __future__ import annotations

import gc
import logging
import wave
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from yapper_common.ipc import Request, Response
from yapper_tts.tones import list_tone_names, resolve_tone

log = logging.getLogger("yapper.tts")

TTS_VERSION = "0.1.0"
TTS_MODEL_ID = "chatterbox-multilingual"
ALLOWED_LANGUAGES = frozenset({"en", "fr"})


@dataclass
class TtsState:
    model: Any = None
    model_name: str | None = None
    device: str | None = None
    sample_rate: int | None = None


@dataclass
class TtsWorker:
    """In-process TTS state + command dispatch."""

    state: TtsState = field(default_factory=TtsState)
    voices_root: Path | None = None

    def handle(self, req: Request) -> Response:
        handlers = {
            "ping": self._ping,
            "status": self._status,
            "load": self._load,
            "unload": self._unload,
            "list_tones": self._list_tones,
            "synthesize": self._synthesize,
            "shutdown": self._shutdown,
        }
        handler = handlers.get(req.cmd)
        if handler is None:
            return Response.failure(req.id, "bad_args", f"unknown cmd: {req.cmd!r}")
        try:
            return handler(req)
        except TtsError as exc:
            return Response.failure(req.id, exc.code, exc.message)
        except Exception as exc:  # noqa: BLE001 — worker boundary
            log.exception("internal error on %s", req.cmd)
            return Response.failure(req.id, "internal", str(exc))

    def _ping(self, req: Request) -> Response:
        return Response.success(
            req.id,
            {"role": "tts", "version": TTS_VERSION, "proto": 1},
        )

    def _status(self, req: Request) -> Response:
        return Response.success(
            req.id,
            {
                "loaded": self.state.model is not None,
                "model": self.state.model_name,
                "device": self.state.device,
            },
        )

    def _list_tones(self, req: Request) -> Response:
        tones = list_tone_names(self.voices_root)
        return Response.success(req.id, {"tones": tones})

    def _load(self, req: Request) -> Response:
        model_name = str(req.params.get("model", TTS_MODEL_ID)).strip() or TTS_MODEL_ID
        if model_name != TTS_MODEL_ID:
            raise TtsError(
                "bad_args",
                f"only model {TTS_MODEL_ID!r} supported, got {model_name!r}",
            )
        device = str(req.params.get("device", "cuda")).strip() or "cuda"
        if device not in ("cuda", "cpu"):
            raise TtsError("bad_args", f"device must be cuda|cpu, got {device!r}")

        if self.state.model is not None and self.state.model_name == model_name:
            return Response.success(
                req.id,
                {
                    "model": model_name,
                    "device": self.state.device,
                    "vram_mb": _approx_vram_mb(),
                    "already_loaded": True,
                },
            )

        if self.state.model is not None:
            self._drop_model()

        try:
            import torch
            from chatterbox.mtl_tts import ChatterboxMultilingualTTS
        except ImportError as exc:
            raise TtsError("internal", f"missing dependency: {exc}") from exc

        if device == "cuda" and not torch.cuda.is_available():
            raise TtsError("internal", "cuda requested but torch.cuda.is_available() is False")

        log.info("loading %s on %s", model_name, device)
        try:
            model = ChatterboxMultilingualTTS.from_pretrained(device=torch.device(device))
        except RuntimeError as exc:
            msg = str(exc).lower()
            if "out of memory" in msg or ("cuda" in msg and "memory" in msg):
                raise TtsError("oom", str(exc)) from exc
            raise

        sr = int(getattr(model, "sr", 24000) or 24000)
        self.state.model = model
        self.state.model_name = model_name
        self.state.device = device
        self.state.sample_rate = sr
        return Response.success(
            req.id,
            {
                "model": model_name,
                "device": device,
                "sample_rate": sr,
                "vram_mb": _approx_vram_mb(),
            },
        )

    def _unload(self, req: Request) -> Response:
        self._drop_model()
        return Response.success(req.id, {})

    def _synthesize(self, req: Request) -> Response:
        if self.state.model is None:
            raise TtsError("not_loaded", "no TTS model loaded; call load first")

        from yapper_tts.sanitize import sanitize_for_tts

        text = sanitize_for_tts(str(req.params.get("text", "")))
        if not text:
            raise TtsError("bad_args", "params.text is required and must be non-empty")

        language = str(req.params.get("language", "en")).strip() or "en"
        if language not in ALLOWED_LANGUAGES:
            raise TtsError(
                "bad_args",
                f"language must be one of {sorted(ALLOWED_LANGUAGES)}, got {language!r}",
            )

        tone_name = str(req.params.get("tone", "neutral")).strip() or "neutral"
        voice = str(req.params.get("voice", "eve")).strip() or "eve"
        out_raw = req.params.get("out_path")
        if not out_raw:
            raise TtsError("bad_args", "params.out_path is required")
        out_path = Path(str(out_raw)).expanduser()
        out_path.parent.mkdir(parents=True, exist_ok=True)

        try:
            tone = resolve_tone(tone_name, voices_root=self.voices_root, voice=voice)
        except KeyError as exc:
            raise TtsError("bad_args", str(exc)) from exc
        except FileNotFoundError as exc:
            raise TtsError("bad_args", str(exc)) from exc

        import time

        log.info(
            "synthesize lang=%s tone=%s ref=%s chars=%d exg=%.3f cfg=%.3f",
            language,
            tone.name,
            tone.ref_wav,
            len(text),
            tone.exaggeration,
            tone.cfg_weight,
        )
        t0 = time.perf_counter()
        try:
            wav = self.state.model.generate(
                text,
                language_id=language,
                audio_prompt_path=str(tone.ref_wav),
                exaggeration=tone.exaggeration,
                cfg_weight=tone.cfg_weight,
            )
        except RuntimeError as exc:
            msg = str(exc).lower()
            if "out of memory" in msg or ("cuda" in msg and "memory" in msg):
                raise TtsError("oom", str(exc)) from exc
            raise

        sr = self.state.sample_rate or int(getattr(self.state.model, "sr", 24000) or 24000)
        gen_ms = (time.perf_counter() - t0) * 1000.0
        duration = _audio_duration_secs(wav, sr)
        log.info(
            "synth done chars=%d duration=%.3fs gen_ms=%.0f path=%s",
            len(text),
            duration,
            gen_ms,
            out_path,
        )

        # Output sanity: empty / near-silent / absurd duration → retry once with safer knobs.
        if not _output_sane(text, duration, wav):
            log.warning(
                "output sanity failed (duration=%.3fs chars=%d); retry with safer knobs",
                duration,
                len(text),
            )
            try:
                wav = self.state.model.generate(
                    text,
                    language_id=language,
                    audio_prompt_path=str(tone.ref_wav),
                    exaggeration=min(tone.exaggeration, 0.35),
                    cfg_weight=min(max(tone.cfg_weight, 0.3), 0.5),
                )
            except RuntimeError as exc:
                msg = str(exc).lower()
                if "out of memory" in msg or ("cuda" in msg and "memory" in msg):
                    raise TtsError("oom", str(exc)) from exc
                raise
            duration = _audio_duration_secs(wav, sr)
            if not _output_sane(text, duration, wav):
                raise TtsError(
                    "bad_output",
                    f"TTS output failed sanity (duration={duration:.3f}s for {len(text)} chars)",
                )

        _write_wav(out_path, wav, sr)
        return Response.success(
            req.id,
            {
                "path": str(out_path),
                "sample_rate": sr,
                "tone": tone.name,
                "language": language,
                "duration_secs": duration,
                "gen_ms": gen_ms,
            },
        )

    def _shutdown(self, req: Request) -> Response:
        self._drop_model()
        return Response.success(req.id, {"shutdown": True})

    def _drop_model(self) -> None:
        if self.state.model is None:
            return
        log.info("unloading TTS model=%s", self.state.model_name)
        self.state.model = None
        self.state.model_name = None
        self.state.device = None
        self.state.sample_rate = None
        gc.collect()
        try:
            import torch

            if torch.cuda.is_available():
                torch.cuda.empty_cache()
        except ImportError:
            pass


class TtsError(Exception):
    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


def _to_float_array(wav: Any) -> Any:
    import numpy as np

    if hasattr(wav, "detach"):
        arr = wav.detach().cpu().float().numpy()
    else:
        arr = np.asarray(wav, dtype=np.float32)
    arr = np.squeeze(arr)
    if arr.ndim > 1:
        arr = arr[0]
    return np.nan_to_num(arr, nan=0.0, posinf=0.0, neginf=0.0)


def _audio_duration_secs(wav: Any, sample_rate: int) -> float:
    import numpy as np

    arr = _to_float_array(wav)
    if arr.size == 0 or sample_rate <= 0:
        return 0.0
    return float(arr.size) / float(sample_rate)


def _output_sane(text: str, duration_secs: float, wav: Any) -> bool:
    """Reject empty, near-silent, or absurdly short/long audio relative to text."""
    import numpy as np

    chars = max(len(text.strip()), 1)
    if duration_secs <= 0.05:
        return False
    # ~12 chars/sec spoken upper bound → min duration; allow slack.
    min_dur = max(0.15, chars / 30.0 * 0.25)
    max_dur = max(3.0, chars / 8.0 * 4.0)  # very slow speech ceiling
    if duration_secs < min_dur or duration_secs > max_dur:
        return False
    arr = _to_float_array(wav)
    if arr.size == 0:
        return False
    peak = float(np.max(np.abs(arr)))
    if peak < 1e-4:
        return False  # near-silent
    return True


def _write_wav(path: Path, wav: Any, sample_rate: int) -> None:
    """Write tensor/ndarray audio to 16-bit PCM WAV (NaN-safe)."""
    import numpy as np

    arr = _to_float_array(wav)
    arr = np.clip(arr, -1.0, 1.0)
    pcm = (arr * 32767.0).astype(np.int16)
    with wave.open(str(path), "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sample_rate)
        wf.writeframes(pcm.tobytes())


def _approx_vram_mb() -> int | None:
    try:
        import torch

        if not torch.cuda.is_available():
            return None
        free, total = torch.cuda.mem_get_info()
        used = (total - free) // (1024 * 1024)
        return int(used)
    except Exception:  # noqa: BLE001
        return None
