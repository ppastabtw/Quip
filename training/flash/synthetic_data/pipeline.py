"""Resumable generator -> validator -> judge -> dedupe -> dataset pipeline."""

from __future__ import annotations

import hashlib
import json
import time
from collections import Counter, defaultdict, deque
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import asdict, dataclass, replace
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Mapping, Sequence

from scoring import score_completion

from .artifacts import append_jsonl, load_jsonl, write_json, write_jsonl
from .config import SyntheticConfig
from .dedupe import Deduplicator
from .models import (
    CATEGORIES,
    CONTEXT_BEHAVIORS,
    Candidate,
    ContextSnippet,
    Judgment,
    compact_json,
    stable_id,
)
from .prompts import (
    GENERATOR_PROMPT_VERSION,
    JUDGE_PROMPT_VERSION,
    generation_request,
    generator_system_prompt,
    judge_request,
    judge_system_prompt,
)
from .provider import Completion, StructuredClient
from .scheduling import Slot, allocate_counts, batches, make_slots, take_slots
from .validation import contrast_group_rejections, judgment_rejection_reasons, validate_candidate


@dataclass(frozen=True)
class CallAttempt:
    attempt: int
    completion: Completion | None
    error: str | None


@dataclass(frozen=True)
class GenerationOutcome:
    batch_id: str
    slots: tuple[Slot, ...]
    candidates: tuple[Candidate, ...]
    attempts: tuple[CallAttempt, ...]
    error: str | None


@dataclass(frozen=True)
class JudgeOutcome:
    batch_id: str
    candidate_ids: tuple[str, ...]
    judgments: tuple[Judgment, ...]
    attempts: tuple[CallAttempt, ...]
    error: str | None


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def _completion_record(stage: str, batch_id: str, attempt: CallAttempt) -> dict[str, Any]:
    completion = attempt.completion
    return {
        "stage": stage,
        "batch_id": batch_id,
        "attempt": attempt.attempt,
        "created_at": utc_now(),
        "provider": completion.provider if completion else None,
        "model": completion.model if completion else None,
        "content": completion.content if completion else None,
        "input_tokens": completion.input_tokens if completion else None,
        "output_tokens": completion.output_tokens if completion else None,
        "total_tokens": completion.total_tokens if completion else None,
        "estimated_cost_usd": completion.estimated_cost_usd if completion else None,
        "latency_ms": completion.latency_ms if completion else None,
        "error": attempt.error,
    }


def _parse_generation(content: str, *, run_id: str, slots: Sequence[Slot]) -> tuple[Candidate, ...]:
    payload = json.loads(content)
    if not isinstance(payload, dict) or set(payload) != {"candidates"} or not isinstance(payload["candidates"], list):
        raise ValueError("generator reply must contain exactly candidates array")
    expected = {slot.slot_id: slot for slot in slots}
    parsed: list[Candidate] = []
    seen: set[str] = set()
    for row in payload["candidates"]:
        candidate = Candidate.from_mapping(row, run_id=run_id)
        if candidate.slot_id not in expected:
            raise ValueError(f"generator returned unexpected slot_id {candidate.slot_id}")
        if candidate.slot_id in seen:
            raise ValueError(f"generator returned duplicate slot_id {candidate.slot_id}")
        seen.add(candidate.slot_id)
        parsed.append(candidate)
    missing = set(expected) - seen
    if missing:
        raise ValueError("generator omitted slots: " + ", ".join(sorted(missing)))
    return tuple(parsed)


def _parse_judgments(content: str, *, candidate_ids: Sequence[str]) -> tuple[Judgment, ...]:
    payload = json.loads(content)
    if not isinstance(payload, dict) or set(payload) != {"judgments"} or not isinstance(payload["judgments"], list):
        raise ValueError("judge reply must contain exactly judgments array")
    expected = set(candidate_ids)
    parsed = tuple(Judgment.from_mapping(row) for row in payload["judgments"])
    actual = {row.candidate_id for row in parsed}
    if len(parsed) != len(actual) or actual != expected:
        raise ValueError("judge candidate IDs are missing, duplicated, or unexpected")
    return parsed


def _with_retries(
    call: Callable[[], Completion],
    parse: Callable[[str], Any],
    *,
    max_attempts: int,
) -> tuple[Any | None, tuple[CallAttempt, ...], str | None]:
    attempts: list[CallAttempt] = []
    last_error: Exception | None = None
    for attempt_number in range(1, max_attempts + 1):
        completion: Completion | None = None
        try:
            completion = call()
            parsed = parse(completion.content)
            attempts.append(CallAttempt(attempt_number, completion, None))
            return parsed, tuple(attempts), None
        except (json.JSONDecodeError, RuntimeError, TypeError, ValueError) as exc:
            last_error = exc
            attempts.append(CallAttempt(attempt_number, completion, f"{type(exc).__name__}: {exc}"))
            if attempt_number < max_attempts:
                time.sleep(min(2 ** (attempt_number - 1), 4))
    error = f"{type(last_error).__name__}: {last_error}" if last_error else "unknown structured call failure"
    return None, tuple(attempts), error


class SyntheticPipeline:
    def __init__(
        self,
        *,
        config: SyntheticConfig,
        client: StructuredClient,
        output_dir: Path,
        run_id: str,
        reference_paths: Sequence[Path] = (),
        candidate_count: int | None = None,
    ) -> None:
        self.config = config
        self.client = client
        self.output_dir = output_dir.resolve()
        self.run_id = run_id
        self.reference_paths = tuple(path.resolve() for path in reference_paths)
        self.reference_candidates = self._load_reference_candidates()
        self.candidate_count = candidate_count
        if candidate_count is not None and candidate_count < self.config.run.target_count:
            raise ValueError("candidate_count cannot be smaller than the final target_count")
        self.output_dir.mkdir(parents=True, exist_ok=True)
        self.raw_responses_path = self.output_dir / "raw_responses.jsonl"
        self.raw_candidates_path = self.output_dir / "raw_candidates.jsonl"
        self.local_validation_path = self.output_dir / "local_validation.jsonl"
        self.judge_results_path = self.output_dir / "judge_results.jsonl"
        self.judge_failures_path = self.output_dir / "judge_failures.jsonl"
        self.accepted_path = self.output_dir / "accepted_examples.jsonl"
        self.rejected_path = self.output_dir / "rejected_examples.jsonl"
        self.training_path = self.output_dir / "train.jsonl"
        self.summary_path = self.output_dir / "summary.json"
        self.state_path = self.output_dir / "state.json"
        self.manifest_path = self.output_dir / "manifest.json"
        self._initialize_manifest()

    def _config_snapshot(self) -> dict[str, Any]:
        return {
            "run": asdict(self.config.run),
            "generator": asdict(self.config.generator),
            "judge": asdict(self.config.judge),
            "thresholds": asdict(self.config.thresholds),
            "behaviors": self.config.behaviors,
            "categories": self.config.categories,
            "diversity_references": [
                {
                    "path": str(path),
                    "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                }
                for path in self.reference_paths
            ],
        }

    def _load_reference_candidates(self) -> tuple[Candidate, ...]:
        candidates: list[Candidate] = []
        for path in self.reference_paths:
            if not path.is_file():
                raise FileNotFoundError(f"diversity reference not found: {path}")
            for index, row in enumerate(load_jsonl(path), 1):
                input_value = row.get("input")
                output_value = row.get("output")
                metadata = row.get("metadata") if isinstance(row.get("metadata"), Mapping) else {}
                if (
                    not isinstance(input_value, Mapping)
                    or not isinstance(input_value.get("text"), str)
                    or not isinstance(output_value, Mapping)
                    or set(output_value) != {"suggestion"}
                    or not isinstance(output_value.get("suggestion"), str)
                ):
                    raise ValueError(f"{path}:{index}: invalid reference training row")
                snippet_rows = input_value.get("context_snippets", [])
                if not isinstance(snippet_rows, list):
                    raise ValueError(f"{path}:{index}: invalid reference context_snippets")
                synthetic = metadata.get("synthetic") if isinstance(metadata.get("synthetic"), Mapping) else {}
                category = metadata.get("category")
                if category not in CATEGORIES:
                    category = "vague_reference"
                behavior = synthetic.get("context_behavior")
                if behavior not in CONTEXT_BEHAVIORS:
                    behavior = "useful" if snippet_rows else "none"
                candidates.append(
                    Candidate(
                        slot_id=f"reference_{len(candidates):08d}",
                        category=str(category),
                        context_behavior=str(behavior),
                        group_id=None,
                        variant=None,
                        domain=str(synthetic.get("domain", "reference")),
                        error_type=str(synthetic.get("error_type", "none")),
                        writing_style=str(synthetic.get("writing_style", "conversational")),
                        text=input_value["text"],
                        context_snippets=tuple(
                            ContextSnippet.from_mapping(snippet) for snippet in snippet_rows
                        ),
                        suggestion=output_value["suggestion"],
                        rationale="diversity reference",
                        candidate_id=str(
                            metadata.get("example_id")
                            or stable_id("reference_", {"path": str(path), "line": index})
                        ),
                    )
                )
        return tuple(candidates)

    def _diversity_reference(self, slots: Sequence[Slot]) -> dict[str, object] | None:
        if not self.reference_candidates:
            return None
        sample_size = min(16, len(self.reference_candidates))
        offset = int(
            hashlib.sha256(
                compact_json([slot.slot_id for slot in slots]).encode("utf-8")
            ).hexdigest()[:12],
            16,
        ) % len(self.reference_candidates)
        sample = [
            self.reference_candidates[(offset + index) % len(self.reference_candidates)]
            for index in range(sample_size)
        ]
        return {
            "existing_rows": len(self.reference_candidates),
            "coverage": {
                "categories": dict(
                    sorted(Counter(row.category for row in self.reference_candidates).items())
                ),
                "context_behaviors": dict(
                    sorted(
                        Counter(
                            row.context_behavior for row in self.reference_candidates
                        ).items()
                    )
                ),
                "apps": dict(
                    sorted(
                        Counter(
                            snippet.app_name
                            for row in self.reference_candidates
                            for snippet in row.context_snippets
                        ).items()
                    )
                ),
                "domains": dict(
                    sorted(Counter(row.domain for row in self.reference_candidates).items())
                ),
                "error_types": dict(
                    sorted(
                        Counter(row.error_type for row in self.reference_candidates).items()
                    )
                ),
                "writing_styles": dict(
                    sorted(
                        Counter(row.writing_style for row in self.reference_candidates).items()
                    )
                ),
            },
            "avoid_examples": [
                {
                    "text": row.text,
                    "suggestion": row.suggestion,
                    "apps": [snippet.app_name for snippet in row.context_snippets],
                }
                for row in sample
            ],
        }

    def _initialize_manifest(self) -> None:
        snapshot = self._config_snapshot()
        fingerprint = hashlib.sha256(compact_json(snapshot).encode("utf-8")).hexdigest()
        if self.manifest_path.is_file():
            existing = json.loads(self.manifest_path.read_text(encoding="utf-8"))
            if existing.get("config_sha256") != fingerprint or existing.get("run_id") != self.run_id:
                raise ValueError("run directory belongs to a different run ID or configuration")
            return
        write_json(
            self.manifest_path,
            {
                "schema_version": 1,
                "run_id": self.run_id,
                "created_at": utc_now(),
                "config_path": str(self.config.path),
                "config_sha256": fingerprint,
                "config": snapshot,
                "generator_prompt_version": GENERATOR_PROMPT_VERSION,
                "judge_prompt_version": JUDGE_PROMPT_VERSION,
            },
        )

    def _write_state(self, stage: str) -> None:
        write_json(
            self.state_path,
            {
                "run_id": self.run_id,
                "stage": stage,
                "updated_at": utc_now(),
                "raw_candidates": len(load_jsonl(self.raw_candidates_path)),
                "locally_validated": len(load_jsonl(self.local_validation_path)),
                "judgments": len(load_jsonl(self.judge_results_path)),
                "training_rows": len(load_jsonl(self.training_path)),
                "candidate_pool_target": self.candidate_count,
            },
        )

    def validate_models(self) -> None:
        if not self.config.generator.provider or not self.config.generator.model:
            raise ValueError("generator model is unset; pass --generator-model provider/model")
        if not self.config.judge.provider or not self.config.judge.model:
            raise ValueError("judge model is unset; pass --judge-model provider/model")
        unique = {(row.provider, row.model): row for row in (self.config.generator, self.config.judge)}
        self.client.validate_models(list(unique.values()))

    def _generate_batch(self, slots: tuple[Slot, ...], *, round_number: int) -> GenerationOutcome:
        batch_id = stable_id("gen_", [slot.slot_id for slot in slots])
        user_prompt = generation_request(
            self.config,
            slots,
            seed=self.config.run.seed + round_number * 1_000_003,
            diversity_reference=self._diversity_reference(slots),
        )
        parsed, attempts, error = _with_retries(
            lambda: self.client.complete(
                system_prompt=generator_system_prompt(),
                user_prompt=user_prompt,
                config=self.config.generator,
            ),
            lambda content: _parse_generation(content, run_id=self.run_id, slots=slots),
            max_attempts=self.config.run.max_attempts,
        )
        return GenerationOutcome(batch_id, slots, parsed or (), attempts, error)

    def generate(self, *, round_number: int = 1, requested_count: int | None = None) -> dict[str, int]:
        completed_slots = {row["slot_id"] for row in load_jsonl(self.raw_candidates_path)}
        scheduling_config = self.config
        if requested_count is not None:
            scheduling_config = replace(
                self.config,
                run=replace(self.config.run, target_count=requested_count),
            )
        slots = [slot for slot in make_slots(scheduling_config, round_number=round_number) if slot.slot_id not in completed_slots]
        if self.candidate_count is not None:
            remaining_candidates = max(0, self.candidate_count - len(completed_slots))
            slots = take_slots(slots, remaining_candidates)
        work = list(batches(slots, self.config.run.batch_size))
        generated = 0
        valid = 0
        failures = 0
        with ThreadPoolExecutor(max_workers=self.config.run.concurrency) as executor:
            futures = [executor.submit(self._generate_batch, batch, round_number=round_number) for batch in work]
            for future in as_completed(futures):
                outcome = future.result()
                append_jsonl(
                    self.raw_responses_path,
                    (
                        _completion_record("generate", outcome.batch_id, attempt)
                        for attempt in outcome.attempts
                    ),
                )
                if outcome.error:
                    failures += 1
                    self._write_state("generating")
                    continue
                expected = {slot.slot_id: slot for slot in outcome.slots}
                final_attempt = outcome.attempts[-1].completion
                generation_metadata = {
                    "prompt_version": GENERATOR_PROMPT_VERSION,
                    "provider": final_attempt.provider if final_attempt else None,
                    "model": final_attempt.model if final_attempt else None,
                    "batch_id": outcome.batch_id,
                    "round": round_number,
                }
                raw_rows = [
                    candidate.audit_record(
                        run_id=self.run_id, generation=generation_metadata
                    )
                    for candidate in outcome.candidates
                ]
                append_jsonl(self.raw_candidates_path, raw_rows)
                generated += len(outcome.candidates)
                validations = []
                for candidate in outcome.candidates:
                    reasons = validate_candidate(
                        candidate,
                        self.config,
                        expected_slot=expected[candidate.slot_id],
                    )
                    validations.append(
                        {
                            "candidate_id": candidate.candidate_id,
                            "pass": not reasons,
                            "failure_reasons": reasons,
                        }
                    )
                    valid += not reasons
                append_jsonl(self.local_validation_path, validations)
                self._write_state("generating")
        self._write_state("generated")
        return {"scheduled": len(slots), "generated": generated, "locally_valid": valid, "failed_batches": failures}

    def _judge_batch(self, candidates: tuple[Candidate, ...]) -> JudgeOutcome:
        candidate_ids = tuple(row.candidate_id for row in candidates)
        batch_id = stable_id("judge_", candidate_ids)
        parsed, attempts, error = _with_retries(
            lambda: self.client.complete(
                system_prompt=judge_system_prompt(),
                user_prompt=judge_request(candidates),
                config=self.config.judge,
            ),
            lambda content: _parse_judgments(content, candidate_ids=candidate_ids),
            max_attempts=self.config.run.max_attempts,
        )
        return JudgeOutcome(batch_id, candidate_ids, parsed or (), attempts, error)

    def judge(self) -> dict[str, int]:
        raw = {row["candidate_id"]: Candidate.from_record(row) for row in load_jsonl(self.raw_candidates_path)}
        local = {row["candidate_id"]: row for row in load_jsonl(self.local_validation_path)}
        completed = {row["candidate_id"] for row in load_jsonl(self.judge_results_path)}
        pending = [
            candidate for candidate_id, candidate in raw.items()
            if local.get(candidate_id, {}).get("pass") is True and candidate_id not in completed
        ]
        pending.sort(key=lambda row: row.candidate_id)
        work = [tuple(pending[index : index + self.config.run.judge_batch_size]) for index in range(0, len(pending), self.config.run.judge_batch_size)]
        judged = 0
        failures = 0
        with ThreadPoolExecutor(max_workers=self.config.run.concurrency) as executor:
            futures = [executor.submit(self._judge_batch, batch) for batch in work]
            for future in as_completed(futures):
                outcome = future.result()
                append_jsonl(
                    self.raw_responses_path,
                    (
                        _completion_record("judge", outcome.batch_id, attempt)
                        for attempt in outcome.attempts
                    ),
                )
                if outcome.error:
                    failures += 1
                    append_jsonl(
                        self.judge_failures_path,
                        (
                            {
                                "batch_id": outcome.batch_id,
                                "candidate_id": candidate_id,
                                "error": outcome.error,
                            }
                            for candidate_id in outcome.candidate_ids
                        ),
                    )
                    self._write_state("judging")
                    continue
                completion = outcome.attempts[-1].completion
                append_jsonl(
                    self.judge_results_path,
                    (
                        {
                            **judgment.to_dict(),
                            "judge": {
                                "prompt_version": JUDGE_PROMPT_VERSION,
                                "provider": completion.provider if completion else None,
                                "model": completion.model if completion else None,
                                "batch_id": outcome.batch_id,
                            },
                        }
                        for judgment in outcome.judgments
                    ),
                )
                judged += len(outcome.judgments)
                self._write_state("judging")
        self._write_state("judged")
        return {"pending": len(pending), "judged": judged, "failed_batches": failures}

    def _training_row(self, candidate: Candidate, judgment: Judgment) -> dict[str, Any]:
        generator = self.config.generator
        judge = self.config.judge
        suggestion = candidate.suggestion
        metadata = {
            "example_id": "quip_" + candidate.candidate_id,
            "family_id": f"synthetic:{self.run_id}:{candidate.group_id or candidate.candidate_id}",
            "source_dataset": "quip_synthetic_context_v1",
            "source_record_id": candidate.candidate_id,
            "source_partition": "train",
            "source_license": "synthetic_generated",
            "generation": {
                "method": "llm_synthetic",
                "run_id": self.run_id,
                "generator_provider": generator.provider or "mock",
                "generator_model": generator.model or "deterministic-v1",
                "generator_prompt_version": GENERATOR_PROMPT_VERSION,
                "judge_provider": judge.provider or "mock",
                "judge_model": judge.model or "deterministic-v1",
                "judge_prompt_version": JUDGE_PROMPT_VERSION,
                "seed": self.config.run.seed,
            },
            "category": candidate.category,
            "target_changed": suggestion != candidate.text,
            "accepted_suggestions": [suggestion],
            "window_size": len(candidate.text.split()),
            "synthetic": {
                "context_behavior": candidate.context_behavior,
                "contrast_group_id": candidate.group_id,
                "contrast_variant": candidate.variant,
                "domain": candidate.domain,
                "error_type": candidate.error_type,
                "writing_style": candidate.writing_style,
                "judge_scores": judgment.scores,
            },
        }
        row = {"input": candidate.input_dict(), "output": {"suggestion": suggestion}, "metadata": metadata}
        score = score_completion(
            input_text=row["input"],
            expected_output=row["output"],
            metadata=metadata,
            response_text=suggestion,
        )
        if not score.success:
            raise ValueError(f"training row {candidate.candidate_id} fails reward contract: {score.reason}")
        return row

    def _select_final(self, accepted: Sequence[tuple[Candidate, Judgment]]) -> list[tuple[Candidate, Judgment]]:
        target = self.config.run.target_count

        def quality(pair: tuple[Candidate, Judgment]) -> tuple[int, int, int, int, str]:
            scores = pair[1].scores
            return (
                -min(scores.values()),
                -sum(scores.values()),
                -scores["dataset_value"],
                -scores["context_grounding"],
                pair[0].candidate_id,
            )

        groups: dict[str, list[tuple[Candidate, Judgment]]] = defaultdict(list)
        singles: list[tuple[Candidate, Judgment]] = []
        for pair in accepted:
            if pair[0].group_id is None:
                singles.append(pair)
            else:
                groups[pair[0].group_id].append(pair)
        complete_groups = [rows for rows in groups.values() if len(rows) == 6]
        style_capacities = Counter(pair[0].writing_style for pair in accepted)
        complete_groups.sort(
            key=lambda rows: (
                sum(style_capacities[pair[0].writing_style] for pair in rows),
                sum(sum(pair[1].scores.values()) for pair in rows) * -1,
                rows[0][0].group_id or "",
            )
        )
        desired_groups = min(
            target // 6,
            max(1 if target >= 6 and self.config.run.contrast_share > 0 else 0, round(target * self.config.run.contrast_share / 6)),
        )
        selected: list[tuple[Candidate, Judgment]] = [pair for rows in complete_groups[:desired_groups] for pair in rows]

        behavior_targets = allocate_counts(target, self.config.behaviors)
        category_targets = allocate_counts(target, self.config.categories)
        style_targets = self._capacity_balanced_targets(
            target, Counter(pair[0].writing_style for pair in accepted)
        )
        error_targets = self._capacity_balanced_targets(
            target, Counter(pair[0].error_type for pair in accepted)
        )
        behavior_counts = Counter(pair[0].context_behavior for pair in selected)
        category_counts = Counter(pair[0].category for pair in selected)
        style_counts = Counter(pair[0].writing_style for pair in selected)
        error_counts = Counter(pair[0].error_type for pair in selected)
        selected_ids = {pair[0].candidate_id for pair in selected}
        remaining = [pair for pair in accepted if pair[0].candidate_id not in selected_ids]
        buckets: dict[
            tuple[str, str, str, str], deque[tuple[Candidate, Judgment]]
        ] = defaultdict(deque)
        for pair in sorted(remaining, key=quality):
            buckets[
                (
                    pair[0].context_behavior,
                    pair[0].category,
                    pair[0].writing_style,
                    pair[0].error_type,
                )
            ].append(pair)
        while len(selected) < target:
            available = [key for key, values in buckets.items() if values]
            if not available:
                break

            def priority(
                key: tuple[str, str, str, str]
            ) -> tuple[float, float, float, float, str, str, str, str]:
                behavior, category, style, error_type = key
                behavior_deficit = behavior_targets[behavior] - behavior_counts[behavior]
                category_deficit = category_targets[category] - category_counts[category]
                style_deficit = style_targets[style] - style_counts[style]
                error_deficit = error_targets[error_type] - error_counts[error_type]
                return (
                    behavior_deficit,
                    category_deficit,
                    style_deficit,
                    error_deficit,
                    behavior,
                    category,
                    style,
                    error_type,
                )

            chosen = max(available, key=priority)
            pair = buckets[chosen].popleft()
            selected.append(pair)
            behavior_counts[pair[0].context_behavior] += 1
            category_counts[pair[0].category] += 1
            style_counts[pair[0].writing_style] += 1
            error_counts[pair[0].error_type] += 1
        return selected

    @staticmethod
    def _capacity_balanced_targets(
        total: int, capacities: Mapping[str, int]
    ) -> dict[str, int]:
        targets = {name: 0 for name in capacities}
        for _ in range(total):
            available = [
                name for name, capacity in capacities.items() if targets[name] < capacity
            ]
            if not available:
                break
            chosen = min(available, key=lambda name: (targets[name], name))
            targets[chosen] += 1
        return targets

    def build(self) -> dict[str, Any]:
        raw_records = load_jsonl(self.raw_candidates_path)
        candidates = {row["candidate_id"]: Candidate.from_record(row) for row in raw_records}
        local = {row["candidate_id"]: row for row in load_jsonl(self.local_validation_path)}
        judgment_records = {row["candidate_id"]: row for row in load_jsonl(self.judge_results_path)}
        judgments = {
            candidate_id: Judgment.from_mapping(
                {key: value for key, value in row.items() if key != "judge"}
            )
            for candidate_id, row in judgment_records.items()
        }
        group_rejections = contrast_group_rejections(list(candidates.values()))
        deduper = Deduplicator(near_threshold=self.config.run.near_duplicate_threshold)
        for reference in self.reference_candidates:
            deduper.seed_reference(reference)
        accepted: list[tuple[Candidate, Judgment]] = []
        accepted_audit: list[dict[str, Any]] = []
        rejected_audit: list[dict[str, Any]] = []
        for raw in sorted(raw_records, key=lambda row: row["candidate_id"]):
            candidate = candidates[raw["candidate_id"]]
            reasons = list(local.get(candidate.candidate_id, {}).get("failure_reasons", []))
            stage = "local_validation" if reasons else "judge"
            judgment = judgments.get(candidate.candidate_id)
            if not reasons and judgment is None:
                reasons.append("judge_missing_or_failed")
            if judgment is not None:
                reasons.extend(judgment_rejection_reasons(judgment, self.config))
            reasons.extend(group_rejections.get(candidate.candidate_id, []))
            if not reasons:
                duplicate_reason = deduper.rejection_reason(candidate)
                if duplicate_reason:
                    reasons.append(duplicate_reason)
                    stage = "deduplication"
            audit = {
                **raw,
                "judgment": judgment_records.get(candidate.candidate_id),
            }
            if reasons:
                rejected_audit.append({**audit, "rejection_stage": stage, "failure_reasons": sorted(set(reasons))})
            else:
                assert judgment is not None
                accepted.append((candidate, judgment))
                accepted_audit.append(audit)

        final_pairs = self._select_final(accepted)
        training_rows = [self._training_row(candidate, judgment) for candidate, judgment in final_pairs]
        write_jsonl(self.accepted_path, accepted_audit)
        write_jsonl(self.rejected_path, rejected_audit)
        write_jsonl(self.training_path, training_rows)
        summary = self._summary(raw_records, accepted, rejected_audit, final_pairs)
        write_json(self.summary_path, summary)
        self._write_state("built")
        return summary

    def _summary(
        self,
        raw_records: Sequence[Mapping[str, Any]],
        accepted: Sequence[tuple[Candidate, Judgment]],
        rejected: Sequence[Mapping[str, Any]],
        final_pairs: Sequence[tuple[Candidate, Judgment]],
    ) -> dict[str, Any]:
        responses = load_jsonl(self.raw_responses_path)
        final = [row[0] for row in final_pairs]

        def distribution(attribute: str) -> dict[str, int]:
            counts = Counter(str(getattr(row, attribute)) for row in final)
            return dict(sorted(counts.items()))

        apps = Counter(snippet.app_name for row in final for snippet in row.context_snippets)
        rejection_reasons = Counter(reason for row in rejected for reason in row.get("failure_reasons", []))
        category_generated = Counter(str(row.get("category")) for row in raw_records)
        category_accepted = Counter(row.category for row, _ in accepted)
        return {
            "schema_version": 1,
            "run_id": self.run_id,
            "created_at": utc_now(),
            "target_count": self.config.run.target_count,
            "candidate_pool_target": self.candidate_count,
            "diversity_reference_rows": len(self.reference_candidates),
            "diversity_reference_files": [str(path) for path in self.reference_paths],
            "raw_candidates": len(raw_records),
            "judge_results": len(load_jsonl(self.judge_results_path)),
            "accepted_after_dedupe": len(accepted),
            "rejected": len(rejected),
            "final_training_rows": len(final),
            "training_sha256": hashlib.sha256(
                self.training_path.read_bytes()
            ).hexdigest(),
            "target_met": len(final) >= self.config.run.target_count,
            "selection_policy": "highest judge scores within exact behavior/category balance, with secondary writing-style/error-mechanism breadth; complete passing contrast groups are prioritized and individually valid variants fill any shortfall",
            "acceptance_rate": round(len(accepted) / len(raw_records), 6) if raw_records else 0.0,
            "usage": {
                "requests": len(responses),
                "failed_requests": sum(row.get("error") is not None for row in responses),
                "input_tokens": sum(row.get("input_tokens") or 0 for row in responses),
                "output_tokens": sum(row.get("output_tokens") or 0 for row in responses),
                "total_tokens": sum(row.get("total_tokens") or 0 for row in responses),
                "estimated_cost_usd": round(sum(row.get("estimated_cost_usd") or 0 for row in responses), 8),
                "latency_ms": sum(row.get("latency_ms") or 0 for row in responses),
            },
            "final_diversity": {
                "category": distribution("category"),
                "context_behavior": distribution("context_behavior"),
                "domain": distribution("domain"),
                "error_type": distribution("error_type"),
                "writing_style": distribution("writing_style"),
                "app_name": dict(sorted(apps.items())),
                "contrast_groups": len({row.group_id for row in final if row.group_id is not None}),
            },
            "category_acceptance": {
                category: {
                    "generated": category_generated[category],
                    "accepted": category_accepted[category],
                    "rate": round(category_accepted[category] / category_generated[category], 6)
                    if category_generated[category]
                    else 0.0,
                }
                for category in CATEGORIES
            },
            "rejection_reasons": dict(sorted(rejection_reasons.items())),
            "artifacts": {
                "raw_responses": self.raw_responses_path.name,
                "raw_candidates": self.raw_candidates_path.name,
                "local_validation": self.local_validation_path.name,
                "judge_results": self.judge_results_path.name,
                "accepted_examples": self.accepted_path.name,
                "rejected_examples": self.rejected_path.name,
                "training_dataset": self.training_path.name,
            },
        }

    def run(self) -> dict[str, Any]:
        self.validate_models()
        summary: dict[str, Any] = {}
        for round_number in range(1, self.config.run.max_generation_rounds + 1):
            existing = len(load_jsonl(self.training_path))
            missing = max(1, self.config.run.target_count - existing)
            self.generate(round_number=round_number, requested_count=missing)
            self.judge()
            summary = self.build()
            if summary["target_met"]:
                break
        return summary
