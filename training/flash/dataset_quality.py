"""Deterministic, non-mutating diagnostics for Quip JSONL datasets."""

from __future__ import annotations

import hashlib
import json
import re
from collections import Counter, defaultdict
from itertools import combinations
from pathlib import Path
from typing import Any, Iterable, Mapping, Sequence

from wordfreq import zipf_frequency


DEFAULT_PROTOCOL = "v1-single-qwerty"
COMMON_WORD_ZIPF = 3.0
SPLIT_NAMES = ("train", "eval", "test")
ENGLISH_WORD_RE = re.compile(r"[A-Za-z]+(?:'[A-Za-z]+)?")


def load_jsonl(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if not line.strip():
                continue
            value = json.loads(line)
            if not isinstance(value, dict):
                raise ValueError(f"{path}:{line_number}: row must be an object")
            rows.append(value)
    return rows


def normalize_text(value: str) -> str:
    return re.sub(r"\s+", " ", value).strip().casefold()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def levenshtein_distance(left: str, right: str) -> int:
    """Return character edit distance without adding a runtime dependency."""
    if len(left) < len(right):
        left, right = right, left
    previous = list(range(len(right) + 1))
    for left_index, left_character in enumerate(left, 1):
        current = [left_index]
        for right_index, right_character in enumerate(right, 1):
            current.append(
                min(
                    current[-1] + 1,
                    previous[right_index] + 1,
                    previous[right_index - 1]
                    + (left_character != right_character),
                )
            )
        previous = current
    return previous[-1]


def _rate(count: int, total: int) -> float:
    return round(count / total, 6) if total else 0.0


def _row_input(row: Mapping[str, Any]) -> str:
    value = row.get("input")
    if not isinstance(value, Mapping) or not isinstance(value.get("text"), str):
        raise ValueError("row input must contain string field text")
    return value["text"]


def _row_target(row: Mapping[str, Any]) -> str:
    value = row.get("output")
    if not isinstance(value, Mapping) or not isinstance(value.get("suggestion"), str):
        raise ValueError("row output must contain string field suggestion")
    return value["suggestion"]


def _metadata(row: Mapping[str, Any]) -> Mapping[str, Any]:
    value = row.get("metadata")
    if not isinstance(value, Mapping):
        raise ValueError("row metadata must be an object")
    return value


def _sorted_counter(counter: Counter[Any]) -> dict[str, int]:
    return {str(key): counter[key] for key in sorted(counter, key=str)}


def load_massive_source_index(
    path: Path | None,
) -> dict[tuple[str, str], Mapping[str, Any]]:
    if path is None or not path.is_file():
        return {}
    index: dict[tuple[str, str], Mapping[str, Any]] = {}
    for row in load_jsonl(path):
        partition = row.get("partition")
        source_id = row.get("id")
        if isinstance(partition, str) and isinstance(source_id, str):
            index[(partition, source_id)] = row
    return index


def _source_identity(metadata: Mapping[str, Any]) -> tuple[str, str] | None:
    source_record_id = metadata.get("source_record_id")
    if not isinstance(source_record_id, str):
        return None
    parts = source_record_id.split(":")
    if len(parts) < 5:
        return None
    return parts[1], parts[2]


def split_diagnostics(
    rows: Sequence[Mapping[str, Any]],
    *,
    source_index: Mapping[tuple[str, str], Mapping[str, Any]],
    source_vocabulary: set[str],
    sample_limit: int,
) -> dict[str, Any]:
    categories: Counter[Any] = Counter()
    changes: Counter[Any] = Counter()
    window_sizes: Counter[Any] = Counter()
    methods: Counter[Any] = Counter()
    operators: Counter[Any] = Counter()
    event_counts: Counter[Any] = Counter()
    edit_distances: Counter[Any] = Counter()
    source_datasets: Counter[Any] = Counter()
    source_partitions: Counter[Any] = Counter()
    source_scenarios: Counter[Any] = Counter()
    source_intents: Counter[Any] = Counter()
    normalized_inputs: Counter[str] = Counter()
    punctuation_targets = 0
    capitalization_targets = 0
    ambiguity_candidates: list[dict[str, Any]] = []

    for row in rows:
        metadata = _metadata(row)
        input_text = _row_input(row)
        target = _row_target(row)
        changed = metadata.get("target_changed") is True
        generation = metadata.get("generation")

        categories[metadata.get("category", "missing")] += 1
        changes["changed" if changed else "unchanged"] += 1
        window_sizes[metadata.get("window_size", "missing")] += 1
        source_datasets[metadata.get("source_dataset", "missing")] += 1
        source_partitions[metadata.get("source_partition", "missing")] += 1
        normalized_inputs[normalize_text(input_text)] += 1

        if any(not character.isalnum() and not character.isspace() for character in target):
            punctuation_targets += 1
        if any(character.isupper() for character in target):
            capitalization_targets += 1

        operations: Sequence[Any] = ()
        if isinstance(generation, Mapping):
            methods[generation.get("method", "missing")] += 1
            raw_operations = generation.get("operations")
            if isinstance(raw_operations, list):
                operations = raw_operations
        event_counts[len(operations)] += 1
        for operation in operations:
            if isinstance(operation, Mapping):
                operators[operation.get("operator", "missing")] += 1

        if changed:
            edit_distances[levenshtein_distance(input_text, target)] += 1
            tokens = input_text.split()
            if tokens and all(ENGLISH_WORD_RE.fullmatch(token) for token in tokens):
                frequencies = [
                    zipf_frequency(token.casefold(), "en") for token in tokens
                ]
                if (
                    all(frequency >= COMMON_WORD_ZIPF for frequency in frequencies)
                    and (
                        not source_vocabulary
                        or all(token.casefold() in source_vocabulary for token in tokens)
                    )
                ):
                    ambiguity_candidates.append(
                        {
                            "input": input_text,
                            "target": target,
                            "minimum_token_zipf": round(min(frequencies), 3),
                        }
                    )

        identity = _source_identity(metadata)
        source_row = source_index.get(identity) if identity is not None else None
        if source_row is not None:
            source_scenarios[source_row.get("scenario", "missing")] += 1
            source_intents[source_row.get("intent", "missing")] += 1

    row_count = len(rows)
    ambiguity_candidates.sort(
        key=lambda item: (
            -item["minimum_token_zipf"],
            item["input"],
            item["target"],
        )
    )
    duplicate_input_rows = sum(count - 1 for count in normalized_inputs.values() if count > 1)
    return {
        "rows": row_count,
        "changes": _sorted_counter(changes),
        "change_rates": {
            key: _rate(count, row_count) for key, count in sorted(changes.items())
        },
        "window_sizes": _sorted_counter(window_sizes),
        "categories": _sorted_counter(categories),
        "generation_methods": _sorted_counter(methods),
        "augmentation_event_counts": _sorted_counter(event_counts),
        "augmentation_operators": _sorted_counter(operators),
        "character_edit_distances": _sorted_counter(edit_distances),
        "punctuation_targets": {
            "count": punctuation_targets,
            "rate": _rate(punctuation_targets, row_count),
        },
        "capitalization_targets": {
            "count": capitalization_targets,
            "rate": _rate(capitalization_targets, row_count),
        },
        "ambiguous_valid_word_candidates": {
            "criterion": (
                "changed input made entirely of English tokens with minimum "
                f"Zipf frequency >= {COMMON_WORD_ZIPF}"
            ),
            "count": len(ambiguity_candidates),
            "sample": ambiguity_candidates[:sample_limit],
        },
        "normalized_duplicate_input_rows": duplicate_input_rows,
        "source_datasets": _sorted_counter(source_datasets),
        "source_partitions": _sorted_counter(source_partitions),
        "source_scenarios": _sorted_counter(source_scenarios),
        "source_intents": _sorted_counter(source_intents),
    }


def _surface_map(
    rows: Iterable[Mapping[str, Any]], extractor: Any
) -> dict[str, list[Mapping[str, Any]]]:
    result: dict[str, list[Mapping[str, Any]]] = defaultdict(list)
    for row in rows:
        result[normalize_text(extractor(row))].append(row)
    return result


def cross_split_diagnostics(
    split_rows: Mapping[str, Sequence[Mapping[str, Any]]], *, sample_limit: int
) -> dict[str, Any]:
    input_maps = {
        name: _surface_map(rows, _row_input) for name, rows in split_rows.items()
    }
    target_maps = {
        name: _surface_map(rows, _row_target) for name, rows in split_rows.items()
    }
    family_sets = {
        name: {
            str(_metadata(row).get("family_id"))
            for row in rows
            if _metadata(row).get("family_id") is not None
        }
        for name, rows in split_rows.items()
    }

    pair_reports: dict[str, Any] = {}
    for left, right in combinations(split_rows, 2):
        input_overlap = sorted(set(input_maps[left]) & set(input_maps[right]))
        target_overlap = sorted(set(target_maps[left]) & set(target_maps[right]))
        family_overlap = sorted(family_sets[left] & family_sets[right])
        pair_reports[f"{left}:{right}"] = {
            "normalized_input_overlap": {
                "count": len(input_overlap),
                "sample": input_overlap[:sample_limit],
            },
            "normalized_target_overlap": {
                "count": len(target_overlap),
                "sample": target_overlap[:sample_limit],
            },
            "source_family_overlap": {
                "count": len(family_overlap),
                "sample": family_overlap[:sample_limit],
            },
        }

    mappings: dict[str, set[str]] = defaultdict(set)
    examples: dict[tuple[str, str], list[str]] = defaultdict(list)
    for split, rows in split_rows.items():
        for row in rows:
            input_value = normalize_text(_row_input(row))
            target_value = normalize_text(_row_target(row))
            mappings[input_value].add(target_value)
            examples[(input_value, target_value)].append(split)

    conflicts = []
    for input_value, targets in sorted(mappings.items()):
        if len(targets) < 2:
            continue
        conflicts.append(
            {
                "input": input_value,
                "targets": [
                    {
                        "target": target,
                        "splits": sorted(set(examples[(input_value, target)])),
                    }
                    for target in sorted(targets)
                ],
            }
        )

    return {
        "pairs": pair_reports,
        "conflicting_input_targets": {
            "count": len(conflicts),
            "sample": conflicts[:sample_limit],
        },
    }


def build_dataset_quality_report(
    dataset_dir: Path,
    *,
    source_records_path: Path | None = None,
    protocol: str = DEFAULT_PROTOCOL,
    sample_limit: int = 10,
) -> dict[str, Any]:
    if sample_limit < 0:
        raise ValueError("sample_limit must be non-negative")
    split_paths = {name: dataset_dir / f"{name}.jsonl" for name in SPLIT_NAMES}
    split_rows = {name: load_jsonl(path) for name, path in split_paths.items()}
    source_index = load_massive_source_index(source_records_path)
    source_vocabulary = {
        token.casefold()
        for row in source_index.values()
        if isinstance(row.get("utt"), str)
        for token in ENGLISH_WORD_RE.findall(row["utt"])
    }

    split_reports: dict[str, Any] = {}
    for name in SPLIT_NAMES:
        split_reports[name] = split_diagnostics(
            split_rows[name],
            source_index=source_index,
            source_vocabulary=source_vocabulary,
            sample_limit=sample_limit,
        )
        split_reports[name]["sha256"] = sha256_file(split_paths[name])

    build_report_path = dataset_dir / "build_report.json"
    build_matches: dict[str, Any] = {"available": build_report_path.is_file()}
    if build_report_path.is_file():
        build_report = json.loads(build_report_path.read_text(encoding="utf-8"))
        matches = {}
        for name in SPLIT_NAMES:
            expected = build_report.get("splits", {}).get(name, {})
            matches[name] = {
                "rows": expected.get("rows") == split_reports[name]["rows"],
                "sha256": expected.get("sha256") == split_reports[name]["sha256"],
            }
        build_matches["splits"] = matches
        build_matches["all"] = all(
            value
            for split in matches.values()
            for value in split.values()
        )

    return {
        "schema_version": 1,
        "protocol": protocol,
        "source_domain_index": {
            "available": bool(source_index),
            "records": len(source_index),
            "path": str(source_records_path) if source_records_path is not None else None,
        },
        "splits": split_reports,
        "cross_split": cross_split_diagnostics(
            split_rows, sample_limit=sample_limit
        ),
        "build_report_matches": build_matches,
    }
