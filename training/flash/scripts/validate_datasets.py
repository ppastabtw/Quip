"""Validate Quip Flash rows, gold outputs, and held-out separation."""

from __future__ import annotations

import json
import sys
from collections import Counter
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from scoring import score_completion  # noqa: E402


ALLOWED_ROW_KEYS = {"input", "output", "metadata"}


def load_rows(path: Path) -> list[dict]:
    rows = []
    with path.open(encoding="utf-8") as handle:
        for line_number, line in enumerate(handle, 1):
            if not line.strip():
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError as exc:
                raise ValueError(f"{path}:{line_number}: invalid JSON: {exc}") from exc
            if not isinstance(row, dict) or set(row) != ALLOWED_ROW_KEYS:
                raise ValueError(f"{path}:{line_number}: row keys must be input, output, metadata")
            if not isinstance(row["input"], str) or not isinstance(row["output"], str):
                raise ValueError(f"{path}:{line_number}: input and output must be strings")
            if not isinstance(row["metadata"], dict):
                raise ValueError(f"{path}:{line_number}: metadata must be an object")
            result = score_completion(
                input_text=row["input"],
                expected_output=row["output"],
                metadata=row["metadata"],
                response_text=row["output"],
            )
            if result.score != 1.0 or not result.success:
                raise ValueError(f"{path}:{line_number}: gold output does not earn full reward: {result.reason}")
            rows.append(row)
    if not rows:
        raise ValueError(f"{path}: dataset is empty")
    return rows


def main() -> int:
    train_path = ROOT / "dataset" / "train.jsonl"
    eval_path = ROOT / "dataset" / "eval.jsonl"
    train_rows = load_rows(train_path)
    eval_rows = load_rows(eval_path)

    train_inputs = {row["input"] for row in train_rows}
    eval_inputs = {row["input"] for row in eval_rows}
    overlap = train_inputs & eval_inputs
    if overlap:
        raise ValueError(f"train and eval contain {len(overlap)} duplicate inputs")

    ids = [row["metadata"].get("example_id") for row in train_rows + eval_rows]
    if any(not isinstance(example_id, str) or not example_id for example_id in ids):
        raise ValueError("every row requires metadata.example_id")
    if len(set(ids)) != len(ids):
        raise ValueError("metadata.example_id values must be unique")

    for name, rows in (("train", train_rows), ("eval", eval_rows)):
        counts = Counter(row["metadata"].get("category", "missing") for row in rows)
        summary = ", ".join(f"{category}={count}" for category, count in sorted(counts.items()))
        print(f"{name}: {len(rows)} rows ({summary})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
