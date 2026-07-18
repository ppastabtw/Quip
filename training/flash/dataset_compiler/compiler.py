"""Compile MASSIVE text windows with deterministic QWERTY augmentation."""

from __future__ import annotations

import json
import random
from collections import Counter
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, Mapping, Sequence

from augmentation import augment_text
from .contract import (
    CONTRACT,
    DATASET_DIR,
    ENGLISH_TOKEN_RE,
    MANIFEST_PATH,
    REPORT_PATH,
    BuildError,
    Candidate,
    ambiguity_rejection_reason,
    compact_json,
    draft_rejection_reason,
    make_row,
    normalize_text,
    sha256_file,
    validate_compiled_datasets,
    window_rejection_reason,
)
from .sources import load_manifest, parse_massive, prepare_sources, sample_massive_windows


@dataclass(frozen=True)
class DatasetPolicy:
    name: str
    category: str
    event_count_weights: tuple[tuple[int, int], ...]
    one_word_event_count_weights: tuple[tuple[int, int], ...] | None = None
    two_word_event_count_weights: tuple[tuple[int, int], ...] | None = None
    operator_weights: tuple[tuple[str, float], ...] | None = None
    ambiguity_minimum_zipf: float | None = None
    allow_input_word_count_change: bool = False
    isolate_normalized_surfaces: bool = False
    source_buffer_share: float | None = None
    one_word_source_buffer_share: float | None = None
    minimum_characters_per_event: int | None = None
    generation_method: str = "qwerty_augmentation"
    letter_neighbors_only: bool = False


V1_POLICY = DatasetPolicy(
    name="massive_window_augmentation_v1",
    category="qwerty_typo",
    event_count_weights=((1, 1),),
)

V2_DRAFT_POLICY = DatasetPolicy(
    name="massive_window_augmentation_v2_draft",
    category="synthetic_typing_error",
    event_count_weights=((1, 3), (2, 5), (3, 3), (4, 1)),
    one_word_event_count_weights=((1, 7), (2, 5)),
    two_word_event_count_weights=((1, 4), (2, 5), (3, 3)),
    operator_weights=(
        ("substitution", 35.0),
        ("deletion", 12.0),
        ("insertion", 8.0),
        ("transposition", 5.0),
        ("repeat", 8.0),
        ("spacing", 10.0),
        ("vowel_deletion", 12.0),
        ("phonetic_rewrite", 10.0),
    ),
    ambiguity_minimum_zipf=3.0,
    allow_input_word_count_change=True,
    isolate_normalized_surfaces=True,
    source_buffer_share=0.50,
    one_word_source_buffer_share=2.25,
    minimum_characters_per_event=3,
    generation_method="deterministic_typing_augmentation",
    letter_neighbors_only=True,
)

POLICIES = {policy.name: policy for policy in (V1_POLICY, V2_DRAFT_POLICY)}


def event_count_weights(
    policy: DatasetPolicy, *, window_size: int | None = None
) -> tuple[tuple[int, int], ...]:
    if window_size == 1 and policy.one_word_event_count_weights is not None:
        return policy.one_word_event_count_weights
    if window_size == 2 and policy.two_word_event_count_weights is not None:
        return policy.two_word_event_count_weights
    return policy.event_count_weights


def event_count_quotas(
    total: int, policy: DatasetPolicy, *, window_size: int | None = None
) -> dict[int, int]:
    weights = event_count_weights(policy, window_size=window_size)
    weight_total = sum(weight for _, weight in weights)
    if total % weight_total:
        raise BuildError(
            f"changed-row quota {total} must divide by event weight total {weight_total}"
        )
    unit = total // weight_total
    return {
        event_count: weight * unit
        for event_count, weight in weights
    }


def event_count_schedule(
    total: int,
    *,
    policy: DatasetPolicy,
    rng: random.Random,
    window_size: int | None = None,
) -> list[int]:
    schedule = [
        event_count
        for event_count, count in event_count_quotas(
            total, policy, window_size=window_size
        ).items()
        for _ in range(count)
    ]
    if len({event_count for event_count, _ in event_count_weights(policy, window_size=window_size)}) > 1:
        rng.shuffle(schedule)
    return schedule


def _augmentation_generation(
    result: Mapping[str, Any], *, seed: int, method: str
) -> dict[str, Any]:
    return {
        "method": method,
        "seed": seed,
        "requested_events": result["requested_events"],
        "operations": [
            {
                "event": operation["event"],
                "operator": operation["operator"],
                "index": operation["index"],
                "source": operation["source"],
                "replacement": operation["replacement"],
            }
            for operation in result["operations"]
        ],
    }


def select_split(
    pools: Mapping[int, Sequence[Candidate]],
    *,
    split: str,
    rng: random.Random,
    policy: DatasetPolicy = V1_POLICY,
    excluded_surfaces: set[str] | None = None,
    valid_vocabulary: set[str] | None = None,
) -> tuple[list[dict[str, Any]], set[str]]:
    expected = CONTRACT.expected_counts(split)
    surfaces: set[str] = set()
    used_families: set[str] = set()
    selected: list[dict[str, Any]] = []

    for size in CONTRACT.window_sizes:
        candidates = list(pools[size])
        rng.shuffle(candidates)
        required = expected["window_sizes"][size]
        unchanged_required = expected["unchanged_by_size"][size]
        changed_required = required - unchanged_required
        remaining_event_counts = Counter(
            event_count_quotas(
                changed_required,
                policy,
                window_size=size,
            )
        )
        size_rows: list[dict[str, Any]] = []
        unchanged_count = 0
        changed_count = 0

        for base in candidates:
            if len(size_rows) >= required:
                break
            if base.family_id in used_families:
                continue
            target_normalized = normalize_text(base.target)
            if (
                not target_normalized
                or target_normalized in surfaces
                or target_normalized in (excluded_surfaces or set())
            ):
                continue

            if unchanged_count < unchanged_required:
                candidate = base
                generation = {"method": "sourced"}
            else:
                clean_characters = sum(
                    character.isalpha() for character in base.target
                )
                feasible_event_counts = [
                    event_count
                    for event_count, remaining in remaining_event_counts.items()
                    if remaining > 0
                    and (
                        policy.minimum_characters_per_event is None
                        or clean_characters
                        >= event_count * policy.minimum_characters_per_event
                    )
                ]
                if not feasible_event_counts:
                    continue
                if len(feasible_event_counts) == 1:
                    requested_events = feasible_event_counts[0]
                else:
                    requested_events = rng.choices(
                        feasible_event_counts,
                        weights=[
                            remaining_event_counts[event_count]
                            for event_count in feasible_event_counts
                        ],
                        k=1,
                    )[0]
                row_seed = rng.randrange(0, 2**63)
                result = augment_text(
                    base.target,
                    seed=row_seed,
                    event_count=requested_events,
                    weights=(
                        dict(policy.operator_weights)
                        if policy.operator_weights is not None
                        else None
                    ),
                    letter_neighbors_only=policy.letter_neighbors_only,
                )
                draft = result["augmented"]
                if result["applied_events"] != requested_events:
                    continue
                draft_reason = (
                    draft_rejection_reason(draft)
                    if policy.allow_input_word_count_change
                    else window_rejection_reason(draft, expected_size=size)
                )
                if draft_reason:
                    continue
                if ambiguity_rejection_reason(
                    draft,
                    minimum_zipf_frequency=policy.ambiguity_minimum_zipf,
                    valid_vocabulary=valid_vocabulary,
                ):
                    continue
                input_normalized = normalize_text(draft)
                if (
                    not input_normalized
                    or input_normalized == target_normalized
                    or input_normalized in surfaces
                ):
                    continue
                candidate = replace(
                    base,
                    text=draft,
                    category=(
                        "phonetic_spelling"
                        if any(
                            operation["operator"] == "phonetic_rewrite"
                            for operation in result["operations"]
                        )
                        else "compressed_typing"
                        if any(
                            operation["operator"] in {"spacing", "vowel_deletion"}
                            for operation in result["operations"]
                        )
                        else policy.category
                    ),
                    target_changed=True,
                )
                generation = _augmentation_generation(
                    result,
                    seed=row_seed,
                    method=policy.generation_method,
                )

            input_normalized = normalize_text(candidate.text)
            if input_normalized in surfaces or input_normalized in (
                excluded_surfaces or set()
            ):
                continue
            size_rows.append(make_row(candidate, generation=generation))
            surfaces.update({input_normalized, target_normalized})
            used_families.add(base.family_id)
            if not candidate.target_changed:
                unchanged_count += 1
            else:
                changed_count += 1
                remaining_event_counts[requested_events] -= 1

        if len(size_rows) != required or unchanged_count != unchanged_required:
            raise BuildError(
                f"{split} size-{size} quota failed: "
                f"rows={len(size_rows)}/{required}, "
                f"unchanged={unchanged_count}/{unchanged_required}"
            )
        selected.extend(size_rows)

    rng.shuffle(selected)
    return selected, surfaces


def write_jsonl(path: Path, rows: Sequence[dict[str, Any]]) -> None:
    rendered = "".join(compact_json(row) + "\n" for row in rows)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(rendered, encoding="utf-8", newline="\n")
    temporary.replace(path)


def split_summary(rows: Sequence[dict[str, Any]], path: Path) -> dict[str, Any]:
    changes = Counter(row["metadata"]["target_changed"] for row in rows)
    sizes = Counter(row["metadata"]["window_size"] for row in rows)
    unchanged_sizes = Counter(
        row["metadata"]["window_size"]
        for row in rows
        if not row["metadata"]["target_changed"]
    )
    event_counts = Counter(
        row["metadata"]["generation"]["requested_events"]
        for row in rows
        if row["metadata"]["target_changed"]
    )
    return {
        "rows": len(rows),
        "sha256": sha256_file(path),
        "changes": {"unchanged": changes[False], "changed": changes[True]},
        "window_sizes": {str(size): sizes[size] for size in CONTRACT.window_sizes},
        "unchanged_by_size": {
            str(size): unchanged_sizes[size] for size in CONTRACT.window_sizes
        },
        "augmentation_event_counts": {
            str(event_count): count
            for event_count, count in sorted(event_counts.items())
        },
    }


def compile_datasets(
    *,
    seed: int,
    offline: bool,
    policy: DatasetPolicy = V1_POLICY,
    output_dir: Path = DATASET_DIR,
) -> dict[str, Any]:
    manifest = load_manifest()
    source_paths = prepare_sources(manifest, offline=offline)
    records = parse_massive(source_paths["massive_en_us"])
    valid_vocabulary = {
        token.casefold()
        for record in records
        for token in ENGLISH_TOKEN_RE.findall(record["utt"])
    }
    rng = random.Random(seed)

    def source_buffer_rows(split: str) -> dict[int, int] | None:
        if policy.source_buffer_share is None:
            return None
        per_size = next(iter(CONTRACT.expected_counts(split)["window_sizes"].values()))
        return {
            size: max(
                10,
                int(
                    per_size
                    * (
                        policy.one_word_source_buffer_share
                        if size == 1 and policy.one_word_source_buffer_share is not None
                        else policy.source_buffer_share
                    )
                ),
            )
            for size in CONTRACT.window_sizes
        }

    test_pools = sample_massive_windows(
        records,
        partition="test",
        required_by_size=CONTRACT.expected_counts("test")["window_sizes"],
        rng=rng,
        buffer_rows=source_buffer_rows("test"),
    )
    eval_pools = sample_massive_windows(
        records,
        partition="dev",
        required_by_size=CONTRACT.expected_counts("eval")["window_sizes"],
        rng=rng,
        buffer_rows=source_buffer_rows("eval"),
    )
    train_pools = sample_massive_windows(
        records,
        partition="train",
        required_by_size=CONTRACT.expected_counts("train")["window_sizes"],
        rng=rng,
        buffer_rows=source_buffer_rows("train"),
    )
    excluded_surfaces: set[str] = set()
    test_rows, test_surfaces = select_split(
        test_pools,
        split="test",
        rng=rng,
        policy=policy,
        excluded_surfaces=(excluded_surfaces if policy.isolate_normalized_surfaces else None),
        valid_vocabulary=valid_vocabulary,
    )
    excluded_surfaces.update(test_surfaces)
    eval_rows, eval_surfaces = select_split(
        eval_pools,
        split="eval",
        rng=rng,
        policy=policy,
        excluded_surfaces=(excluded_surfaces if policy.isolate_normalized_surfaces else None),
        valid_vocabulary=valid_vocabulary,
    )
    excluded_surfaces.update(eval_surfaces)
    train_rows, _ = select_split(
        train_pools,
        split="train",
        rng=rng,
        policy=policy,
        excluded_surfaces=(excluded_surfaces if policy.isolate_normalized_surfaces else None),
        valid_vocabulary=valid_vocabulary,
    )

    output_dir.mkdir(parents=True, exist_ok=True)
    train_path = output_dir / "train.jsonl"
    eval_path = output_dir / "eval.jsonl"
    test_path = output_dir / "test.jsonl"
    report_path = output_dir / "build_report.json"
    write_jsonl(train_path, train_rows)
    write_jsonl(eval_path, eval_rows)
    write_jsonl(test_path, test_rows)

    report = {
        "schema_version": 2,
        "dataset_policy": policy.name,
        "seed": seed,
        "source": {
            "dataset": "MASSIVE",
            "revision": "1.1",
            "locale": "en-US",
            "records": len(records),
            "license": "CC BY 4.0",
        },
        "contract": {
            "window_sizes": list(CONTRACT.window_sizes),
            "window_share": 1 / len(CONTRACT.window_sizes),
            "unchanged_share": CONTRACT.unchanged_share,
            "augmented_share": 1 - CONTRACT.unchanged_share,
            "augmentation": "deterministic_typing_errors",
            "augmentation_event_weights": {
                str(event_count): weight
                for event_count, weight in policy.event_count_weights
            },
            "one_word_augmentation_event_weights": (
                {
                    str(event_count): weight
                    for event_count, weight in policy.one_word_event_count_weights
                }
                if policy.one_word_event_count_weights is not None
                else None
            ),
            "two_word_augmentation_event_weights": (
                {
                    str(event_count): weight
                    for event_count, weight in policy.two_word_event_count_weights
                }
                if policy.two_word_event_count_weights is not None
                else None
            ),
            "augmentation_operators": (
                dict(policy.operator_weights)
                if policy.operator_weights is not None
                else "default_v1"
            ),
            "ambiguity_minimum_zipf": policy.ambiguity_minimum_zipf,
            "profanity_filter": "better-profanity==0.7.0",
            "source_quality": "wordfreq==3.1.1 with minimum Zipf frequency 3.0",
            "sampling": "one seeded random contiguous window per source utterance",
            "source_buffer_share": policy.source_buffer_share,
            "one_word_source_buffer_share": policy.one_word_source_buffer_share,
            "minimum_characters_per_event": policy.minimum_characters_per_event,
            "letter_neighbors_only": policy.letter_neighbors_only,
            "split_policy": (
                "MASSIVE partitions plus normalized input and target isolation"
                if policy.isolate_normalized_surfaces
                else "MASSIVE train, dev, and test remain separate"
            ),
        },
        "source_manifest_sha256": sha256_file(MANIFEST_PATH),
        "splits": {
            "train": split_summary(train_rows, train_path),
            "eval": split_summary(eval_rows, eval_path),
            "test": split_summary(test_rows, test_path),
        },
    }
    report_path.write_text(
        json.dumps(report, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    expected_event_counts = {
        name: {
            size: event_count_quotas(
                CONTRACT.expected_counts(name)["window_sizes"][size]
                - CONTRACT.expected_counts(name)["unchanged_by_size"][size],
                policy,
                window_size=size,
            )
            for size in CONTRACT.window_sizes
        }
        for name in ("train", "eval", "test")
    }
    validate_compiled_datasets(
        train_path=train_path,
        eval_path=eval_path,
        test_path=test_path,
        report_path=report_path,
        expected_policy=policy.name,
        expected_event_counts=expected_event_counts,
        require_normalized_surface_isolation=policy.isolate_normalized_surfaces,
    )
    return report


def verify_only() -> None:
    validate_compiled_datasets(
        train_path=DATASET_DIR / "train.jsonl",
        eval_path=DATASET_DIR / "eval.jsonl",
        test_path=DATASET_DIR / "test.jsonl",
        report_path=REPORT_PATH,
        expected_policy=V1_POLICY.name,
        expected_event_counts={
            name: {
                size: event_count_quotas(
                    CONTRACT.expected_counts(name)["window_sizes"][size]
                    - CONTRACT.expected_counts(name)["unchanged_by_size"][size],
                    V1_POLICY,
                    window_size=size,
                )
                for size in CONTRACT.window_sizes
            }
            for name in ("train", "eval", "test")
        },
    )
