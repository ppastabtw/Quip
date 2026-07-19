#!/usr/bin/env python3
"""Generate, judge, and build context-aware synthetic Quip training data."""

from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path


FLASH_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(FLASH_ROOT))

from synthetic_data.config import load_config  # noqa: E402
from synthetic_data.pipeline import SyntheticPipeline  # noqa: E402
from synthetic_data.provider import (  # noqa: E402
    BackboardStructuredClient,
    MockStructuredClient,
)
from synthetic_data.scheduling import allocate_counts, make_slots  # noqa: E402


REPO_ROOT = FLASH_ROOT.parents[1]
DEFAULT_CONFIG = FLASH_ROOT / "configs" / "synthetic-context-v1.toml"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("plan", "generate", "judge", "build-dataset", "run"))
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--count", type=int, help="override final target count")
    parser.add_argument(
        "--candidate-count",
        type=int,
        help="cap generated candidates independently of the final target; useful when rescoping a checkpointed run",
    )
    parser.add_argument("--category", action="append", dest="categories", help="limit generation to a category; repeatable")
    parser.add_argument("--generator-model", help="Backboard provider/model slug")
    parser.add_argument("--judge-model", help="Backboard provider/model slug")
    parser.add_argument("--run-id", help="stable run ID required to resume separate stages")
    parser.add_argument("--output-dir", type=Path, help="run artifact directory")
    parser.add_argument(
        "--avoid-dataset",
        action="append",
        type=Path,
        default=[],
        help="existing train JSONL to condition diversity and reject cross-dataset duplicates; repeatable",
    )
    parser.add_argument("--mock", action="store_true", help="use deterministic local fixture responses; spends no API credit")
    parser.add_argument("--round", type=int, default=1, help="generation round for staged/resumed runs")
    return parser.parse_args()


def default_run_id() -> str:
    return datetime.now(timezone.utc).strftime("synthetic-%Y%m%dT%H%M%SZ")


def main() -> None:
    args = parse_args()
    config = load_config(args.config).with_overrides(
        count=args.count,
        categories=args.categories,
        generator_model=args.generator_model,
        judge_model=args.judge_model,
    )
    run_id = args.run_id or default_run_id()
    output_dir = args.output_dir or REPO_ROOT / "artifacts" / "synthetic" / run_id

    if args.command == "plan":
        slots = make_slots(config)
        if args.candidate_count is not None:
            slots = slots[: args.candidate_count]
        print(
            json.dumps(
                {
                    "run_id": run_id,
                    "output_dir": str(output_dir.resolve()),
                    "target_count": config.run.target_count,
                    "scheduled_candidates_per_first_round": len(slots),
                    "estimated_generator_requests": len(slots) // config.run.batch_size + bool(len(slots) % config.run.batch_size),
                    "estimated_judge_requests_if_all_valid": len(slots) // config.run.judge_batch_size + bool(len(slots) % config.run.judge_batch_size),
                    "target_behavior_counts": allocate_counts(config.run.target_count, config.behaviors),
                    "generator_model": config.generator.slug,
                    "judge_model": config.judge.slug,
                    "network_requests": False,
                },
                indent=2,
                sort_keys=True,
            )
        )
        return

    if args.mock:
        client = MockStructuredClient()
    else:
        if config.generator.provider == "mock" or config.judge.provider == "mock":
            raise SystemExit("real runs require --generator-model and --judge-model provider/model")
        client = BackboardStructuredClient(
            timeout_seconds=config.run.timeout_seconds,
            requests_per_minute=config.run.requests_per_minute,
        )

    pipeline = SyntheticPipeline(
        config=config,
        client=client,
        output_dir=output_dir,
        run_id=run_id,
        reference_paths=args.avoid_dataset,
        candidate_count=args.candidate_count,
    )
    try:
        if args.command == "generate":
            pipeline.validate_models()
            result = pipeline.generate(round_number=args.round)
        elif args.command == "judge":
            pipeline.validate_models()
            result = pipeline.judge()
        elif args.command == "build-dataset":
            result = pipeline.build()
        else:
            result = pipeline.run()
        print(json.dumps(result, indent=2, sort_keys=True))
        print(f"artifacts: {output_dir.resolve()}")
    finally:
        client.close()


if __name__ == "__main__":
    main()
