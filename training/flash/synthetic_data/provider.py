"""Backboard and deterministic mock structured-output clients."""

from __future__ import annotations

import json
import random
import threading
import time
from dataclasses import dataclass
from typing import Any, Protocol

import httpx

from benchmarking import BackboardTransport, ModelSpec, _usage, estimate_cost

from .config import ModelConfig


@dataclass(frozen=True)
class Completion:
    content: str
    provider: str
    model: str
    input_tokens: int | None
    output_tokens: int | None
    total_tokens: int | None
    estimated_cost_usd: float | None
    latency_ms: int


class StructuredClient(Protocol):
    def validate_models(self, configs: list[ModelConfig]) -> None: ...
    def complete(self, *, system_prompt: str, user_prompt: str, config: ModelConfig) -> Completion: ...
    def close(self) -> None: ...


class RateLimiter:
    def __init__(self, requests_per_minute: int, *, clock: Any = time.monotonic, sleep: Any = time.sleep) -> None:
        self.interval = 60.0 / requests_per_minute
        self.clock = clock
        self.sleep = sleep
        self.lock = threading.Lock()
        self.next_at = 0.0

    def wait(self) -> None:
        with self.lock:
            now = self.clock()
            delay = max(0.0, self.next_at - now)
            self.next_at = max(now, self.next_at) + self.interval
        if delay:
            self.sleep(delay)


class BackboardStructuredClient:
    """Stateless Backboard calls using the repository's existing transport."""

    def __init__(
        self,
        *,
        timeout_seconds: float,
        requests_per_minute: int,
        api_key: str | None = None,
        transport: httpx.BaseTransport | None = None,
        sleep: Any = time.sleep,
    ) -> None:
        self.transport = BackboardTransport(
            timeout_seconds=timeout_seconds,
            api_key=api_key,
            transport=transport,
            sleep=sleep,
        )
        self.rate_limiter = RateLimiter(requests_per_minute)

    def close(self) -> None:
        self.transport.close()

    def validate_models(self, configs: list[ModelConfig]) -> None:
        specs = [ModelSpec(item.slug, "backboard", item.model, item.provider) for item in configs]
        self.transport.validate_models(specs)

    def complete(self, *, system_prompt: str, user_prompt: str, config: ModelConfig) -> Completion:
        self.rate_limiter.wait()
        started = time.perf_counter()
        payload = {
            "content": user_prompt,
            "system_prompt": system_prompt,
            "llm_provider": config.provider,
            "model_name": config.model,
            "stream": False,
            "memory": "off",
            "web_search": "off",
            "json_output": True,
            "temperature": config.temperature,
            "max_tokens": config.max_tokens,
        }
        body = self.transport._request("POST", "/threads/messages", json_payload=payload).json()
        if not isinstance(body, dict) or body.get("status") != "COMPLETED":
            raise ValueError(f"Backboard response status is {body.get('status') if isinstance(body, dict) else 'invalid'}")
        content = body.get("content")
        if not isinstance(content, str):
            raise ValueError("Backboard response content must be a string")
        if body.get("model_provider") != config.provider or body.get("model_name") != config.model:
            raise ValueError("Backboard returned a model different from the requested model")
        input_tokens = _usage(body, "input_tokens")
        output_tokens = _usage(body, "output_tokens")
        price = self.transport.prices.get((config.provider, config.model))
        return Completion(
            content=content,
            provider=config.provider,
            model=config.model,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            total_tokens=_usage(body, "total_tokens"),
            estimated_cost_usd=estimate_cost(input_tokens, output_tokens, price),
            latency_ms=round((time.perf_counter() - started) * 1000),
        )


class MockStructuredClient:
    """Paid-API-free fixture client for tests and workflow smoke runs."""

    PEOPLE = ("Siobhan", "Amara", "Mateo", "Ji-woo", "O'Connor")
    PLACES = ("Union Station", "Pearson Airport", "Canoe Restaurant", "Room 204", "Gate B17")
    APPS = ("Notes", "Messages", "Calendar", "Gmail", "Microsoft Word", "Maps")

    def validate_models(self, configs: list[ModelConfig]) -> None:
        del configs

    def close(self) -> None:
        return None

    def complete(self, *, system_prompt: str, user_prompt: str, config: ModelConfig) -> Completion:
        del system_prompt, config
        started = time.perf_counter()
        request = json.loads(user_prompt)
        if request.get("task") == "generate":
            content = json.dumps({"candidates": [self._candidate(slot, index) for index, slot in enumerate(request["slots"])]})
        elif request.get("task") == "judge":
            content = json.dumps({"judgments": [self._judgment(row) for row in request["candidates"]]})
        else:
            raise ValueError("unknown mock task")
        return Completion(
            content=content,
            provider="mock",
            model="deterministic-v1",
            input_tokens=len(user_prompt.split()),
            output_tokens=len(content.split()),
            total_tokens=len(user_prompt.split()) + len(content.split()),
            estimated_cost_usd=0.0,
            latency_ms=round((time.perf_counter() - started) * 1000),
        )

    def _candidate(self, slot: dict[str, Any], index: int) -> dict[str, Any]:
        rng = random.Random(f"{slot['slot_id']}:{index}")
        person = self.PEOPLE[rng.randrange(len(self.PEOPLE))]
        place = self.PLACES[rng.randrange(len(self.PLACES))]
        app = self.APPS[rng.randrange(len(self.APPS))]
        behavior = slot["context_behavior"]
        variant = slot.get("variant")
        group_id = slot.get("group_id")
        if group_id:
            draft = "meet there tmrw"
            if variant == "context_a":
                place, suggestion, visible = "Union Station", "meet at Union Station tomorrow", "Tomorrow: meet at Union Station."
            elif variant == "context_b":
                place, suggestion, visible = "Pearson Airport", "meet at Pearson Airport tomorrow", "Tomorrow: meet at Pearson Airport."
            elif variant == "no_context":
                suggestion, visible = "meet there tomorrow", ""
            elif variant == "irrelevant":
                suggestion, visible = "meet there tomorrow", "Laptop warranty expires Friday."
            elif variant == "ambiguous":
                suggestion, visible = "meet there tomorrow", "Lunch at Alo. Drinks at Bar Raval."
            else:
                draft, suggestion, visible = "actually meet there at 4", "actually meet there at 4", "Meeting at 3 PM."
        elif behavior == "useful":
            draft, suggestion, visible = "tell shivon ill call", f"tell {person} I'll call", f"Contact: {person}. Follow-up call today."
        elif behavior == "irrelevant":
            draft, suggestion, visible = "my laptop is brokn", "my laptop is broken", f"Tomorrow: meet at {place}."
        elif behavior == "ambiguous":
            draft, suggestion, visible = "send them the file", "send them the file", "Alex handles design. Jordan handles billing."
        elif behavior == "conflicting":
            draft, suggestion, visible = "actually lets meet at 4", "actually let's meet at 4", "Meeting at 3 PM."
        else:
            draft, suggestion, visible = "cnt cm tmrw", "can't come tomorrow", ""
        context = [] if behavior == "none" else [{"app_name": app, "window_title": f"{person} — plans", "visible_text": visible}]
        return {
            **slot,
            "domain": "everyday_planning",
            "error_type": "compressed_wording" if "tmrw" in draft else "phonetic",
            "writing_style": "lowercase_texting",
            "input": {"text": draft, **({"context_snippets": context} if context else {})},
            "output": {"suggestion": suggestion},
            "rationale": "Deterministic mock fixture for pipeline validation; not a quality training sample.",
        }

    def _judgment(self, candidate: dict[str, Any]) -> dict[str, Any]:
        return {
            "candidate_id": candidate["candidate_id"],
            "pass": True,
            "scores": {
                "correctness": 5,
                "context_grounding": 5,
                "context_usefulness": 5,
                "meaning_preservation": 5,
                "tone_preservation": 5,
                "minimality": 5,
                "naturalness": 4,
                "category_validity": 4,
                "dataset_value": 4,
            },
            "failure_reasons": [],
            "notes": "mock judgment",
        }
