"""Build or verify the reviewed context corpus and mixed V2 SFT training data."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from context_data import (  # noqa: E402
    build_context_dataset,
    build_mixed_dataset,
    validate_context_dataset,
    validate_mixed_dataset,
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify-only", action="store_true")
    parser.add_argument("--context-source", type=Path, default=ROOT / "context_data" / "source.jsonl")
    parser.add_argument("--v2-dir", type=Path, default=ROOT / "context_data" / "v2-runtime-10w-5k")
    parser.add_argument("--output-root", type=Path, default=ROOT / "context_data")
    parser.add_argument("--report-dir", type=Path, default=ROOT / "context_data" / "reports")
    parser.add_argument("--config", type=Path, default=ROOT / "configs" / "sft-v2-context-qwen-2b.toml")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    context_output = args.output_root / "context"
    mixed_output = args.output_root / "mixed"
    context_report = args.report_dir / "context-build-report.json"
    mixed_report = args.report_dir / "mixed-build-report.json"
    if args.verify_only:
        validate_context_dataset(args.context_source, context_output, context_report)
        summary = validate_mixed_dataset(
            context_source_path=args.context_source,
            context_output_dir=context_output,
            context_report_path=context_report,
            v2_train_path=args.v2_dir / "train.jsonl",
            v2_report_path=args.v2_dir / "build_report.json",
            mixed_path=mixed_output / "train.jsonl",
            mixed_report_path=mixed_report,
            config_path=args.config,
        )
    else:
        build_context_dataset(args.context_source, context_output, context_report)
        build_mixed_dataset(
            context_source_path=args.context_source,
            context_output_dir=context_output,
            context_report_path=context_report,
            v2_train_path=args.v2_dir / "train.jsonl",
            v2_report_path=args.v2_dir / "build_report.json",
            output_dir=mixed_output,
            report_path=mixed_report,
        )
        summary = validate_mixed_dataset(
            context_source_path=args.context_source,
            context_output_dir=context_output,
            context_report_path=context_report,
            v2_train_path=args.v2_dir / "train.jsonl",
            v2_report_path=args.v2_dir / "build_report.json",
            mixed_path=mixed_output / "train.jsonl",
            mixed_report_path=mixed_report,
            config_path=args.config,
        )
    print(
        f"validated {summary['mixed_rows']} mixed training rows "
        f"with {summary['context_rows']} context rows and max_examples={summary['max_examples']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
