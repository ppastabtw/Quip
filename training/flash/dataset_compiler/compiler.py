"""Compile MASSIVE text windows with deterministic QWERTY augmentation."""

from __future__ import annotations

import json
import random
from collections import Counter
from dataclasses import replace
from pathlib import Path
from typing import Any, Mapping, Sequence

from augmentation import augment_text
from .contract import (
    CONTRACT,
    DATASET_DIR,
    MANIFEST_PATH,
    REPORT_PATH,
    BuildError,
    Candidate,
    compact_json,
    make_row,
    normalize_text,
    sha256_file,
    validate_compiled_datasets,
    window_rejection_reason,
)
from .sources import load_manifest, parse_massive, prepare_sources, sample_massive_windows


def _augmentation_generation(result: Mapping[str, Any], *, seed: int) -> dict[str, Any]:
    return {
        "method": "qwerty_augmentation",
        "seed": seed,
        "requested_events": CONTRACT.augmentation_events,
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
        size_rows: list[dict[str, Any]] = []
        unchanged_count = 0

        for base in candidates:
            if len(size_rows) >= required:
                break
            if base.family_id in used_families:
                continue
            target_normalized = normalize_text(base.target)
            if not target_normalized or target_normalized in surfaces:
                continue

            if unchanged_count < unchanged_required:
                candidate = base
                generation = {"method": "sourced"}
            else:
                row_seed = rng.randrange(0, 2**63)
                result = augment_text(
                    base.target,
                    seed=row_seed,
                    event_count=CONTRACT.augmentation_events,
                )
                draft = result["augmented"]
                if result["applied_events"] != CONTRACT.augmentation_events:
                    continue
                if window_rejection_reason(draft, expected_size=size):
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
                    category="qwerty_typo",
                    target_changed=True,
                )
                generation = _augmentation_generation(result, seed=row_seed)

            input_normalized = normalize_text(candidate.text)
            if input_normalized in surfaces:
                continue
            size_rows.append(make_row(candidate, generation=generation))
            surfaces.update({input_normalized, target_normalized})
            used_families.add(base.family_id)
            if not candidate.target_changed:
                unchanged_count += 1

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
    return {
        "rows": len(rows),
        "sha256": sha256_file(path),
        "changes": {"unchanged": changes[False], "changed": changes[True]},
        "window_sizes": {str(size): sizes[size] for size in CONTRACT.window_sizes},
        "unchanged_by_size": {
            str(size): unchanged_sizes[size] for size in CONTRACT.window_sizes
        },
    }


def compile_datasets(*, seed: int, offline: bool) -> dict[str, Any]:
    manifest = load_manifest()
    source_paths = prepare_sources(manifest, offline=offline)
    records = parse_massive(source_paths["massive_en_us"])
    rng = random.Random(seed)

    test_pools = sample_massive_windows(
        records,
        partition="test",
        required_by_size=CONTRACT.expected_counts("test")["window_sizes"],
        rng=rng,
    )
    eval_pools = sample_massive_windows(
        records,
        partition="dev",
        required_by_size=CONTRACT.expected_counts("eval")["window_sizes"],
        rng=rng,
    )
    train_pools = sample_massive_windows(
        records,
        partition="train",
        required_by_size=CONTRACT.expected_counts("train")["window_sizes"],
        rng=rng,
    )
    test_rows, _ = select_split(test_pools, split="test", rng=rng)
    eval_rows, _ = select_split(eval_pools, split="eval", rng=rng)
    train_rows, _ = select_split(train_pools, split="train", rng=rng)

    DATASET_DIR.mkdir(parents=True, exist_ok=True)
    train_path = DATASET_DIR / "train.jsonl"
    eval_path = DATASET_DIR / "eval.jsonl"
    test_path = DATASET_DIR / "test.jsonl"
    write_jsonl(train_path, train_rows)
    write_jsonl(eval_path, eval_rows)
    write_jsonl(test_path, test_rows)

    report = {
        "schema_version": 2,
        "dataset_policy": "massive_window_augmentation_v1",
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
            "augmentation": "deterministic_us_qwerty",
            "augmentation_events": CONTRACT.augmentation_events,
            "profanity_filter": "better-profanity==0.7.0",
            "source_quality": "wordfreq==3.1.1 with minimum Zipf frequency 3.0",
            "sampling": "one seeded random contiguous window per source utterance",
            "split_policy": "MASSIVE train, dev, and test remain separate",
        },
        "source_manifest_sha256": sha256_file(MANIFEST_PATH),
        "splits": {
            "train": split_summary(train_rows, train_path),
            "eval": split_summary(eval_rows, eval_path),
            "test": split_summary(test_rows, test_path),
        },
    }
    REPORT_PATH.write_text(
        json.dumps(report, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    validate_compiled_datasets(
        train_path=train_path,
        eval_path=eval_path,
        test_path=test_path,
        report_path=REPORT_PATH,
    )
    return report


def verify_only() -> None:
    validate_compiled_datasets(
        train_path=DATASET_DIR / "train.jsonl",
        eval_path=DATASET_DIR / "eval.jsonl",
        test_path=DATASET_DIR / "test.jsonl",
        report_path=REPORT_PATH,
    )
