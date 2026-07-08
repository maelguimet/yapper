"""JSON-lines IPC envelopes (proto v1). See docs/ipc.md."""

from __future__ import annotations

from dataclasses import asdict, dataclass, field
from typing import Any, Mapping

PROTO = 1


@dataclass
class ErrorBody:
    code: str
    message: str

    def to_dict(self) -> dict[str, str]:
        return {"code": self.code, "message": self.message}


@dataclass
class Request:
    id: str
    cmd: str
    params: dict[str, Any] = field(default_factory=dict)
    proto: int = PROTO

    @classmethod
    def from_dict(cls, data: Mapping[str, Any]) -> Request:
        return cls(
            id=str(data["id"]),
            cmd=str(data["cmd"]),
            params=dict(data.get("params") or {}),
            proto=int(data.get("proto", PROTO)),
        )


@dataclass
class Response:
    id: str
    ok: bool
    result: dict[str, Any] = field(default_factory=dict)
    error: ErrorBody | None = None

    def to_dict(self) -> dict[str, Any]:
        out: dict[str, Any] = {"id": self.id, "ok": self.ok}
        if self.ok:
            out["result"] = self.result
        if self.error is not None:
            out["error"] = self.error.to_dict()
        return out

    @classmethod
    def success(cls, req_id: str, result: dict[str, Any] | None = None) -> Response:
        return cls(id=req_id, ok=True, result=result or {})

    @classmethod
    def failure(cls, req_id: str, code: str, message: str) -> Response:
        return cls(id=req_id, ok=False, error=ErrorBody(code=code, message=message))


def dumps_response(resp: Response) -> str:
    import json

    return json.dumps(resp.to_dict(), ensure_ascii=False)


def loads_request(line: str) -> Request:
    import json

    data = json.loads(line)
    if not isinstance(data, dict):
        raise ValueError("request must be a JSON object")
    return Request.from_dict(data)


# silence unused import lint for asdict if helpers grow
_ = asdict
