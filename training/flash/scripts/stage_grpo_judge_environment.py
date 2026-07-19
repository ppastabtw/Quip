"""Build a self-contained Freesolo judge environment publish directory."""

from __future__ import annotations

import argparse
import shutil
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = ROOT / ".data-cache" / "grpo-judge-mixed"
SOURCE_FILES = (
    (ROOT / "grpo_judge" / "environment.py", "environment.py"),
    (ROOT / "grpo_judge" / "judge_reward.py", "judge_reward.py"),
    (ROOT / "model_input.py", "model_input.py"),
    (ROOT / "lexical_candidates.py", "lexical_candidates.py"),
    (ROOT / "scoring.py", "scoring.py"),
    (ROOT / "system_prompt.txt", "system_prompt.txt"),
    (ROOT / "system_prompt_hybrid.txt", "system_prompt_hybrid.txt"),
    (ROOT / "pyproject.toml", "pyproject.toml"),
    (ROOT / "uv.lock", "uv.lock"),
    (ROOT / "context_data" / "mixed" / "train.jsonl", "dataset/train.jsonl"),
)


def stage_environment(output: Path = DEFAULT_OUTPUT) -> Path:
    output = output.resolve()
    cache_root = (ROOT / ".data-cache").resolve()
    if output.exists():
        if cache_root not in output.parents:
            raise ValueError("existing output must be inside training/flash/.data-cache")
        shutil.rmtree(output)
    output.mkdir(parents=True)
    for source, destination_name in SOURCE_FILES:
        if not source.is_file():
            raise FileNotFoundError(f"required environment source missing: {source}")
        destination = output / destination_name
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    return output


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    print(stage_environment(args.output))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
