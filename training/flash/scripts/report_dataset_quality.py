"""Print deterministic, non-mutating diagnostics for the Quip dataset."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from dataset_quality import DEFAULT_PROTOCOL, build_dataset_quality_report  # noqa: E402


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dataset-dir", type=Path, default=ROOT / "dataset")
    parser.add_argument(
        "--source-records",
        type=Path,
        default=ROOT / ".data-cache" / "sources" / "massive-en-US.jsonl",
        help="optional MASSIVE JSONL used only for scenario and intent diagnostics",
    )
    parser.add_argument("--protocol", default=DEFAULT_PROTOCOL)
    parser.add_argument("--sample-limit", type=int, default=10)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    report = build_dataset_quality_report(
        args.dataset_dir,
        source_records_path=args.source_records,
        protocol=args.protocol,
        sample_limit=args.sample_limit,
    )
    print(json.dumps(report, indent=2, sort_keys=True, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
