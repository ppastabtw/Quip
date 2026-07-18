"""Provider-neutral Quip model benchmarking."""

from __future__ import annotations

import json
import os
import re
import statistics
import time
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence

import httpx

from environment import SYSTEM_PROMPT
from scoring import score_completion


FLASH_ROOT = Path(__file__).resolve().parent
REPO_ROOT = FLASH_ROOT.parents[1]
BACKBOARD_BASE_URL = "https://app.backboard.io/api"
BACKBOARD_MESSAGE_PATH = "/threads/messages"
BACKBOARD_MEMORY_MODE = "off"


@dataclass(frozen=True)
class ModelSpec:
    label: str
    transport: str
    model: str
    provider: str | None = None

    @property
    def artifact_id(self) -> str:
        value = re.sub(r"[^a-z0-9]+", "-", self.label.casefold()).strip("-")
        if not value:
            raise ValueError(f"model label cannot form an artifact id: {self.label}")
        return value


@dataclass(frozen=True)
class BenchmarkConfig:
    dataset: Path
    max_tokens: int
    timeout_seconds: float
    models: tuple[ModelSpec, ...]


@dataclass(frozen=True)
class ModelPrice:
    input_per_million: float
    output_per_million: float


def load_jsonl(path: Path) -> list[dict[str, Any]]:
    with path.open(encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def write_jsonl(path: Path, rows: Iterable[Mapping[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    with temporary.open("w", encoding="utf-8", newline="\n") as handle:
        for row in rows:
            handle.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")
    temporary.replace(path)


def _required_string(value: object, name: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{name} must be a non-empty string")
    return value.strip()


def load_config(path: Path) -> BenchmarkConfig:
    payload = tomllib.loads(path.read_text(encoding="utf-8"))
    benchmark = payload.get("benchmark")
    model_rows = payload.get("models")
    if not isinstance(benchmark, dict) or not isinstance(model_rows, list):
        raise ValueError("config must contain [benchmark] and [[models]]")

    dataset_value = _required_string(benchmark.get("dataset"), "benchmark.dataset")
    dataset = Path(dataset_value)
    if not dataset.is_absolute():
        dataset = FLASH_ROOT / dataset
    dataset = dataset.resolve()
    max_tokens = benchmark.get("max_tokens", 128)
    timeout_seconds = benchmark.get("timeout_seconds", 180)
    if isinstance(max_tokens, bool) or not isinstance(max_tokens, int) or not 16 <= max_tokens <= 512:
        raise ValueError("benchmark.max_tokens must be an integer from 16 to 512")
    if (
        isinstance(timeout_seconds, bool)
        or not isinstance(timeout_seconds, (int, float))
        or not 1 <= timeout_seconds <= 600
    ):
        raise ValueError("benchmark.timeout_seconds must be from 1 to 600")

    models: list[ModelSpec] = []
    labels: set[str] = set()
    artifact_ids: set[str] = set()
    for index, row in enumerate(model_rows, 1):
        if not isinstance(row, dict):
            raise ValueError(f"models entry {index} must be a table")
        if row.get("enabled", True) is False:
            continue
        label = _required_string(row.get("label"), f"models entry {index} label")
        transport = _required_string(
            row.get("transport", "freesolo"), f"models entry {index} transport"
        )
        model = _required_string(row.get("model"), f"models entry {index} model")
        provider_value = row.get("provider")
        provider = (
            _required_string(provider_value, f"models entry {index} provider")
            if provider_value is not None
            else None
        )
        unknown = set(row) - {"label", "transport", "model", "provider", "enabled"}
        if unknown:
            raise ValueError(
                f"models entry {index} has unsupported fields: {', '.join(sorted(unknown))}"
            )
        if label in labels:
            raise ValueError(f"duplicate model label: {label}")
        if transport not in {"freesolo", "backboard"}:
            raise ValueError(f"unsupported transport for {label}: {transport}")
        if transport == "backboard":
            if provider is None or provider == "backboard-provider":
                if "/" not in model:
                    raise ValueError(
                        f"Backboard model {label} must use provider/model slug format"
                    )
                provider, model = model.split("/", 1)
            elif model.startswith(provider + "/"):
                model = model[len(provider) + 1 :]
        elif provider is not None:
            raise ValueError(f"Freesolo model {label} must not declare provider")
        spec = ModelSpec(label, transport, model, provider)
        if spec.artifact_id in artifact_ids:
            raise ValueError(f"duplicate model artifact id: {spec.artifact_id}")
        labels.add(label)
        artifact_ids.add(spec.artifact_id)
        models.append(spec)
    if not models:
        raise ValueError("config must enable at least one model")
    if not dataset.is_file():
        raise FileNotFoundError(f"benchmark dataset not found: {dataset}")
    return BenchmarkConfig(dataset, max_tokens, float(timeout_seconds), tuple(models))


def select_models(
    models: Sequence[ModelSpec], labels: Sequence[str] | None
) -> tuple[ModelSpec, ...]:
    if not labels:
        return tuple(models)
    requested = set(labels)
    selected = tuple(
        model
        for model in models
        if model.label in requested or model.artifact_id in requested
    )
    matched = {value for model in selected for value in (model.label, model.artifact_id)}
    missing = sorted(requested - matched)
    if missing:
        raise ValueError(f"unknown model labels: {', '.join(missing)}")
    return selected


def validate_dataset(rows: Sequence[Mapping[str, Any]]) -> None:
    if not rows:
        raise ValueError("benchmark dataset is empty")
    example_ids: set[str] = set()
    for index, row in enumerate(rows, 1):
        try:
            example_id = row["metadata"]["example_id"]
            response_text = row["output"]
            result = score_completion(
                input_text=row["input"],
                expected_output=response_text,
                metadata=row["metadata"],
                response_text=response_text,
            )
        except (KeyError, TypeError, ValueError) as error:
            raise ValueError(f"invalid benchmark row {index}: {error}") from error
        if not isinstance(example_id, str) or not example_id:
            raise ValueError(f"benchmark row {index} has invalid example_id")
        if example_id in example_ids:
            raise ValueError(f"duplicate benchmark example_id: {example_id}")
        if not result.success:
            raise ValueError(f"benchmark row {index} gold output does not pass scoring")
        example_ids.add(example_id)


def freesolo_request_payload(
    row: Mapping[str, Any], model: str, max_tokens: int
) -> dict[str, Any]:
    return {
        "model": model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": row["input"]},
        ],
        "temperature": 0.0,
        "max_tokens": max_tokens,
        "chat_template_kwargs": {"enable_thinking": False},
    }


def backboard_request_payload(row: Mapping[str, Any], spec: ModelSpec) -> dict[str, Any]:
    if spec.provider is None:
        raise ValueError("Backboard provider is required")
    return {
        "content": row["input"],
        "system_prompt": SYSTEM_PROMPT,
        "llm_provider": spec.provider,
        "model_name": spec.model,
        "stream": False,
        "memory": BACKBOARD_MEMORY_MODE,
        "web_search": "off",
        "json_output": True,
    }


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


def _usage(payload: Mapping[str, Any], *keys: str) -> int | None:
    for key in keys:
        value = payload.get(key)
        if isinstance(value, int) and not isinstance(value, bool):
            return value
    return None


def estimate_cost(
    input_tokens: int | None,
    output_tokens: int | None,
    price: ModelPrice | None,
) -> float | None:
    if input_tokens is None or output_tokens is None or price is None:
        return None
    return round(
        input_tokens * price.input_per_million / 1_000_000
        + output_tokens * price.output_per_million / 1_000_000,
        8,
    )


class FreesoloTransport:
    def __init__(self, *, timeout_seconds: float) -> None:
        from flash.client.config import load_credentials
        from flash.serve.deploy import serving_openai_base_url

        _, api_key = load_credentials()
        if not api_key:
            raise RuntimeError("Flash login is required before benchmarking")
        self.client = httpx.Client(
            base_url=serving_openai_base_url(),
            headers={"Authorization": f"Bearer {api_key}"},
            timeout=timeout_seconds,
        )

    def close(self) -> None:
        self.client.close()

    def complete(
        self, row: Mapping[str, Any], spec: ModelSpec, max_tokens: int
    ) -> dict[str, Any]:
        started = time.perf_counter()
        response = self.client.post(
            "/chat/completions",
            json=freesolo_request_payload(row, spec.model, max_tokens),
        )
        response.raise_for_status()
        body = response.json()
        content = body["choices"][0]["message"]["content"]
        if not isinstance(content, str):
            raise ValueError("Freesolo response content must be a string")
        usage = body.get("usage") if isinstance(body.get("usage"), dict) else {}
        input_tokens = _usage(usage, "prompt_tokens", "input_tokens")
        output_tokens = _usage(usage, "completion_tokens", "output_tokens")
        return {
            "response": content,
            "latency_ms": round((time.perf_counter() - started) * 1000),
            "returned_provider": "freesolo",
            "returned_model": str(body.get("model", spec.model)),
            "status": "COMPLETED",
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "total_tokens": _usage(usage, "total_tokens"),
            "estimated_cost_usd": None,
        }


class BackboardTransport:
    def __init__(
        self,
        *,
        timeout_seconds: float,
        api_key: str | None = None,
        transport: httpx.BaseTransport | None = None,
        sleep: Any = time.sleep,
    ) -> None:
        self.api_key = api_key or os.environ.get("BACKBOARD_API_KEY") or dotenv_value(
            "BACKBOARD_API_KEY"
        )
        if not self.api_key:
            raise RuntimeError("BACKBOARD_API_KEY is required before benchmarking")
        self.client = httpx.Client(
            base_url=BACKBOARD_BASE_URL,
            headers={"X-API-Key": self.api_key},
            timeout=timeout_seconds,
            transport=transport,
        )
        self.sleep = sleep
        self.prices: dict[tuple[str, str], ModelPrice] = {}

    def close(self) -> None:
        self.client.close()

    def _request(
        self, method: str, url: str, *, json_payload: dict[str, Any] | None = None
    ) -> httpx.Response:
        if url == BACKBOARD_MESSAGE_PATH:
            json_payload = dict(json_payload or {})
            json_payload["memory"] = BACKBOARD_MEMORY_MODE
        for attempt in range(4):
            try:
                response = self.client.request(method, url, json=json_payload)
            except httpx.RequestError:
                if attempt == 3:
                    raise
                self.sleep(min(2**attempt, 4))
                continue
            if response.status_code == 429 or response.status_code >= 500:
                if attempt == 3:
                    response.raise_for_status()
                self.sleep(min(2**attempt, 4))
                continue
            response.raise_for_status()
            return response
        raise RuntimeError("Backboard request exhausted retries")

    def validate_models(self, specs: Sequence[ModelSpec]) -> None:
        if not specs:
            return
        response = self._request(
            "GET", "/models?model_type=llm&supports_json_output=true&limit=500"
        ).json()
        models = response.get("models") if isinstance(response, dict) else None
        if not isinstance(models, list):
            raise ValueError("Backboard model catalog response is invalid")
        catalog: dict[tuple[str, str], Mapping[str, Any]] = {}
        for item in models:
            if (
                isinstance(item, dict)
                and isinstance(item.get("provider"), str)
                and isinstance(item.get("name"), str)
            ):
                catalog[(item["provider"], item["name"])] = item
        missing = []
        for spec in specs:
            key = (str(spec.provider), spec.model)
            model = catalog.get(key)
            if model is None:
                missing.append(f"{key[0]}/{key[1]}")
                continue
            input_price = model.get("input_cost_per_1m_tokens")
            output_price = model.get("output_cost_per_1m_tokens")
            if isinstance(input_price, (int, float)) and isinstance(
                output_price, (int, float)
            ):
                self.prices[key] = ModelPrice(float(input_price), float(output_price))
        if missing:
            raise ValueError(
                "Backboard JSON-capable models not found: " + ", ".join(missing)
            )

    def complete(
        self, row: Mapping[str, Any], spec: ModelSpec, max_tokens: int
    ) -> dict[str, Any]:
        del max_tokens
        started = time.perf_counter()
        body = self._request(
            "POST", BACKBOARD_MESSAGE_PATH, json_payload=backboard_request_payload(row, spec)
        ).json()
        if not isinstance(body, dict):
            raise ValueError("Backboard response must be an object")
        status = body.get("status")
        if status != "COMPLETED":
            raise ValueError(f"Backboard response status is {status}")
        content = body.get("content")
        if not isinstance(content, str):
            raise ValueError("Backboard response content must be a string")
        returned_provider = body.get("model_provider")
        returned_model = body.get("model_name")
        if returned_provider != spec.provider or returned_model != spec.model:
            raise ValueError(
                "Backboard returned a model that differs from the requested model"
            )
        input_tokens = _usage(body, "input_tokens")
        output_tokens = _usage(body, "output_tokens")
        return {
            "response": content,
            "latency_ms": round((time.perf_counter() - started) * 1000),
            "returned_provider": returned_provider,
            "returned_model": returned_model,
            "status": status,
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "total_tokens": _usage(body, "total_tokens"),
            "estimated_cost_usd": estimate_cost(
                input_tokens,
                output_tokens,
                self.prices.get((str(spec.provider), spec.model)),
            ),
        }


def validate_freesolo_models(specs: Sequence[ModelSpec]) -> None:
    if not specs:
        return
    from flash.catalog import list_models

    catalog = {model.id for model in list_models()}
    missing = sorted(spec.model for spec in specs if spec.model not in catalog)
    if missing:
        raise ValueError("Freesolo models not found: " + ", ".join(missing))


def prediction_record(
    *,
    row: Mapping[str, Any],
    spec: ModelSpec,
    result: Mapping[str, Any] | None = None,
    error: Exception | None = None,
) -> dict[str, Any]:
    record: dict[str, Any] = {
        "example_id": row["metadata"]["example_id"],
        "label": spec.label,
        "transport": spec.transport,
        "requested_provider": spec.provider,
        "requested_model": spec.model,
    }
    if result is not None:
        record.update(result)
        record["error"] = None
    elif error is not None:
        record.update(
            {
                "response": None,
                "latency_ms": None,
                "returned_provider": None,
                "returned_model": None,
                "status": "ERROR",
                "input_tokens": None,
                "output_tokens": None,
                "total_tokens": None,
                "estimated_cost_usd": None,
                "error": f"{type(error).__name__}: {error}",
            }
        )
    else:
        raise ValueError("prediction record requires a result or error")
    return record


def _percentile(values: Sequence[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    index = round((len(ordered) - 1) * fraction)
    return round(ordered[index], 2)


def model_runtime_summary(predictions: Sequence[Mapping[str, Any]]) -> dict[str, Any]:
    latencies = [
        float(row["latency_ms"])
        for row in predictions
        if isinstance(row.get("latency_ms"), (int, float))
    ]
    costs = [
        float(row["estimated_cost_usd"])
        for row in predictions
        if isinstance(row.get("estimated_cost_usd"), (int, float))
    ]
    return {
        "requests": len(predictions),
        "errors": sum(row.get("error") is not None for row in predictions),
        "input_tokens": sum(
            int(row["input_tokens"])
            for row in predictions
            if isinstance(row.get("input_tokens"), int)
        ),
        "output_tokens": sum(
            int(row["output_tokens"])
            for row in predictions
            if isinstance(row.get("output_tokens"), int)
        ),
        "estimated_cost_usd": round(sum(costs), 8) if costs else None,
        "median_latency_ms": round(statistics.median(latencies), 2) if latencies else None,
        "p95_latency_ms": _percentile(latencies, 0.95),
    }


def markdown_report(summary: Mapping[str, Any]) -> str:
    lines = [
        "# Quip model benchmark",
        "",
        "| Model | Transport | Success | Decode | Unneeded edits | Schema | Mean ms | P95 ms | Errors | Est. USD |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for model in summary["models"]:
        metrics = model["metrics"]
        runtime = model["runtime"]
        cost = runtime["estimated_cost_usd"]
        lines.append(
            "| {label} | {transport} | {success:.1%} | {decode:.1%} | {edits:.1%} | "
            "{schema:.1%} | {mean} | {p95} | {errors} | {cost} |".format(
                label=model["label"],
                transport=model["transport"],
                success=metrics["overall_success"],
                decode=metrics["decode_success"],
                edits=metrics["unnecessary_edit_rate"],
                schema=metrics["schema_validity"],
                mean=metrics["mean_latency_ms"] if metrics["mean_latency_ms"] is not None else "n/a",
                p95=runtime["p95_latency_ms"] if runtime["p95_latency_ms"] is not None else "n/a",
                errors=runtime["errors"],
                cost=f"${cost:.6f}" if cost is not None else "n/a",
            )
        )
    return "\n".join(lines) + "\n"
