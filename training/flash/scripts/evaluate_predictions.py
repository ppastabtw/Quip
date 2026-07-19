"""Score JSONL predictions against the held-out Quip dataset."""

from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from scoring import parse_prediction, rank_candidate_items, score_completion  # noqa: E402


COMPLETIONS_PER_EXAMPLE = 5


def _rate(numerator: int, denominator: int) -> float:
    return round(numerator / denominator, 4) if denominator else 0.0


def load_jsonl(path: Path) -> list[dict]:
    with path.open(encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def evaluate(
    dataset_path: Path, predictions_path: Path, limit: int | None = None
) -> dict:
    rows = load_jsonl(dataset_path)
    if limit is not None:
        rows = rows[:limit]
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

        if prediction_row is None:
            totals["missing"] += 1
            categories[category]["missing"] += 1
            continue

        raw_responses = prediction_row.get("responses")
        if not isinstance(raw_responses, list):
            legacy_response = prediction_row.get("response")
            raw_responses = [legacy_response] if isinstance(legacy_response, str) else []
        responses = raw_responses[:COMPLETIONS_PER_EXAMPLE]
        if len(raw_responses) != COMPLETIONS_PER_EXAMPLE:
            totals["incomplete_batches"] += 1
            categories[category]["incomplete_batches"] += 1

        results = []
        candidate_items = []
        for completion_index, response_text in enumerate(responses, 1):
            if not isinstance(response_text, str):
                continue
            result = score_completion(
                input_text=row["input"],
                expected_output=row["output"],
                metadata=metadata,
                response_text=response_text,
            )
            results.append(result)
            for key, value in (
                ("schema_valid_completions", result.schema_valid),
                ("change_correct_completions", result.change_correct),
                ("content_correct_completions", result.content_correct),
                ("successful_completions", result.success),
            ):
                if value:
                    totals[key] += 1
                    categories[category][key] += 1
            if result.prediction is not None:
                candidate_items.append(
                    {
                        "index": completion_index,
                        "suggestion": result.prediction.suggestion,
                        "result": result,
                    }
                )

        input_text = row["input"]["text"]
        ranked_candidates = rank_candidate_items(candidate_items, input_text)
        batch_schema_valid = (
            len(responses) == COMPLETIONS_PER_EXAMPLE
            and len(results) == COMPLETIONS_PER_EXAMPLE
            and all(result.schema_valid for result in results)
        )
        target_changed = metadata.get("target_changed") is True
        top_candidate_success = (
            bool(ranked_candidates)
            and ranked_candidates[0]["result"].success
            if target_changed
            else not ranked_candidates
        )
        pass_at_five = (
            any(result.success for result in results)
            if target_changed
            else not ranked_candidates
        )
        if batch_schema_valid and top_candidate_success:
            totals["success"] += 1
            categories[category]["success"] += 1
        if batch_schema_valid and pass_at_five:
            totals["pass_at_five"] += 1
            categories[category]["pass_at_five"] += 1

        if not target_changed:
            totals["unchanged_examples"] += 1
            categories[category]["unchanged_examples"] += 1
            if ranked_candidates:
                totals["unnecessary_edits"] += 1
                categories[category]["unnecessary_edits"] += 1

        latency = prediction_row.get("latency_ms")
        if isinstance(latency, (int, float)) and latency >= 0:
            latency_values.append(float(latency))

    completion_slots = totals["examples"] * COMPLETIONS_PER_EXAMPLE
    report = {
        "evaluation_protocol": "five_completion_ranked_candidates_v1",
        "completions_per_example": COMPLETIONS_PER_EXAMPLE,
        "examples": totals["examples"],
        "missing_predictions": totals["missing"],
        "incomplete_batch_rate": _rate(
            totals["incomplete_batches"], totals["examples"]
        ),
        "schema_validity": _rate(
            totals["schema_valid_completions"], completion_slots
        ),
        "change_accuracy": _rate(
            totals["change_correct_completions"], completion_slots
        ),
        "decode_success": _rate(
            totals["content_correct_completions"], completion_slots
        ),
        "mean_completion_success": _rate(
            totals["successful_completions"], completion_slots
        ),
        "candidate_recall_at_5": _rate(
            totals["pass_at_five"], totals["examples"]
        ),
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
            "candidate_recall_at_5": _rate(values["pass_at_five"], count),
            "mean_completion_success": _rate(
                values["successful_completions"],
                count * COMPLETIONS_PER_EXAMPLE,
            ),
            "unnecessary_edit_rate": _rate(
                values["unnecessary_edits"], values["unchanged_examples"]
            ),
        }
    return report


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("predictions", type=Path)
    parser.add_argument("--dataset", type=Path, default=ROOT / "dataset" / "eval.jsonl")
    parser.add_argument("--limit", type=int)
    args = parser.parse_args()
    print(
        json.dumps(
            evaluate(args.dataset, args.predictions, args.limit),
            indent=2,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
