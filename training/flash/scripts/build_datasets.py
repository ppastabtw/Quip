"""Build or verify the sourced Quip Flash datasets."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from dataset_compiler.compiler import POLICIES, compile_datasets, verify_only  # noqa: E402
from dataset_compiler.contract import DATASET_DIR  # noqa: E402


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--verify-only", action="store_true")
    parser.add_argument(
        "--policy",
        choices=tuple(POLICIES),
        default="massive_window_augmentation_v1",
    )
    parser.add_argument("--output-dir", type=Path, default=DATASET_DIR)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.verify_only:
        if args.policy != "massive_window_augmentation_v1" or args.output_dir != DATASET_DIR:
            raise SystemExit("--verify-only validates only the checked-in V1 dataset")
        verify_only()
        return 0
    report = compile_datasets(
        seed=args.seed,
        offline=args.offline,
        policy=POLICIES[args.policy],
        output_dir=args.output_dir,
    )
    print(
        f"built {report['splits']['train']['rows']} train and "
        f"{report['splits']['eval']['rows']} eval and "
        f"{report['splits']['test']['rows']} test rows"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
