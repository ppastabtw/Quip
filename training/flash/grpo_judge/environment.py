"""Judge-backed Freesolo environment for warm-started Quip GRPO."""

from __future__ import annotations

from pathlib import Path
from typing import Any, Mapping

from freesolo.datasets import TaskExample
from freesolo.datasets.records import load_task_examples
from freesolo.environments import EnvironmentSingleTurn, RewardMetric, RewardResult

from model_input import model_input_json
from scoring import parse_gold_output, score_completion

try:
    from .judge_reward import judge_correction
except ImportError:
    from judge_reward import judge_correction


ROOT = Path(__file__).parent
SOURCE_ROOT = ROOT if (ROOT / "system_prompt.txt").is_file() else ROOT.parent
SYSTEM_PROMPT = (SOURCE_ROOT / "system_prompt.txt").read_text(encoding="utf-8")
HYBRID_SYSTEM_PROMPT = (SOURCE_ROOT / "system_prompt_hybrid.txt").read_text(
    encoding="utf-8"
)


def _default_dataset_path(split: str) -> Path:
    packaged = ROOT / "dataset" / f"{split}.jsonl"
    if packaged.is_file():
        return packaged
    return ROOT.parent / "context_data" / "mixed" / f"{split}.jsonl"


def _accepted_suggestions(example: TaskExample) -> tuple[str, ...]:
    metadata = example.metadata if isinstance(example.metadata, Mapping) else {}
    declared = metadata.get("accepted_suggestions")
    if (
        isinstance(declared, list)
        and declared
        and all(isinstance(value, str) and value.strip() for value in declared)
    ):
        return tuple(declared)
    return (parse_gold_output(example.output).suggestion,)


class QuipJudgeEnvironment(EnvironmentSingleTurn):
    max_score_concurrency = 8

    def __init__(
        self,
        *,
        split: str = "train",
        dataset_path: str | None = None,
        judge_model: str = "Qwen/Qwen3.6-35B-A3B",
        lexical_hints: bool = False,
    ) -> None:
        path = Path(dataset_path) if dataset_path else _default_dataset_path(split)
        if not path.is_file():
            raise FileNotFoundError(f"Quip dataset split not found: {path}")
        self.dataset_path = path
        self.dataset = load_task_examples(path)
        self.judge_model = judge_model
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
        deterministic = score_completion(
            input_text=example.input,
            expected_output=example.output,
            metadata=example.metadata,
            response_text=str(response_text),
        )
        base_metrics = (
            RewardMetric(name="exact_success", score=float(deterministic.success)),
            RewardMetric(
                name="correct_change_decision",
                score=float(deterministic.change_correct),
            ),
        )
        if deterministic.success:
            return RewardResult(
                score=1.0,
                threshold=0.9,
                success=True,
                reason="accepted suggestion",
                metrics=base_metrics
                + (RewardMetric(name="judge_used", score=0.0),),
            )
        if not deterministic.schema_valid or not deterministic.change_correct:
            return RewardResult(
                score=deterministic.score,
                threshold=0.9,
                success=False,
                reason=deterministic.reason,
                metrics=base_metrics
                + (RewardMetric(name="judge_used", score=0.0),),
            )

        try:
            verdict = judge_correction(
                input_value=example.input,
                candidate=deterministic.prediction.suggestion,
                accepted_suggestions=_accepted_suggestions(example),
                model=self.judge_model,
            )
        except Exception as error:
            return RewardResult(
                score=deterministic.score,
                threshold=0.9,
                success=False,
                reason="judge unavailable",
                error=f"{type(error).__name__}: {error}",
                metrics=base_metrics
                + (RewardMetric(name="judge_used", score=1.0),),
            )

        score = round(min(0.99, 0.40 + 0.59 * verdict.score), 6)
        success = verdict.acceptable and score >= 0.9
        return RewardResult(
            score=score,
            threshold=0.9,
            success=success,
            reason=verdict.reason or "judge-scored alternative",
            metrics=base_metrics
            + (
                RewardMetric(name="judge_used", score=1.0),
                RewardMetric(name="judge_score", score=verdict.score),
                RewardMetric(name="judge_acceptable", score=float(verdict.acceptable)),
            ),
        )


def load_environment(
    split: str = "train",
    dataset_path: str | None = None,
    judge_model: str = "Qwen/Qwen3.6-35B-A3B",
    lexical_hints: bool = False,
    **kwargs: Any,
) -> QuipJudgeEnvironment:
    return QuipJudgeEnvironment(
        split=split,
        dataset_path=dataset_path,
        judge_model=judge_model,
        lexical_hints=lexical_hints,
    )
