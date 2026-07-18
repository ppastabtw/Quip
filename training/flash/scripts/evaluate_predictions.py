"""Score JSONL predictions against the held-out Quip dataset."""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from scoring import score_completion  # noqa: E402


def _rate(numerator: int, denominator: int) -> float:
    return round(numerator / denominator, 4) if denominator else 0.0


def load_jsonl(path: Path) -> list[dict]:
    with path.open(encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def evaluate(dataset_path: Path, predictions_path: Path) -> dict:
    rows = load_jsonl(dataset_path)
    predictions = {
        item["example_id"]: item
        for item in load_jsonl(predictions_path)
        if isinstance(item.get("example_id"), str)
    }

    totals = defaultdict(int)
    categories: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    latency_values = []

    for row in rows:
        metadata = row["metadata"]
        example_id = metadata["example_id"]
        category = metadata["category"]
        prediction_row = predictions.get(example_id)
        totals["examples"] += 1
        categories[category]["examples"] += 1

        if prediction_row is None or not isinstance(prediction_row.get("response"), str):
            totals["missing"] += 1
            categories[category]["missing"] += 1
            continue

        result = score_completion(
            input_text=row["input"],
            expected_output=row["output"],
            metadata=metadata,
            response_text=prediction_row["response"],
        )
        for key, value in (
            ("schema_valid", result.schema_valid),
            ("change_correct", result.change_correct),
            ("content_correct", result.content_correct),
            ("protected_preserved", result.protected_preserved),
            ("success", result.success),
        ):
            if value:
                totals[key] += 1
                categories[category][key] += 1

        if metadata.get("target_changed") is False:
            totals["unchanged_examples"] += 1
            categories[category]["unchanged_examples"] += 1
            input_text = json.loads(row["input"])["text"]
            if (
                result.prediction is not None
                and result.prediction.suggestion != input_text
            ):
                totals["unnecessary_edits"] += 1
                categories[category]["unnecessary_edits"] += 1

        latency = prediction_row.get("latency_ms")
        if isinstance(latency, (int, float)) and latency >= 0:
            latency_values.append(float(latency))

    report = {
        "examples": totals["examples"],
        "missing_predictions": totals["missing"],
        "schema_validity": _rate(totals["schema_valid"], totals["examples"]),
        "change_accuracy": _rate(totals["change_correct"], totals["examples"]),
        "decode_success": _rate(totals["content_correct"], totals["examples"]),
        "protected_token_preservation": _rate(totals["protected_preserved"], totals["examples"]),
        "unnecessary_edit_rate": _rate(
            totals["unnecessary_edits"], totals["unchanged_examples"]
        ),
        "overall_success": _rate(totals["success"], totals["examples"]),
        "mean_latency_ms": round(sum(latency_values) / len(latency_values), 2) if latency_values else None,
        "categories": {},
    }
    for category, values in sorted(categories.items()):
        count = values["examples"]
        report["categories"][category] = {
            "examples": count,
            "success_rate": _rate(values["success"], count),
            "unnecessary_edit_rate": _rate(
                values["unnecessary_edits"], values["unchanged_examples"]
            ),
        }
    return report


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("predictions", type=Path)
    parser.add_argument("--dataset", type=Path, default=ROOT / "dataset" / "eval.jsonl")
    args = parser.parse_args()
    print(json.dumps(evaluate(args.dataset, args.predictions), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
