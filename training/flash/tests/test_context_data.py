from __future__ import annotations

import json
from pathlib import Path

import pytest

from context_data import (
    EXPECTED_SOURCE_ROWS,
    EXPECTED_SOURCE_SHA256,
    EXPECTED_V2_TRAIN_ROWS,
    MULTI_SNIPPET_SELECTIONS,
    build_context_dataset,
    build_mixed_dataset,
    sha256_file,
    validate_context_dataset,
    validate_mixed_dataset,
)


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "context_data" / "source.jsonl"
V2_DIR = ROOT / "context_data" / "v2-runtime-10w-5k"
CONFIG = ROOT / "configs" / "sft-v2-context-qwen-2b.toml"


def _build(tmp_path: Path):
    context_output = tmp_path / "context"
    reports = tmp_path / "reports"
    context_report = reports / "context-build-report.json"
    build_context_dataset(SOURCE, context_output, context_report)
    mixed_output = tmp_path / "mixed"
    mixed_report = reports / "mixed-build-report.json"
    build_mixed_dataset(
        context_source_path=SOURCE,
        context_output_dir=context_output,
        context_report_path=context_report,
        v2_train_path=V2_DIR / "train.jsonl",
        v2_report_path=V2_DIR / "build_report.json",
        output_dir=mixed_output,
        report_path=mixed_report,
    )
    return context_output, context_report, mixed_output, mixed_report


def test_context_build_preserves_all_reviewed_rows_and_one_snippet_policy(tmp_path: Path):
    context_output, context_report, _, _ = _build(tmp_path)
    report = validate_context_dataset(SOURCE, context_output, context_report)

    assert sha256_file(SOURCE) == EXPECTED_SOURCE_SHA256
    assert report["source_rows"] == EXPECTED_SOURCE_ROWS
    assert report["compiled_rows"] == EXPECTED_SOURCE_ROWS
    assert report["multi_snippet_policy"]["runtime_max_snippets"] == 1
    assert [item["source_line"] for item in report["multi_snippet_policy"]["source_rows"]] == list(MULTI_SNIPPET_SELECTIONS)

    families = {}
    rows = []
    for split in ("train", "eval", "test"):
        split_rows = [json.loads(line) for line in (context_output / f"{split}.jsonl").read_text(encoding="utf-8").splitlines()]
        rows.extend(split_rows)
        for row in split_rows:
            assert len(row["input"].get("context_snippets", [])) <= 1
            families.setdefault(row["metadata"]["family_id"], set()).add(split)
    assert len(rows) == EXPECTED_SOURCE_ROWS
    assert all(len(splits) == 1 for splits in families.values())


def test_context_build_is_deterministic(tmp_path: Path):
    first_output = tmp_path / "first"
    second_output = tmp_path / "second"
    first_report = tmp_path / "first-report.json"
    second_report = tmp_path / "second-report.json"
    build_context_dataset(SOURCE, first_output, first_report)
    build_context_dataset(SOURCE, second_output, second_report)

    assert first_report.read_bytes() == second_report.read_bytes()
    for split in ("train", "eval", "test"):
        assert (first_output / f"{split}.jsonl").read_bytes() == (second_output / f"{split}.jsonl").read_bytes()


def test_mixed_build_keeps_all_v2_and_context_training_rows(tmp_path: Path):
    context_output, context_report, mixed_output, mixed_report = _build(tmp_path)
    summary = validate_mixed_dataset(
        context_source_path=SOURCE,
        context_output_dir=context_output,
        context_report_path=context_report,
        v2_train_path=V2_DIR / "train.jsonl",
        v2_report_path=V2_DIR / "build_report.json",
        mixed_path=mixed_output / "train.jsonl",
        mixed_report_path=mixed_report,
        config_path=CONFIG,
    )
    report = json.loads(mixed_report.read_text(encoding="utf-8"))

    assert report["v2"]["train_rows"] == EXPECTED_V2_TRAIN_ROWS
    assert report["mixed"]["rows"] == EXPECTED_V2_TRAIN_ROWS + summary["context_rows"]
    assert summary["max_examples"] >= report["mixed"]["rows"]
    assert (mixed_output / "train.jsonl").read_bytes().startswith((V2_DIR / "train.jsonl").read_bytes())


def test_context_validator_rejects_modified_split(tmp_path: Path):
    context_output, context_report, _, _ = _build(tmp_path)
    eval_path = context_output / "eval.jsonl"
    eval_path.write_text(eval_path.read_text(encoding="utf-8") + "{}\n", encoding="utf-8")

    with pytest.raises(ValueError, match="invalid row fields"):
        validate_context_dataset(SOURCE, context_output, context_report)
