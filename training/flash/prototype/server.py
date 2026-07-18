"""Serve a local-only page for probing a base model or deployed Flash adapter."""

from __future__ import annotations

import argparse
import json
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

import httpx
from flash.client.config import load_credentials
from flash.serve.deploy import serving_openai_base_url


FLASH_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(FLASH_ROOT))

from environment import SYSTEM_PROMPT  # noqa: E402
from scoring import OUTPUT_JSON_SCHEMA, parse_prediction  # noqa: E402


INDEX_PATH = Path(__file__).with_name("index.html")
DEFAULT_MODEL = "Qwen/Qwen3.5-2B"
MAX_REQUEST_BYTES = 32_768
MAX_DRAFT_CHARS = 4_000
MAX_SYSTEM_PROMPT_CHARS = 8_000
MAX_SUGGESTIONS = 5


class PlaygroundError(Exception):
    def __init__(self, status: HTTPStatus, message: str) -> None:
        super().__init__(message)
        self.status = status


def _required_text(payload: dict[str, Any], key: str, max_chars: int) -> str:
    value = payload.get(key)
    if not isinstance(value, str) or not value.strip():
        raise PlaygroundError(HTTPStatus.BAD_REQUEST, f"{key} must be a non-empty string")
    if len(value) > max_chars:
        raise PlaygroundError(
            HTTPStatus.BAD_REQUEST,
            f"{key} must be at most {max_chars} characters",
        )
    return value


def _number(
    payload: dict[str, Any],
    key: str,
    default: int | float,
    minimum: int | float,
    maximum: int | float,
    *,
    integer: bool = False,
) -> int | float:
    value = payload.get(key, default)
    expected_type = int if integer else (int, float)
    if isinstance(value, bool) or not isinstance(value, expected_type):
        kind = "an integer" if integer else "a number"
        raise PlaygroundError(HTTPStatus.BAD_REQUEST, f"{key} must be {kind}")
    if not minimum <= value <= maximum:
        raise PlaygroundError(
            HTTPStatus.BAD_REQUEST,
            f"{key} must be between {minimum} and {maximum}",
        )
    return value


def validate_request(payload: Any) -> tuple[str, dict[str, Any], dict[str, Any]]:
    if not isinstance(payload, dict):
        raise PlaygroundError(HTTPStatus.BAD_REQUEST, "request body must be a JSON object")
    model = _required_text(payload, "model", 200)
    system_prompt = _required_text(payload, "system_prompt", MAX_SYSTEM_PROMPT_CHARS)
    draft = _required_text(payload, "draft", MAX_DRAFT_CHARS)
    model_input = {"text": draft}
    settings = {
        "system_prompt": system_prompt,
        "temperature": float(_number(payload, "temperature", 0.7, 0.0, 2.0)),
        "max_tokens": int(_number(payload, "max_tokens", 128, 16, 512, integer=True)),
        "suggestion_count": int(
            _number(payload, "suggestion_count", 3, 1, MAX_SUGGESTIONS, integer=True)
        ),
    }
    return model, model_input, settings


def request_payload(
    model: str,
    model_input: dict[str, Any],
    *,
    system_prompt: str = SYSTEM_PROMPT,
    temperature: float = 0.7,
    max_tokens: int = 128,
) -> dict[str, Any]:
    return {
        "model": model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": json.dumps(model_input, ensure_ascii=False)},
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
        "response_format": {
            "type": "json_schema",
            "json_schema": {"schema": OUTPUT_JSON_SCHEMA},
        },
        "chat_template_kwargs": {"enable_thinking": False},
    }


def _run_suggestion(
    model: str,
    model_input: dict[str, Any],
    settings: dict[str, Any],
    headers: dict[str, str],
    index: int,
) -> dict[str, Any]:
    started = time.perf_counter()
    try:
        with httpx.Client(
            base_url=serving_openai_base_url(),
            headers=headers,
            timeout=180.0,
        ) as client:
            response = client.post(
                "/chat/completions",
                json=request_payload(
                    model,
                    model_input,
                    system_prompt=settings["system_prompt"],
                    temperature=settings["temperature"],
                    max_tokens=settings["max_tokens"],
                ),
            )
            response.raise_for_status()
    except httpx.TimeoutException as error:
        raise PlaygroundError(HTTPStatus.GATEWAY_TIMEOUT, "Freesolo serving timed out") from error
    except httpx.HTTPStatusError as error:
        status_code = error.response.status_code
        raise PlaygroundError(
            HTTPStatus.BAD_GATEWAY,
            f"Freesolo serving returned HTTP {status_code}. Check the model ID and deployment.",
        ) from error
    except httpx.HTTPError as error:
        raise PlaygroundError(HTTPStatus.BAD_GATEWAY, "Freesolo serving is unreachable") from error

    latency_ms = round((time.perf_counter() - started) * 1000)
    body = response.json()
    try:
        raw = body["choices"][0]["message"]["content"]
        if not isinstance(raw, str):
            raise TypeError
        prediction = parse_prediction(raw)
    except (KeyError, IndexError, TypeError, ValueError, json.JSONDecodeError) as error:
        raise PlaygroundError(
            HTTPStatus.BAD_GATEWAY,
            "The model returned a response outside the Quip JSON contract",
        ) from error

    usage = body.get("usage") if isinstance(body.get("usage"), dict) else {}
    return {
        "index": index,
        "latency_ms": latency_ms,
        "suggestion": prediction.suggestion,
        "changed": prediction.suggestion != model_input["text"],
        "raw": raw,
        "usage": {
            "prompt_tokens": usage.get("prompt_tokens"),
            "completion_tokens": usage.get("completion_tokens"),
        },
    }


def run_prediction(payload: Any) -> dict[str, Any]:
    model, model_input, settings = validate_request(payload)
    _, api_key = load_credentials()
    if not api_key:
        raise PlaygroundError(
            HTTPStatus.SERVICE_UNAVAILABLE,
            "Flash login is required inside Ubuntu WSL2 before testing",
        )

    headers = {"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"}
    started = time.perf_counter()
    with ThreadPoolExecutor(max_workers=settings["suggestion_count"]) as pool:
        futures = [
            pool.submit(_run_suggestion, model, model_input, settings, headers, index)
            for index in range(1, settings["suggestion_count"] + 1)
        ]
        suggestions = [future.result() for future in futures]

    return {
        "model": model,
        "latency_ms": round((time.perf_counter() - started) * 1000),
        "settings": {
            "temperature": settings["temperature"],
            "max_tokens": settings["max_tokens"],
            "suggestion_count": settings["suggestion_count"],
        },
        "suggestions": suggestions,
    }


class PlaygroundHandler(BaseHTTPRequestHandler):
    server_version = "QuipPlayground/0"

    def _send_json(self, status: HTTPStatus, payload: dict[str, Any]) -> None:
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802
        if self.path == "/health":
            self._send_json(HTTPStatus.OK, {"status": "ready"})
            return
        if self.path == "/api/config":
            self._send_json(
                HTTPStatus.OK,
                {"default_model": DEFAULT_MODEL, "system_prompt": SYSTEM_PROMPT},
            )
            return
        if self.path not in ("/", "/index.html"):
            self.send_error(HTTPStatus.NOT_FOUND)
            return
        body = INDEX_PATH.read_bytes()
        self.send_response(HTTPStatus.OK)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Content-Security-Policy", "default-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self) -> None:  # noqa: N802
        if self.path != "/api/predict":
            self.send_error(HTTPStatus.NOT_FOUND)
            return
        try:
            content_length = int(self.headers.get("Content-Length", "0"))
            if content_length <= 0 or content_length > MAX_REQUEST_BYTES:
                raise PlaygroundError(HTTPStatus.BAD_REQUEST, "request body size is invalid")
            payload = json.loads(self.rfile.read(content_length))
            self._send_json(HTTPStatus.OK, run_prediction(payload))
        except json.JSONDecodeError:
            self._send_json(HTTPStatus.BAD_REQUEST, {"error": "request body must be valid JSON"})
        except PlaygroundError as error:
            self._send_json(error.status, {"error": str(error)})
        except Exception as error:  # pragma: no cover - final boundary protection
            print(f"unexpected {type(error).__name__}: {error}", file=sys.stderr)
            self._send_json(HTTPStatus.INTERNAL_SERVER_ERROR, {"error": "unexpected server error"})

    def log_message(self, format_string: str, *args: Any) -> None:
        print(f"{self.address_string()} {format_string % args}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8765)
    args = parser.parse_args()
    if args.host not in {"127.0.0.1", "localhost"}:
        parser.error("the prototype server only binds to localhost")
    server = ThreadingHTTPServer((args.host, args.port), PlaygroundHandler)
    print(f"Quip model playground: http://{args.host}:{args.port}")
    print("Press Ctrl+C to stop")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
