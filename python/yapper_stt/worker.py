"""STT worker command handlers (Whisper). Process stays up across unload."""

from __future__ import annotations

import gc
import logging
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal

from yapper_common.ipc import Request, Response
from yapper_common.paths import ensure_runtime_dirs, whisper_models_dir

log = logging.getLogger("yapper.stt")

STT_VERSION = "0.1.0"
ALLOWED_MODELS = frozenset({"small", "medium"})
ALLOWED_LANGUAGES = frozenset({"auto", "en", "fr"})
DeviceName = Literal["cuda", "cpu"]


@dataclass
class SttState:
    model: Any = None
    model_name: str | None = None
    device: str | None = None


@dataclass
class SttWorker:
    """In-process STT state + command dispatch. One model size at a time."""

    state: SttState = field(default_factory=SttState)
    download_root: Path | None = None

    def whisper_root(self) -> Path:
        """Configured Whisper download/load root (``YAPPER_MODELS_DIR``/whisper or XDG)."""
        return self.download_root or whisper_models_dir()

    def handle(self, req: Request) -> Response:
        handlers = {
            "ping": self._ping,
            "status": self._status,
            "load": self._load,
            "unload": self._unload,
            "transcribe": self._transcribe,
            "shutdown": self._shutdown,
        }
        handler = handlers.get(req.cmd)
        if handler is None:
            return Response.failure(req.id, "bad_args", f"unknown cmd: {req.cmd!r}")
        try:
            return handler(req)
        except SttError as exc:
            return Response.failure(req.id, exc.code, exc.message)
        except Exception as exc:  # noqa: BLE001 — worker boundary
            log.exception("internal error on %s", req.cmd)
            return Response.failure(req.id, "internal", str(exc))

    def _ping(self, req: Request) -> Response:
        return Response.success(
            req.id,
            {"role": "stt", "version": STT_VERSION, "proto": 1},
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

    def _load(self, req: Request) -> Response:
        model_name = str(req.params.get("model", "")).strip()
        if model_name not in ALLOWED_MODELS:
            raise SttError(
                "bad_args",
                f"model must be one of {sorted(ALLOWED_MODELS)}, got {model_name!r}",
            )
        device = str(req.params.get("device", "cuda")).strip() or "cuda"
        if device not in ("cuda", "cpu"):
            raise SttError("bad_args", f"device must be cuda|cpu, got {device!r}")

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

        ensure_runtime_dirs()
        root = self.whisper_root()
        root.mkdir(parents=True, exist_ok=True)

        try:
            import torch
            import whisper
        except ImportError as exc:
            raise SttError("internal", f"missing dependency: {exc}") from exc

        if device == "cuda" and not torch.cuda.is_available():
            raise SttError("internal", "cuda requested but torch.cuda.is_available() is False")

        log.info("loading whisper model=%s device=%s root=%s", model_name, device, root)
        try:
            model = whisper.load_model(model_name, device=device, download_root=str(root))
        except RuntimeError as exc:
            msg = str(exc).lower()
            if "out of memory" in msg or "cuda" in msg and "memory" in msg:
                raise SttError("oom", str(exc)) from exc
            raise

        self.state.model = model
        self.state.model_name = model_name
        self.state.device = device
        return Response.success(
            req.id,
            {
                "model": model_name,
                "device": device,
                "vram_mb": _approx_vram_mb(),
            },
        )

    def _unload(self, req: Request) -> Response:
        self._drop_model()
        return Response.success(req.id, {})

    def _transcribe(self, req: Request) -> Response:
        if self.state.model is None:
            raise SttError("not_loaded", "no STT model loaded; call load first")

        path_raw = req.params.get("path")
        if not path_raw:
            raise SttError("bad_args", "params.path is required")
        path = Path(str(path_raw)).expanduser()
        if not path.is_file():
            raise SttError("bad_args", f"audio file not found: {path}")

        language = str(req.params.get("language", "auto")).strip() or "auto"
        if language not in ALLOWED_LANGUAGES:
            raise SttError(
                "bad_args",
                f"language must be one of {sorted(ALLOWED_LANGUAGES)}, got {language!r}",
            )

        whisper_lang = None if language == "auto" else language
        # fp16 on CUDA is faster; force False only on CPU.
        use_fp16 = (self.state.device or "") == "cuda"
        result = self.state.model.transcribe(
            str(path), language=whisper_lang, fp16=use_fp16
        )
        text = str(result.get("text", "")).strip()
        detected = result.get("language") or language
        return Response.success(
            req.id,
            {
                "text": text,
                "language": str(detected),
            },
        )

    def _shutdown(self, req: Request) -> Response:
        self._drop_model()
        return Response.success(req.id, {"shutdown": True})

    def _drop_model(self) -> None:
        if self.state.model is None:
            return
        log.info("unloading whisper model=%s", self.state.model_name)
        self.state.model = None
        self.state.model_name = None
        self.state.device = None
        gc.collect()
        try:
            import torch

            if torch.cuda.is_available():
                torch.cuda.empty_cache()
        except ImportError:
            pass


class SttError(Exception):
    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


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
