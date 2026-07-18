"""Pure parsing and reward logic shared by training and offline evaluation."""

from __future__ import annotations

import json
import re
import unicodedata
from dataclasses import dataclass
from difflib import SequenceMatcher
from typing import Any, Mapping


OUTPUT_KEYS = {"suggestion"}


@dataclass(frozen=True)
class Prediction:
    suggestion: str


@dataclass(frozen=True)
class ScoreResult:
    score: float
    schema_valid: bool
    change_correct: bool
    content_correct: bool
    prediction: Prediction | None
    reason: str

    @property
    def success(self) -> bool:
        return (
            self.schema_valid
            and self.change_correct
            and self.content_correct
        )


def _normalize(value: str) -> str:
    value = unicodedata.normalize("NFKC", value)
    return re.sub(r"\s+", " ", value).strip().casefold()


def _input_payload(input_text: str) -> dict[str, Any]:
    payload = json.loads(input_text)
    if (
        not isinstance(payload, dict)
        or set(payload) != {"text"}
        or not isinstance(payload.get("text"), str)
    ):
        raise ValueError("input must contain only string field text")
    return payload


def parse_prediction(response_text: str) -> Prediction:
    """Parse a model reply: the reply is the suggestion text itself."""
    suggestion = response_text.strip()
    if not suggestion:
        raise ValueError("suggestion must be a non-empty string")
    lowered = suggestion.casefold()
    if (
        suggestion[0] in "{}[]"
        or lowered == "suggestion"
        or lowered.startswith("suggestion:")
    ):
        raise ValueError("reply must be plain corrected text")
    return Prediction(suggestion=suggestion)


def parse_gold_output(output_text: str) -> Prediction:
    """Parse a dataset gold output, which stays JSON-encoded on disk."""
    payload = json.loads(output_text.strip())
    if not isinstance(payload, dict) or set(payload) != OUTPUT_KEYS:
        raise ValueError("gold output must contain exactly suggestion")
    suggestion = payload["suggestion"]
    if not isinstance(suggestion, str) or not suggestion.strip():
        raise ValueError("gold suggestion must be a non-empty string")
    return Prediction(suggestion=suggestion)


def _accepted_suggestions(
    expected_output: object, metadata: Mapping[str, Any]
) -> tuple[str, ...]:
    declared = metadata.get("accepted_suggestions")
    if (
        isinstance(declared, list)
        and declared
        and all(isinstance(item, str) and item.strip() for item in declared)
    ):
        return tuple(declared)
    if isinstance(expected_output, str):
        return (parse_gold_output(expected_output).suggestion,)
    raise ValueError("accepted suggestions are missing")


def score_completion(
    *,
    input_text: str,
    expected_output: object,
    metadata: Mapping[str, Any],
    response_text: str,
) -> ScoreResult:
    try:
        prediction = parse_prediction(response_text)
        input_payload = _input_payload(input_text)
        accepted = _accepted_suggestions(expected_output, metadata)
        target_changed = metadata.get("target_changed")
        if not isinstance(target_changed, bool):
            target_changed = accepted[0] != input_payload["text"]
    except (json.JSONDecodeError, TypeError, ValueError) as exc:
        return ScoreResult(0.0, False, False, False, None, f"invalid schema: {exc}")

    predicted_changed = prediction.suggestion != input_payload["text"]
    change_correct = predicted_changed == target_changed
    score = 0.15

    if not change_correct:
        return ScoreResult(
            score,
            True,
            False,
            False,
            prediction,
            "wrong change decision",
        )

    score += 0.25
    accepted_normalized = {_normalize(suggestion) for suggestion in accepted}
    suggestion_normalized = _normalize(prediction.suggestion)
    content_correct = suggestion_normalized in accepted_normalized

    if content_correct:
        score += 0.60
    else:
        best_similarity = max(
            SequenceMatcher(None, suggestion_normalized, gold).ratio()
            for gold in accepted_normalized
        )
        score += 0.25 * best_similarity

    score = round(min(score, 1.0), 6)
    reason = "accepted suggestion" if content_correct else "suggestion not accepted"
    return ScoreResult(
        score,
        True,
        True,
        content_correct,
        prediction,
        reason,
    )
