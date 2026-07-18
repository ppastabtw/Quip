"""Validate Quip Flash smoke rows or the complete sourced corpus."""

from __future__ import annotations

import argparse
import json
import re
import sys
import unicodedata
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from dataset_compiler.contract import DATASET_DIR, REPORT_PATH, validate_compiled_datasets  # noqa: E402
from scoring import parse_prediction, score_completion  # noqa: E402


SMOKE_METADATA_KEYS = {
    "example_id",
    "category",
    "target_changed",
    "accepted_suggestions",
    "protected_tokens",
}


def normalize(value: str) -> str:
    value = unicodedata.normalize("NFKC", value)
    return re.sub(r"\s+", " ", value).strip().casefold()


def validate_smoke_split(path: Path) -> tuple[set[str], set[str]]:
    inputs: set[str] = set()
    example_ids: set[str] = set()
    rows = 0
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if not line.strip():
                continue
            row = json.loads(line)
            if not isinstance(row, dict) or set(row) != {"input", "output", "metadata"}:
                raise ValueError(f"{path}:{line_number}: invalid row shape")
            input_payload = json.loads(row["input"])
            if (
                not isinstance(input_payload, dict)
                or set(input_payload) != {"text"}
                or not isinstance(input_payload["text"], str)
                or not input_payload["text"].strip()
            ):
                raise ValueError(f"{path}:{line_number}: invalid input")
            prediction = parse_prediction(row["output"])
            metadata = row["metadata"]
            if not isinstance(metadata, dict) or set(metadata) != SMOKE_METADATA_KEYS:
                raise ValueError(f"{path}:{line_number}: invalid smoke metadata")
            if not isinstance(metadata["target_changed"], bool):
                raise ValueError(f"{path}:{line_number}: target_changed must be boolean")
            if metadata["target_changed"] != (
                prediction.suggestion != input_payload["text"]
            ):
                raise ValueError(f"{path}:{line_number}: target_changed is incorrect")
            accepted = metadata["accepted_suggestions"]
            if (
                not isinstance(accepted, list)
                or not accepted
                or not all(isinstance(item, str) and item.strip() for item in accepted)
                or normalize(prediction.suggestion) not in {normalize(item) for item in accepted}
            ):
                raise ValueError(f"{path}:{line_number}: invalid accepted_suggestions")
            if not isinstance(metadata["protected_tokens"], list) or not all(
                isinstance(item, str) and item for item in metadata["protected_tokens"]
            ):
                raise ValueError(f"{path}:{line_number}: invalid protected_tokens")
            result = score_completion(
                input_text=row["input"],
                expected_output=row["output"],
                metadata=metadata,
                response_text=row["output"],
            )
            if result.score != 1.0 or not result.success:
                raise ValueError(
                    f"{path}:{line_number}: gold output failed reward: {result.reason}"
                )
            normalized_input = normalize(input_payload["text"])
            if normalized_input in inputs:
                raise ValueError(f"{path}:{line_number}: duplicate input")
            example_id = metadata["example_id"]
            if not isinstance(example_id, str) or not example_id or example_id in example_ids:
                raise ValueError(f"{path}:{line_number}: invalid or duplicate example_id")
            inputs.add(normalized_input)
            example_ids.add(example_id)
            rows += 1
    if not rows:
        raise ValueError(f"{path}: dataset is empty")
    print(f"{path.stem}: {rows} smoke rows")
    return inputs, example_ids


def validate_smoke_datasets() -> None:
    train_inputs, train_ids = validate_smoke_split(DATASET_DIR / "train.jsonl")
    eval_inputs, eval_ids = validate_smoke_split(DATASET_DIR / "eval.jsonl")
    if train_inputs & eval_inputs:
        raise ValueError("smoke train and eval inputs overlap")
    if train_ids & eval_ids:
        raise ValueError("smoke train and eval example IDs overlap")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--smoke",
        action="store_true",
        help="validate the small integration corpus without sourced-build quotas",
    )
    args = parser.parse_args()
    if args.smoke:
        validate_smoke_datasets()
        return 0
    validate_compiled_datasets(
        train_path=DATASET_DIR / "train.jsonl",
        eval_path=DATASET_DIR / "eval.jsonl",
        report_path=REPORT_PATH,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
