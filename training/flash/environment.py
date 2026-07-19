"""Freesolo environment for Quip suggestion training."""

from __future__ import annotations

import json
from pathlib import Path

from freesolo.datasets import TaskExample
from freesolo.datasets.records import load_task_examples
from freesolo.environments import EnvironmentSingleTurn, RewardResult

from lexical_candidates import default_generator, enrich_model_input
from scoring import model_input_payload, score_completion


ROOT = Path(__file__).parent

SYSTEM_PROMPT = (ROOT / "system_prompt.txt").read_text(encoding="utf-8")
HYBRID_SYSTEM_PROMPT = (ROOT / "system_prompt_hybrid.txt").read_text(encoding="utf-8")


def model_input_json(
    example_input: str | dict, *, lexical_hints: bool = False
) -> str:
    raw_input = json.loads(example_input) if isinstance(example_input, str) else example_input
    if not isinstance(raw_input, dict) or not isinstance(raw_input.get("text"), str):
        raise ValueError("example input must contain string field text")
    model_input = {}
    if raw_input.get("context_snippets"):
        model_input["context_snippets"] = raw_input["context_snippets"]
    model_input["text"] = raw_input["text"]
    if lexical_hints:
        model_input = enrich_model_input(model_input, default_generator())
    return json.dumps(
        model_input_payload(model_input), ensure_ascii=False, separators=(",", ":")
    )


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
