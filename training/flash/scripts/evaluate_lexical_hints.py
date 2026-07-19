"""Measure dictionary hint coverage without calling a model."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from lexical_candidates import default_generator, enrich_model_input  # noqa: E402


def load_jsonl(path: Path) -> list[dict]:
    with path.open(encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def evaluate(rows: list[dict]) -> dict:
    generator = default_generator()
    changed_examples = 0
    examples_with_hints = 0
    changed_examples_with_hints = 0
    changed_examples_with_gold_hint = 0
    total_hints = 0

    for row in rows:
        enriched = enrich_model_input(row["input"], generator)
        hints = enriched["lexical_hints"]
        target_changed = row["metadata"]["target_changed"] is True
        if target_changed:
            changed_examples += 1
        if hints:
            examples_with_hints += 1
            total_hints += len(hints)
            if target_changed:
                changed_examples_with_hints += 1

        accepted_tokens = {
            token.casefold().strip(".,!?;:'\"")
            for suggestion in row["metadata"]["accepted_suggestions"]
            for token in suggestion.split()
        }
        if target_changed and any(
            candidate.casefold().strip(".,!?;:'\"") in accepted_tokens
            for hint in hints
            for candidate in hint["candidates"]
        ):
            changed_examples_with_gold_hint += 1

    def rate(numerator: int, denominator: int) -> float:
        return round(numerator / denominator, 4) if denominator else 0.0

    return {
        "protocol": "lexical_hints_v1",
        "examples": len(rows),
        "changed_examples": changed_examples,
        "examples_with_hints": examples_with_hints,
        "changed_examples_with_hints": changed_examples_with_hints,
        "changed_hint_coverage": rate(changed_examples_with_hints, changed_examples),
        "changed_gold_hint_recall": rate(
            changed_examples_with_gold_hint, changed_examples
        ),
        "mean_hints_when_present": round(total_hints / examples_with_hints, 3)
        if examples_with_hints
        else 0.0,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dataset", type=Path, default=ROOT / "dataset" / "eval.jsonl"
    )
    args = parser.parse_args()
    print(json.dumps(evaluate(load_jsonl(args.dataset)), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
