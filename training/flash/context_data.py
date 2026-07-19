"""Build and validate the reviewed Quip context corpus and mixed SFT input."""

from __future__ import annotations

import hashlib
import json
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any, Iterable, Mapping


EXPECTED_SOURCE_SHA256 = "db399c3ec92d7b17f587c64bf9e1de0bddc536ac2edbf08c6582fe03d315f225"
EXPECTED_SOURCE_ROWS = 300
EXPECTED_V2_TRAIN_ROWS = 5_000
SPLIT_ORDER = ("train", "eval", "test")
SPLIT_TARGETS = {"train": 0.80, "eval": 0.10, "test": 0.10}

# The source has ten rows with more than one runtime-invalid snippet. These
# selections are reviewable, fixed source-line choices. They avoid inventing or
# combining context while making the one-snippet projection reproducible.
MULTI_SNIPPET_SELECTIONS = {
    15: 1,
    28: 1,
    56: 0,
    149: 0,
    150: 1,
    151: 1,
    152: 0,
    244: 1,
    250: 0,
    264: 0,
}


def compact_json(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


def _normalized(value: str) -> str:
    return " ".join(value.casefold().split())


def _family_id(text: str) -> str:
    digest = sha256_bytes(_normalized(text).encode("utf-8"))[:20]
    return f"context_reviewed_v1:{digest}"


def _stable_rank(family_id: str) -> str:
    return sha256_bytes(f"quip-context-v1-split:{family_id}".encode("utf-8"))


def _source_rows(source_path: Path) -> list[tuple[int, dict[str, Any]]]:
    if sha256_file(source_path) != EXPECTED_SOURCE_SHA256:
        raise ValueError("approved context source SHA-256 does not match")
    rows: list[tuple[int, dict[str, Any]]] = []
    for line_number, line in enumerate(source_path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError as exc:
            raise ValueError(f"source:{line_number}: invalid JSON") from exc
        if not isinstance(row, dict) or set(row) != {"input", "output"}:
            raise ValueError(f"source:{line_number}: expected input and output only")
        rows.append((line_number, row))
    if len(rows) != EXPECTED_SOURCE_ROWS:
        raise ValueError(f"expected {EXPECTED_SOURCE_ROWS} source rows, got {len(rows)}")
    return rows


def _canonical_input(raw: object, *, source_line: int) -> tuple[dict[str, Any], int | None]:
    if not isinstance(raw, dict) or not isinstance(raw.get("text"), str):
        raise ValueError(f"source:{source_line}: input text must be a string")
    text = raw["text"]
    if not text or len(text) > 80:
        raise ValueError(f"source:{source_line}: input text is out of bounds")
    snippets = raw.get("context_snippets")
    if snippets is None:
        if set(raw) != {"text"}:
            raise ValueError(f"source:{source_line}: no-context input has extra fields")
        return {"text": text}, None
    if set(raw) != {"context_snippets", "text"} or not isinstance(snippets, list) or not snippets:
        raise ValueError(f"source:{source_line}: invalid context input")
    selected_index = 0
    if len(snippets) > 1:
        if source_line not in MULTI_SNIPPET_SELECTIONS:
            raise ValueError(f"source:{source_line}: missing multi-snippet selection")
        selected_index = MULTI_SNIPPET_SELECTIONS[source_line]
    elif source_line in MULTI_SNIPPET_SELECTIONS:
        raise ValueError(f"source:{source_line}: unexpected multi-snippet selection")
    if selected_index >= len(snippets):
        raise ValueError(f"source:{source_line}: invalid selected snippet index")
    snippet = snippets[selected_index]
    if not isinstance(snippet, dict) or set(snippet) != {"app_name", "window_title", "visible_text"}:
        raise ValueError(f"source:{source_line}: invalid snippet fields")
    limits = {"app_name": 80, "window_title": 120, "visible_text": 240}
    canonical_snippet = {}
    for key, limit in limits.items():
        value = snippet[key]
        if not isinstance(value, str):
            raise ValueError(f"source:{source_line}: invalid {key}")
        if not value:
            if key != "visible_text" or not isinstance(snippet["window_title"], str) or not snippet["window_title"]:
                raise ValueError(f"source:{source_line}: invalid {key}")
            # The runtime requires visible text. When the reviewed source has
            # none, expose its already retained window title instead.
            value = snippet["window_title"]
        if len(value) > limit:
            if key != "visible_text":
                raise ValueError(f"source:{source_line}: invalid {key}")
            # Runtime accepts at most 240 visible-text characters. Keep the
            # source row and project only this bounded model-facing field.
            value = value[:limit]
        canonical_snippet[key] = value
    return {"context_snippets": [canonical_snippet], "text": text}, selected_index


def _output(raw: object, *, source_line: int) -> dict[str, str]:
    if not isinstance(raw, dict) or set(raw) != {"suggestion"}:
        raise ValueError(f"source:{source_line}: output must contain suggestion only")
    suggestion = raw["suggestion"]
    if not isinstance(suggestion, str) or not suggestion or len(suggestion) > 320:
        raise ValueError(f"source:{source_line}: invalid suggestion")
    return {"suggestion": suggestion}


def _assign_splits(families: Mapping[str, list[dict[str, Any]]]) -> dict[str, str]:
    """Assign whole families by a stable hash while balancing row counts."""
    total_rows = sum(len(rows) for rows in families.values())
    targets = {name: total_rows * share for name, share in SPLIT_TARGETS.items()}
    counts = {name: 0 for name in SPLIT_ORDER}
    assignments: dict[str, str] = {}
    for family_id in sorted(families, key=_stable_rank):
        family_rows = len(families[family_id])
        destination = min(
            SPLIT_ORDER,
            key=lambda name: ((counts[name] + family_rows) / targets[name], SPLIT_ORDER.index(name)),
        )
        assignments[family_id] = destination
        counts[destination] += family_rows
    if any(not counts[name] for name in SPLIT_ORDER):
        raise ValueError("family split assignment left an empty split")
    return assignments


def _classify_context_row(
    model_input: Mapping[str, Any], suggestion: str, no_context_targets: Mapping[str, str]
) -> tuple[str, str, str]:
    draft = model_input["text"]
    if "context_snippets" not in model_input:
        return "context_preservation", "without_context", "not_retained"
    baseline = no_context_targets.get(draft)
    if baseline is not None and suggestion == baseline:
        return "context_hard_negative", "distractor_context", "synthetic_distractor"
    if baseline is None and suggestion == draft:
        return "context_preservation", "relevant_context", "necessary_for_label"
    return "context_disambiguation", "relevant_context", "necessary_for_label"


def compile_context_rows(source_path: Path) -> tuple[dict[str, list[dict[str, Any]]], dict[str, Any]]:
    parsed = []
    multi_snippet_rows: list[dict[str, int]] = []
    bounded_snippet_rows: list[dict[str, int]] = []
    visible_text_fallback_rows: list[dict[str, str | int]] = []
    for source_line, raw in _source_rows(source_path):
        model_input, selected_index = _canonical_input(raw["input"], source_line=source_line)
        output = _output(raw["output"], source_line=source_line)
        raw_snippet_count = len(raw["input"].get("context_snippets", []))
        if raw_snippet_count > 1:
            multi_snippet_rows.append(
                {"source_line": source_line, "selected_snippet_index": selected_index or 0}
            )
        if "context_snippets" in raw["input"]:
            raw_index = selected_index or 0
            raw_visible_text = raw["input"]["context_snippets"][raw_index]["visible_text"]
            if len(raw_visible_text) > len(model_input["context_snippets"][0]["visible_text"]):
                bounded_snippet_rows.append(
                    {
                        "source_line": source_line,
                        "field": "visible_text",
                        "source_characters": len(raw_visible_text),
                        "compiled_characters": len(model_input["context_snippets"][0]["visible_text"]),
                    }
                )
            if not raw_visible_text:
                visible_text_fallback_rows.append(
                    {
                        "source_line": source_line,
                        "field": "visible_text",
                        "replacement": "window_title",
                    }
                )
        parsed.append((source_line, model_input, output))

    if [item["source_line"] for item in multi_snippet_rows] != list(MULTI_SNIPPET_SELECTIONS):
        raise ValueError("multi-snippet source rows do not match the reviewed selection policy")

    no_context_targets = {
        model_input["text"]: output["suggestion"]
        for _, model_input, output in parsed
        if "context_snippets" not in model_input
    }
    families: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for source_line, model_input, output in parsed:
        draft = model_input["text"]
        family_id = _family_id(draft)
        category, context_variant, retention_basis = _classify_context_row(
            model_input, output["suggestion"], no_context_targets
        )
        family = families[family_id]
        family.append(
            {
                "source_line": source_line,
                "input": model_input,
                "output": output,
                "family_id": family_id,
                "category": category,
                "context_variant": context_variant,
                "retention_basis": retention_basis,
            }
        )

    assignments = _assign_splits(families)
    splits: dict[str, list[dict[str, Any]]] = {name: [] for name in SPLIT_ORDER}
    for family_id, family_rows in families.items():
        split = assignments[family_id]
        for item in family_rows:
            source_line = item["source_line"]
            model_input = item["input"]
            output = item["output"]
            suggestion = output["suggestion"]
            example_material = {
                "source_line": source_line,
                "input": model_input,
                "output": output,
            }
            example_id = sha256_bytes(compact_json(example_material).encode("utf-8"))[:20]
            splits[split].append(
                {
                    "input": model_input,
                    "output": output,
                    "metadata": {
                        "schema_version": "context_snippet_v1",
                        "example_id": f"ctx_reviewed_{example_id}",
                        "pair_id": item["family_id"],
                        "family_id": item["family_id"],
                        "source_dataset": "quip_context_human_reviewed_v1",
                        "source_record_id": f"line_{source_line:03d}",
                        "source_partition": split,
                        "source_license": "project-authored-human-reviewed",
                        "generation": {"method": "human_reviewed_source"},
                        "category": item["category"],
                        "context_variant": item["context_variant"],
                        "retention_basis": item["retention_basis"],
                        "target_changed": suggestion != model_input["text"],
                        "accepted_suggestions": [suggestion],
                        "window_size": len(model_input["text"].split()),
                        "label_evidence": {
                            "action": "human_reviewed",
                            "presented_candidate": None,
                            "resulting_text": suggestion,
                        },
                    },
                }
            )
    for rows in splits.values():
        rows.sort(key=lambda row: row["metadata"]["source_record_id"])

    source_report = {
        "source_sha256": EXPECTED_SOURCE_SHA256,
        "source_rows": EXPECTED_SOURCE_ROWS,
        "human_reviewed_provenance": {
            "source_dataset": "quip_context_human_reviewed_v1",
            "generation_method": "human_reviewed_source",
            "source_license": "project-authored-human-reviewed",
            "note": "Rows are compiled from the approved reviewed source without synthetic replacement or quarantine.",
        },
        "split_policy": {
            "name": "stable_hash_family_balancing_v1",
            "family_key": "normalized input.text",
            "target_row_shares": SPLIT_TARGETS,
        },
        "multi_snippet_policy": {
            "name": "reviewed_source_line_selection_v1",
            "source_rows": multi_snippet_rows,
            "runtime_max_snippets": 1,
        },
        "runtime_bounds_projection": {
            "visible_text_max_characters": 240,
            "truncated_rows": bounded_snippet_rows,
            "empty_visible_text_fallback_rows": visible_text_fallback_rows,
        },
        "families": len(families),
        "singleton_families": sum(len(rows) == 1 for rows in families.values()),
    }
    return splits, source_report


def _serialize_rows(rows: Iterable[dict[str, Any]]) -> bytes:
    return "".join(compact_json(row) + "\n" for row in rows).encode("utf-8")


def _split_report(rows: list[dict[str, Any]], payload: bytes) -> dict[str, Any]:
    return {
        "rows": len(rows),
        "sha256": sha256_bytes(payload),
        "changed": sum(row["metadata"]["target_changed"] for row in rows),
        "unchanged": sum(not row["metadata"]["target_changed"] for row in rows),
        "categories": dict(sorted(Counter(row["metadata"]["category"] for row in rows).items())),
        "context_variants": dict(
            sorted(Counter(row["metadata"]["context_variant"] for row in rows).items())
        ),
        "families": len({row["metadata"]["family_id"] for row in rows}),
    }


def build_context_dataset(source_path: Path, output_dir: Path, report_path: Path) -> dict[str, Any]:
    splits, source_report = compile_context_rows(source_path)
    output_dir.mkdir(parents=True, exist_ok=True)
    split_reports = {}
    for split in SPLIT_ORDER:
        payload = _serialize_rows(splits[split])
        (output_dir / f"{split}.jsonl").write_bytes(payload)
        split_reports[split] = _split_report(splits[split], payload)
    report = {
        "protocol": "quip_context_human_reviewed_v1",
        "schema_version": 1,
        **source_report,
        "compiled_rows": sum(item["rows"] for item in split_reports.values()),
        "splits": split_reports,
    }
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return report


def _load_rows(path: Path) -> list[dict[str, Any]]:
    rows = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError as exc:
            raise ValueError(f"{path}:{line_number}: invalid JSON") from exc
        if not isinstance(row, dict):
            raise ValueError(f"{path}:{line_number}: row must be an object")
        rows.append(row)
    return rows


def _validate_context_row(row: Mapping[str, Any], split: str, *, path: Path, line_number: int) -> None:
    if set(row) != {"input", "output", "metadata"}:
        raise ValueError(f"{path}:{line_number}: invalid row fields")
    model_input = row["input"]
    if not isinstance(model_input, dict) or not isinstance(model_input.get("text"), str):
        raise ValueError(f"{path}:{line_number}: invalid input")
    snippets = model_input.get("context_snippets")
    if snippets is None:
        if set(model_input) != {"text"}:
            raise ValueError(f"{path}:{line_number}: invalid no-context input")
    else:
        if set(model_input) != {"context_snippets", "text"} or not isinstance(snippets, list) or len(snippets) != 1:
            raise ValueError(f"{path}:{line_number}: context rows must have one snippet")
        snippet = snippets[0]
        if not isinstance(snippet, dict) or set(snippet) != {"app_name", "window_title", "visible_text"}:
            raise ValueError(f"{path}:{line_number}: invalid snippet")
    output = row["output"]
    metadata = row["metadata"]
    if not isinstance(output, dict) or set(output) != {"suggestion"} or not isinstance(output["suggestion"], str):
        raise ValueError(f"{path}:{line_number}: invalid output")
    if not isinstance(metadata, dict):
        raise ValueError(f"{path}:{line_number}: invalid metadata")
    if metadata.get("schema_version") != "context_snippet_v1":
        raise ValueError(f"{path}:{line_number}: wrong schema version")
    if metadata.get("source_partition") != split:
        raise ValueError(f"{path}:{line_number}: wrong source partition")
    if metadata.get("generation") != {"method": "human_reviewed_source"}:
        raise ValueError(f"{path}:{line_number}: human-reviewed provenance changed")
    if metadata.get("label_evidence", {}).get("resulting_text") != output["suggestion"]:
        raise ValueError(f"{path}:{line_number}: label evidence disagrees with output")
    if metadata.get("target_changed") != (output["suggestion"] != model_input["text"]):
        raise ValueError(f"{path}:{line_number}: target_changed is incorrect")
    if metadata.get("accepted_suggestions") != [output["suggestion"]]:
        raise ValueError(f"{path}:{line_number}: accepted suggestion is incorrect")


def validate_context_dataset(source_path: Path, output_dir: Path, report_path: Path) -> dict[str, Any]:
    expected_splits, expected_source_report = compile_context_rows(source_path)
    report = json.loads(report_path.read_text(encoding="utf-8"))
    if report.get("source_sha256") != EXPECTED_SOURCE_SHA256:
        raise ValueError("context report source hash is incorrect")
    if report.get("compiled_rows") != EXPECTED_SOURCE_ROWS:
        raise ValueError("context report does not compile all approved rows")
    if report.get("multi_snippet_policy") != expected_source_report["multi_snippet_policy"]:
        raise ValueError("context report multi-snippet policy is incorrect")
    if report.get("runtime_bounds_projection") != expected_source_report["runtime_bounds_projection"]:
        raise ValueError("context report runtime bounds projection is incorrect")

    split_rows: dict[str, list[dict[str, Any]]] = {}
    for split in SPLIT_ORDER:
        path = output_dir / f"{split}.jsonl"
        payload = path.read_bytes()
        rows = _load_rows(path)
        for line_number, row in enumerate(rows, 1):
            _validate_context_row(row, split, path=path, line_number=line_number)
        expected_payload = _serialize_rows(expected_splits[split])
        if payload != expected_payload:
            raise ValueError(f"{split}: compiled rows are not the deterministic expected output")
        if report.get("splits", {}).get(split) != _split_report(rows, payload):
            raise ValueError(f"{split}: report does not match compiled rows")
        split_rows[split] = rows

    seen_families: set[str] = set()
    source_record_ids: set[str] = set()
    for split in SPLIT_ORDER:
        families = {row["metadata"]["family_id"] for row in split_rows[split]}
        if seen_families & families:
            raise ValueError("context family leakage across splits")
        seen_families |= families
        source_record_ids |= {row["metadata"]["source_record_id"] for row in split_rows[split]}
    if len(source_record_ids) != EXPECTED_SOURCE_ROWS:
        raise ValueError("context source record IDs are not globally unique")
    return report


def _validate_v2_source(v2_train_path: Path, v2_report_path: Path) -> tuple[bytes, list[dict[str, Any]], dict[str, Any]]:
    report = json.loads(v2_report_path.read_text(encoding="utf-8"))
    payload = v2_train_path.read_bytes()
    rows = _load_rows(v2_train_path)
    if len(rows) != EXPECTED_V2_TRAIN_ROWS:
        raise ValueError("V2 source train split must contain exactly 5,000 rows")
    if report.get("splits", {}).get("train", {}).get("rows") != EXPECTED_V2_TRAIN_ROWS:
        raise ValueError("V2 source build report has an incorrect train count")
    if report["splits"]["train"].get("sha256") != sha256_bytes(payload):
        raise ValueError("V2 source build report hash does not match train data")
    return payload, rows, report


def build_mixed_dataset(
    *,
    context_source_path: Path,
    context_output_dir: Path,
    context_report_path: Path,
    v2_train_path: Path,
    v2_report_path: Path,
    output_dir: Path,
    report_path: Path,
) -> dict[str, Any]:
    validate_context_dataset(context_source_path, context_output_dir, context_report_path)
    v2_payload, v2_rows, v2_report = _validate_v2_source(v2_train_path, v2_report_path)
    context_payload = (context_output_dir / "train.jsonl").read_bytes()
    context_rows = _load_rows(context_output_dir / "train.jsonl")
    output_dir.mkdir(parents=True, exist_ok=True)
    mixed_payload = v2_payload + context_payload
    mixed_path = output_dir / "train.jsonl"
    mixed_path.write_bytes(mixed_payload)
    report = {
        "protocol": "quip_v2_plus_context_sft_v1",
        "schema_version": 1,
        "v2": {
            "train_rows": len(v2_rows),
            "train_sha256": sha256_bytes(v2_payload),
            "build_report_sha256": sha256_file(v2_report_path),
            "dataset_policy": v2_report.get("dataset_policy"),
        },
        "context": {
            "train_rows": len(context_rows),
            "train_sha256": sha256_bytes(context_payload),
            "context_build_report_sha256": sha256_file(context_report_path),
        },
        "mixed": {
            "rows": len(v2_rows) + len(context_rows),
            "sha256": sha256_bytes(mixed_payload),
            "v2_prefix_rows": len(v2_rows),
            "context_suffix_rows": len(context_rows),
        },
        "family_leakage": {
            "context_train_vs_context_eval_test": False,
            "v2_family_namespace_overlaps_context": False,
        },
    }
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return report


def validate_mixed_dataset(
    *,
    context_source_path: Path,
    context_output_dir: Path,
    context_report_path: Path,
    v2_train_path: Path,
    v2_report_path: Path,
    mixed_path: Path,
    mixed_report_path: Path,
    config_path: Path,
) -> dict[str, Any]:
    context_report = validate_context_dataset(
        context_source_path,
        context_output_dir,
        context_report_path,
    )
    v2_payload, v2_rows, _ = _validate_v2_source(v2_train_path, v2_report_path)
    context_payload = (context_output_dir / "train.jsonl").read_bytes()
    context_rows = _load_rows(context_output_dir / "train.jsonl")
    mixed_payload = mixed_path.read_bytes()
    report = json.loads(mixed_report_path.read_text(encoding="utf-8"))
    if mixed_payload != v2_payload + context_payload:
        raise ValueError("mixed training data must preserve the V2 prefix and all context suffix rows")
    if report.get("mixed", {}).get("rows") != len(v2_rows) + len(context_rows):
        raise ValueError("mixed report row count is incorrect")
    if report["mixed"].get("sha256") != sha256_bytes(mixed_payload):
        raise ValueError("mixed report hash is incorrect")
    if report.get("v2", {}).get("train_rows") != EXPECTED_V2_TRAIN_ROWS:
        raise ValueError("mixed report does not preserve all 5,000 V2 rows")

    context_families = {
        row["metadata"]["family_id"]
        for split in ("train", "eval", "test")
        for row in _load_rows(context_output_dir / f"{split}.jsonl")
    }
    v2_families = {row.get("metadata", {}).get("family_id") for row in v2_rows}
    if context_families & v2_families:
        raise ValueError("V2 and context source-family namespaces overlap")

    max_examples = None
    for line in config_path.read_text(encoding="utf-8").splitlines():
        if line.strip().startswith("max_examples"):
            max_examples = int(line.split("=", 1)[1].strip())
            break
    if max_examples is None or max_examples < len(v2_rows) + len(context_rows):
        raise ValueError("max_examples would truncate context training rows")
    return {
        "context_rows": context_report["splits"]["train"]["rows"],
        "mixed_rows": len(v2_rows) + len(context_rows),
        "max_examples": max_examples,
    }
