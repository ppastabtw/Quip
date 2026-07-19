"""Freesolo environment for Quip suggestion training."""

from __future__ import annotations

from pathlib import Path

from freesolo.datasets import TaskExample
from freesolo.datasets.records import load_task_examples
from freesolo.environments import EnvironmentSingleTurn, RewardResult

from model_input import model_input_json
from scoring import score_completion


ROOT = Path(__file__).parent

SYSTEM_PROMPT = (ROOT / "system_prompt.txt").read_text(encoding="utf-8")
HYBRID_SYSTEM_PROMPT = (ROOT / "system_prompt_hybrid.txt").read_text(encoding="utf-8")


class QuipEnvironment(EnvironmentSingleTurn):
    def __init__(
        self,
        *,
        split: str = "train",
        dataset_path: str | None = None,
        lexical_hints: bool = False,
    ) -> None:
        path = Path(dataset_path) if dataset_path else ROOT / "dataset" / f"{split}.jsonl"
        if not path.is_file():
            raise FileNotFoundError(f"Quip dataset split not found: {path}")
        self.dataset = load_task_examples(path)
        self.lexical_hints = lexical_hints
        self.system_prompt = HYBRID_SYSTEM_PROMPT if lexical_hints else SYSTEM_PROMPT

    def build_prompt_messages(self, example: TaskExample, prompt_text: str):
        return [
            {"role": "system", "content": self.system_prompt},
            {
                "role": "user",
                "content": model_input_json(
                    example.input, lexical_hints=self.lexical_hints
                ),
            },
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
    lexical_hints: bool = False,
    **kwargs,
) -> QuipEnvironment:
    return QuipEnvironment(
        split=split,
        dataset_path=dataset_path,
        lexical_hints=lexical_hints,
    )
