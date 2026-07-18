"""Build or verify the sourced Quip Flash datasets."""

from __future__ import annotations

import argparse
import shutil
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from dataset_compiler.compiler import compile_datasets, verify_only  # noqa: E402
from dataset_compiler.contract import CACHE_DIR, CONTRACT, BuildError, TEACHER_CACHE_DIR  # noqa: E402


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--train-size", type=int, default=CONTRACT.train_size)
    parser.add_argument("--eval-size", type=int, default=CONTRACT.eval_size)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--verify-only", action="store_true")
    parser.add_argument("--refresh-teacher", action="store_true")
    args = parser.parse_args()
    if (args.train_size, args.eval_size) != (CONTRACT.train_size, CONTRACT.eval_size):
        parser.error(
            f"the v1 contract requires exactly {CONTRACT.train_size} training and "
            f"{CONTRACT.eval_size} evaluation rows"
        )
    if args.verify_only and args.refresh_teacher:
        parser.error("--verify-only cannot be combined with --refresh-teacher")
    if args.offline and args.refresh_teacher:
        parser.error("--offline cannot be combined with --refresh-teacher")
    return args


def clear_teacher_cache() -> None:
    if not TEACHER_CACHE_DIR.exists():
        return
    resolved = TEACHER_CACHE_DIR.resolve()
    if resolved.parent != CACHE_DIR.resolve():
        raise BuildError("refusing to clear teacher cache outside the data cache")
    shutil.rmtree(resolved)


def main() -> int:
    args = parse_args()
    if args.verify_only:
        verify_only()
        return 0
    if args.refresh_teacher:
        clear_teacher_cache()
    report = compile_datasets(seed=args.seed, offline=args.offline)
    verify_only()
    print(
        f"built {report['splits']['train']['rows']} train and "
        f"{report['splits']['eval']['rows']} eval rows"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
