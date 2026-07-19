"""Freesolo environment for Quip suggestion training."""

from __future__ import annotations

from pathlib import Path

from freesolo.datasets import TaskExample
from freesolo.datasets.records import load_task_examples
from freesolo.environments import EnvironmentSingleTurn, RewardResult

from scoring import score_completion


ROOT = Path(__file__).parent

SYSTEM_PROMPT = (ROOT / "system_prompt.txt").read_text(encoding="utf-8")


class QuipEnvironment(EnvironmentSingleTurn):
    def __init__(self, *, split: str = "train", dataset_path: str | None = None) -> None:
        path = Path(dataset_path) if dataset_path else ROOT / "dataset" / f"{split}.jsonl"
        if not path.is_file():
            raise FileNotFoundError(f"Quip dataset split not found: {path}")
        self.dataset = load_task_examples(path)

    def build_prompt_messages(self, example: TaskExample, prompt_text: str):
        return [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": example.input},
        ]

    def score_response(self, example: TaskExample, response_text: str) -> RewardResult:
        result = score_completion(
            input_text=example.input,
            expected_output=example.output,
            metadata=example.metadata,
            response_text=str(response_text),
        )
        return RewardResult(
            score=result.score,
            threshold=0.9,
            success=result.success,
            reason=result.reason,
        )


def load_environment(
    split: str = "train",
    dataset_path: str | None = None,
    **kwargs,
) -> QuipEnvironment:
    return QuipEnvironment(split=split, dataset_path=dataset_path)
