"""TOML configuration loading and validation for synthetic runs."""

from __future__ import annotations

import tomllib
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Mapping, Sequence

from .models import CATEGORIES, CONTEXT_BEHAVIORS


ROOT = Path(__file__).resolve().parents[1]


def _number(value: object, name: str, *, minimum: float, maximum: float) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not minimum <= value <= maximum:
        raise ValueError(f"{name} must be from {minimum} to {maximum}")
    return float(value)


def _integer(value: object, name: str, *, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= maximum:
        raise ValueError(f"{name} must be an integer from {minimum} to {maximum}")
    return value


def _string(value: object, name: str, *, allow_empty: bool = False) -> str:
    if not isinstance(value, str) or (not allow_empty and not value.strip()):
        raise ValueError(f"{name} must be a {'string' if allow_empty else 'non-empty string'}")
    return value.strip()


def _weights(value: object, allowed: Sequence[str], name: str) -> dict[str, float]:
    if not isinstance(value, Mapping) or set(value) != set(allowed):
        raise ValueError(f"{name} must define exactly: {', '.join(allowed)}")
    result = {
        key: _number(value[key], f"{name}.{key}", minimum=0, maximum=1)
        for key in allowed
    }
    total = sum(result.values())
    if not 0.999 <= total <= 1.001:
        raise ValueError(f"{name} weights must sum to 1, got {total}")
    return result


@dataclass(frozen=True)
class ModelConfig:
    provider: str
    model: str
    temperature: float
    max_tokens: int

    @property
    def slug(self) -> str:
        return f"{self.provider}/{self.model}" if self.provider and self.model else "mock"


@dataclass(frozen=True)
class ThresholdConfig:
    minimum_score: int
    minimum_dataset_value: int
    require_pass_flag: bool


@dataclass(frozen=True)
class RunConfig:
    target_count: int
    batch_size: int
    judge_batch_size: int
    concurrency: int
    requests_per_minute: int
    seed: int
    oversample_factor: float
    max_generation_rounds: int
    contrast_share: float
    min_words: int
    max_words: int
    max_draft_chars: int
    max_context_snippets: int
    max_context_chars: int
    near_duplicate_threshold: float
    timeout_seconds: float
    max_attempts: int


@dataclass(frozen=True)
class SyntheticConfig:
    path: Path
    run: RunConfig
    generator: ModelConfig
    judge: ModelConfig
    thresholds: ThresholdConfig
    behaviors: dict[str, float]
    categories: dict[str, float]

    def with_overrides(
        self,
        *,
        count: int | None = None,
        categories: Sequence[str] | None = None,
        generator_model: str | None = None,
        judge_model: str | None = None,
    ) -> "SyntheticConfig":
        run = replace(self.run, target_count=count) if count is not None else self.run
        category_weights = self.categories
        if categories:
            unknown = set(categories) - set(CATEGORIES)
            if unknown:
                raise ValueError("unknown categories: " + ", ".join(sorted(unknown)))
            category_weights = {name: (1 / len(categories) if name in categories else 0.0) for name in CATEGORIES}
        generator = _override_model(self.generator, generator_model, "generator")
        judge = _override_model(self.judge, judge_model, "judge")
        return replace(self, run=run, categories=category_weights, generator=generator, judge=judge)


def _override_model(config: ModelConfig, value: str | None, name: str) -> ModelConfig:
    if value is None:
        return config
    if "/" not in value:
        raise ValueError(f"{name} model must use provider/model format")
    provider, model = value.split("/", 1)
    return replace(config, provider=_string(provider, f"{name}.provider"), model=_string(model, f"{name}.model"))


def _model(value: object, name: str) -> ModelConfig:
    if not isinstance(value, Mapping) or set(value) != {"provider", "model", "temperature", "max_tokens"}:
        raise ValueError(f"[{name}] has invalid keys")
    return ModelConfig(
        provider=_string(value["provider"], f"{name}.provider", allow_empty=True),
        model=_string(value["model"], f"{name}.model", allow_empty=True),
        temperature=_number(value["temperature"], f"{name}.temperature", minimum=0, maximum=2),
        max_tokens=_integer(value["max_tokens"], f"{name}.max_tokens", minimum=128, maximum=32000),
    )


def load_config(path: Path) -> SyntheticConfig:
    payload = tomllib.loads(path.read_text(encoding="utf-8"))
    expected = {"run", "generator", "judge", "thresholds", "behaviors", "categories"}
    if set(payload) != expected:
        raise ValueError("config must contain exactly: " + ", ".join(sorted(expected)))
    run = payload["run"]
    expected_run = {
        "target_count", "batch_size", "judge_batch_size", "concurrency",
        "requests_per_minute", "seed", "oversample_factor", "max_generation_rounds",
        "contrast_share", "min_words", "max_words", "max_draft_chars",
        "max_context_snippets", "max_context_chars", "near_duplicate_threshold",
        "timeout_seconds", "max_attempts",
    }
    if not isinstance(run, Mapping) or set(run) != expected_run:
        raise ValueError("[run] has invalid keys")
    run_config = RunConfig(
        target_count=_integer(run["target_count"], "run.target_count", minimum=1, maximum=1_000_000),
        batch_size=_integer(run["batch_size"], "run.batch_size", minimum=1, maximum=50),
        judge_batch_size=_integer(run["judge_batch_size"], "run.judge_batch_size", minimum=1, maximum=50),
        concurrency=_integer(run["concurrency"], "run.concurrency", minimum=1, maximum=32),
        requests_per_minute=_integer(run["requests_per_minute"], "run.requests_per_minute", minimum=1, maximum=10_000),
        seed=_integer(run["seed"], "run.seed", minimum=0, maximum=2**63 - 1),
        oversample_factor=_number(run["oversample_factor"], "run.oversample_factor", minimum=1, maximum=10),
        max_generation_rounds=_integer(run["max_generation_rounds"], "run.max_generation_rounds", minimum=1, maximum=20),
        contrast_share=_number(run["contrast_share"], "run.contrast_share", minimum=0, maximum=1),
        min_words=_integer(run["min_words"], "run.min_words", minimum=1, maximum=5),
        max_words=_integer(run["max_words"], "run.max_words", minimum=1, maximum=5),
        max_draft_chars=_integer(run["max_draft_chars"], "run.max_draft_chars", minimum=10, maximum=1000),
        max_context_snippets=_integer(run["max_context_snippets"], "run.max_context_snippets", minimum=1, maximum=12),
        max_context_chars=_integer(run["max_context_chars"], "run.max_context_chars", minimum=20, maximum=10000),
        near_duplicate_threshold=_number(run["near_duplicate_threshold"], "run.near_duplicate_threshold", minimum=0.5, maximum=1),
        timeout_seconds=_number(run["timeout_seconds"], "run.timeout_seconds", minimum=1, maximum=600),
        max_attempts=_integer(run["max_attempts"], "run.max_attempts", minimum=1, maximum=10),
    )
    if run_config.min_words > run_config.max_words:
        raise ValueError("run.min_words cannot exceed run.max_words")
    thresholds = payload["thresholds"]
    if not isinstance(thresholds, Mapping) or set(thresholds) != {
        "minimum_score", "minimum_dataset_value", "require_pass_flag"
    }:
        raise ValueError("[thresholds] has invalid keys")
    require_pass = thresholds["require_pass_flag"]
    if not isinstance(require_pass, bool):
        raise ValueError("thresholds.require_pass_flag must be boolean")
    return SyntheticConfig(
        path=path.resolve(),
        run=run_config,
        generator=_model(payload["generator"], "generator"),
        judge=_model(payload["judge"], "judge"),
        thresholds=ThresholdConfig(
            minimum_score=_integer(thresholds["minimum_score"], "thresholds.minimum_score", minimum=1, maximum=5),
            minimum_dataset_value=_integer(thresholds["minimum_dataset_value"], "thresholds.minimum_dataset_value", minimum=1, maximum=5),
            require_pass_flag=require_pass,
        ),
        behaviors=_weights(payload["behaviors"], CONTEXT_BEHAVIORS, "behaviors"),
        categories=_weights(payload["categories"], CATEGORIES, "categories"),
    )
