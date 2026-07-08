"""IPC envelope encode/decode round-trips on shipped modules."""

from __future__ import annotations

import json

import pytest

from yapper_common.ipc import PROTO, Request, Response, dumps_response, loads_request


def test_request_round_trip_defaults() -> None:
    line = json.dumps({"id": "1", "cmd": "ping"})
    req = loads_request(line)
    assert req.id == "1"
    assert req.cmd == "ping"
    assert req.params == {}
    assert req.proto == PROTO


def test_request_with_params_and_proto() -> None:
    line = json.dumps(
        {
            "id": "abc",
            "cmd": "load",
            "params": {"model": "small", "device": "cuda"},
            "proto": 1,
        }
    )
    req = loads_request(line)
    assert req.params["model"] == "small"
    assert req.proto == 1


def test_success_response_json_shape() -> None:
    resp = Response.success("42", {"role": "stt"})
    data = json.loads(dumps_response(resp))
    assert data == {"id": "42", "ok": True, "result": {"role": "stt"}}
    assert "error" not in data


def test_failure_response_json_shape() -> None:
    resp = Response.failure("9", "not_loaded", "load first")
    data = json.loads(dumps_response(resp))
    assert data["ok"] is False
    assert data["error"]["code"] == "not_loaded"
    assert "load first" in data["error"]["message"]
    assert "result" not in data


def test_loads_request_rejects_non_object() -> None:
    with pytest.raises(ValueError, match="JSON object"):
        loads_request("[1,2,3]")


def test_loads_request_requires_id_and_cmd() -> None:
    with pytest.raises(KeyError):
        loads_request(json.dumps({"cmd": "ping"}))
