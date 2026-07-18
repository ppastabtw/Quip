"""Backboard teacher transport, cache, and response validation."""

from __future__ import annotations

import json
import os
import re
import time
from collections import Counter
from typing import Any, Sequence

import httpx

from .contract import (
    CONTRACT,
    REPO_ROOT,
    TEACHER_CACHE_DIR,
    BuildError,
    compact_json,
    sha256_bytes,
)


BACKBOARD_ENDPOINT = "https://app.backboard.io/api/threads/messages"
BACKBOARD_LLM_PROVIDER = "openrouter"
GENERATION_MODEL = "z-ai/glm-5.2"
VERIFICATION_MODEL = "openai/gpt-5.6-luna"

GENERATION_SYSTEM_PROMPT = """You create realistic short typing mistakes for a text decoder dataset.
For every target, return exactly one draft that a person might type when they intend the target.
The draft must contain a clear typo, phonetic spelling, or compressed shorthand.
It must preserve meaning, facts, tone, names, and numbers.
Use 2 to 5 whitespace-separated words. Do not add context, commentary, or alternatives.
For targets longer than 7 words, use exactly 5 draft words so the correction is not an excessive expansion.
Return only a JSON object with a drafts array. Each item must contain target_id and draft."""

VERIFICATION_SYSTEM_PROMPT = """You verify paired short drafts and intended targets for a conservative text decoder.
Accept only when the target is a plausible intentional casual message, the draft and target preserve meaning, facts, and tone, the draft is natural typed text, and the correction is minimal.
Intentional casual grammar and slang are valid. Reject obvious source errors, nonsense, and sentence fragments.
Reject semantic rewrites, invented facts, unnatural corruption, ambiguous intent, and excessive expansion.
Return only a JSON object with a verdicts array. Each item must contain target_id, target_intentional, meaning_preserved, facts_preserved, tone_preserved, natural, minimal, accepted, and reason.
accepted must be true only when all six checks are true."""


def dotenv_value(name: str) -> str | None:
    path = REPO_ROOT / ".env"
    if not path.is_file():
        return None
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        if key.strip() == name:
            return value.strip().strip('"').strip("'") or None
    return None


def model_for_phase(phase: str) -> str:
    if phase == "generation":
        return GENERATION_MODEL
    if phase == "verification":
        return VERIFICATION_MODEL
    raise BuildError(f"unsupported teacher phase: {phase}")


def build_request_payload(
    *, system_prompt: str, user_payload: dict[str, Any], phase: str
) -> dict[str, Any]:
    return {
        "content": compact_json(user_payload),
        "system_prompt": system_prompt,
        "llm_provider": BACKBOARD_LLM_PROVIDER,
        "model_name": model_for_phase(phase),
        "stream": False,
        "memory": "off",
        "web_search": "off",
        "json_output": True,
    }


class BackboardClient:
    def __init__(
        self,
        *,
        offline: bool,
        api_key: str | None = None,
        endpoint: str = BACKBOARD_ENDPOINT,
        transport: httpx.BaseTransport | None = None,
        sleep: Any = time.sleep,
    ) -> None:
        self.offline = offline
        self.api_key = api_key or os.environ.get("BACKBOARD_API_KEY") or dotenv_value(
            "BACKBOARD_API_KEY"
        )
        if endpoint != BACKBOARD_ENDPOINT:
            raise BuildError(f"unsupported Backboard endpoint: {endpoint}")
        self.endpoint = endpoint
        self.sleep = sleep
        self.request_count = 0
        self.network_request_count = 0
        self.retry_count = 0
        self.cache_hits = 0
        self.response_ids: list[str] = []
        self.returned_models: Counter[str] = Counter()
        self.statuses: Counter[str] = Counter()
        self.usage: Counter[str] = Counter()
        self.client = httpx.Client(transport=transport, timeout=120.0)

    def close(self) -> None:
        self.client.close()

    def _cached(
        self, request_hash: str, *, phase: str, model_name: str
    ) -> dict[str, Any] | None:
        path = TEACHER_CACHE_DIR / f"{request_hash}.json"
        if not path.is_file():
            return None
        envelope = json.loads(path.read_text(encoding="utf-8"))
        if envelope.get("request_hash") != request_hash:
            raise BuildError(f"teacher cache hash mismatch: {path.name}")
        if envelope.get("endpoint") != self.endpoint:
            raise BuildError(f"teacher cache endpoint mismatch: {path.name}")
        if envelope.get("phase") != phase or envelope.get("model_name") != model_name:
            raise BuildError(f"teacher cache model mismatch: {path.name}")
        self.cache_hits += 1
        self._record_response(
            response_id=str(envelope.get("response_id", "")),
            returned_model=str(envelope.get("returned_model", "")),
            status=str(envelope.get("status", "")),
            usage=envelope.get("usage") if isinstance(envelope.get("usage"), dict) else {},
        )
        return envelope

    def _record_response(
        self,
        *,
        response_id: str,
        returned_model: str,
        status: str,
        usage: dict[str, Any],
    ) -> None:
        if response_id:
            self.response_ids.append(response_id)
        if returned_model:
            self.returned_models[returned_model] += 1
        if status:
            self.statuses[status] += 1
        for key, value in usage.items():
            if isinstance(value, int):
                self.usage[key] += value

    def _post(self, request_payload: dict[str, Any]) -> dict[str, Any]:
        for attempt in range(4):
            if self.network_request_count >= CONTRACT.max_teacher_requests:
                raise BuildError(
                    f"teacher request cap of {CONTRACT.max_teacher_requests} exceeded"
                )
            self.network_request_count += 1
            response = self.client.post(
                self.endpoint,
                headers={"X-API-Key": self.api_key},
                json=request_payload,
            )
            retryable_http = response.status_code == 429 or response.status_code >= 500
            if retryable_http:
                if attempt == 3:
                    response.raise_for_status()
                self.retry_count += 1
                self.sleep(min(2**attempt, 4))
                continue
            response.raise_for_status()
            payload = response.json()
            if not isinstance(payload, dict):
                raise BuildError("invalid Backboard response envelope: expected object")
            if payload.get("status") == "FAILED":
                if attempt == 3:
                    raise BuildError("Backboard model run failed after three retries")
                self.retry_count += 1
                self.sleep(min(2**attempt, 4))
                continue
            return payload
        raise BuildError("Backboard request exhausted retries")

    def complete(
        self, *, system_prompt: str, user_payload: dict[str, Any], phase: str
    ) -> dict[str, Any]:
        self.request_count += 1
        if self.request_count > CONTRACT.max_teacher_requests:
            raise BuildError(f"teacher request cap of {CONTRACT.max_teacher_requests} exceeded")
        model_name = model_for_phase(phase)
        request_payload = build_request_payload(
            system_prompt=system_prompt,
            user_payload=user_payload,
            phase=phase,
        )
        request_hash = sha256_bytes(
            compact_json(
                {"endpoint": self.endpoint, "phase": phase, "request": request_payload}
            ).encode("utf-8")
        )
        cached = self._cached(request_hash, phase=phase, model_name=model_name)
        if cached is not None:
            return json.loads(cached["content"])
        if self.offline:
            raise BuildError(f"offline teacher cache miss: {request_hash}")
        if not self.api_key:
            raise BuildError("BACKBOARD_API_KEY is required for an online build")

        payload = self._post(request_payload)
        try:
            content = payload["content"]
            if payload["status"] != "COMPLETED":
                raise BuildError(f"Backboard response status is {payload['status']}")
            parsed = json.loads(content)
        except (KeyError, TypeError, json.JSONDecodeError) as exc:
            raise BuildError(f"invalid Backboard response envelope: {exc}") from exc

        response_id = str(payload.get("message_id", ""))
        returned_provider = str(payload.get("model_provider", ""))
        returned_model = str(payload.get("model_name", ""))
        if returned_provider != BACKBOARD_LLM_PROVIDER or returned_model != model_name:
            raise BuildError(
                "Backboard returned a model that differs from the pinned teacher model"
            )
        status = str(payload.get("status", ""))
        usage = {
            key: payload[key]
            for key in ("input_tokens", "output_tokens", "total_tokens")
            if isinstance(payload.get(key), int)
        }
        self._record_response(
            response_id=response_id,
            returned_model=returned_model,
            status=status,
            usage=usage,
        )

        envelope = {
            "request_hash": request_hash,
            "endpoint": self.endpoint,
            "phase": phase,
            "llm_provider": BACKBOARD_LLM_PROVIDER,
            "model_name": model_name,
            "request": request_payload,
            "response_id": response_id,
            "returned_model": returned_model,
            "status": status,
            "usage": usage,
            "content": content,
        }
        TEACHER_CACHE_DIR.mkdir(parents=True, exist_ok=True)
        (TEACHER_CACHE_DIR / f"{request_hash}.json").write_text(
            json.dumps(envelope, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
            encoding="utf-8",
        )
        return parsed

    def report(self) -> dict[str, Any]:
        return {
            "provider": "backboard",
            "endpoint": self.endpoint,
            "generation": {
                "llm_provider": BACKBOARD_LLM_PROVIDER,
                "model": GENERATION_MODEL,
                "prompt_hash": sha256_bytes(GENERATION_SYSTEM_PROMPT.encode("utf-8")),
            },
            "verification": {
                "llm_provider": BACKBOARD_LLM_PROVIDER,
                "model": VERIFICATION_MODEL,
                "prompt_hash": sha256_bytes(VERIFICATION_SYSTEM_PROMPT.encode("utf-8")),
            },
            "request_count": self.request_count,
            "network_request_count": self.network_request_count,
            "retries": self.retry_count,
            "cache_hits": self.cache_hits,
            "response_ids": self.response_ids,
            "returned_models": dict(sorted(self.returned_models.items())),
            "statuses": dict(sorted(self.statuses.items())),
            "token_usage": dict(sorted(self.usage.items())),
        }


def validate_generation_payload(payload: Any, target_ids: Sequence[str]) -> dict[str, str]:
    if not isinstance(payload, dict) or set(payload) != {"drafts"} or not isinstance(payload["drafts"], list):
        raise BuildError("teacher generation does not match committed schema")
    drafts: dict[str, str] = {}
    for item in payload["drafts"]:
        if not isinstance(item, dict) or set(item) != {"target_id", "draft"}:
            raise BuildError("teacher draft item does not match committed schema")
        target_id = item["target_id"]
        draft = item["draft"]
        if not isinstance(target_id, str) or not isinstance(draft, str) or not draft.strip():
            raise BuildError("teacher draft fields have invalid types")
        if target_id in drafts:
            raise BuildError(f"teacher returned duplicate target_id: {target_id}")
        drafts[target_id] = re.sub(r"\s+", " ", draft).strip()
    if set(drafts) != set(target_ids):
        raise BuildError("teacher generation target IDs do not match request")
    return drafts


def validate_verification_payload(
    payload: Any, target_ids: Sequence[str]
) -> dict[str, dict[str, Any]]:
    required = {
        "target_id",
        "target_intentional",
        "meaning_preserved",
        "facts_preserved",
        "tone_preserved",
        "natural",
        "minimal",
        "accepted",
        "reason",
    }
    if not isinstance(payload, dict) or set(payload) != {"verdicts"} or not isinstance(payload["verdicts"], list):
        raise BuildError("teacher verification does not match committed schema")
    verdicts: dict[str, dict[str, Any]] = {}
    for item in payload["verdicts"]:
        if not isinstance(item, dict) or set(item) != required:
            raise BuildError("teacher verdict item does not match committed schema")
        target_id = item["target_id"]
        if not isinstance(target_id, str) or target_id in verdicts:
            raise BuildError("teacher verdict target_id is invalid or duplicated")
        checks = [
            item["target_intentional"],
            item["meaning_preserved"],
            item["facts_preserved"],
            item["tone_preserved"],
            item["natural"],
            item["minimal"],
            item["accepted"],
        ]
        if not all(isinstance(value, bool) for value in checks) or not isinstance(item["reason"], str):
            raise BuildError("teacher verdict fields have invalid types")
        if item["accepted"] != all(checks[:6]):
            raise BuildError("teacher accepted flag disagrees with verification checks")
        verdicts[target_id] = item
    if set(verdicts) != set(target_ids):
        raise BuildError("teacher verification target IDs do not match request")
    return verdicts
