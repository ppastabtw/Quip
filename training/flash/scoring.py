"""Pure parsing and reward logic shared by training and offline evaluation."""

from __future__ import annotations

import json
import re
import unicodedata
from dataclasses import dataclass
from difflib import SequenceMatcher
from typing import Any, Mapping


ALLOWED_ACTIONS = {"keep", "replace"}
OUTPUT_KEYS = {"action", "candidates"}
OUTPUT_JSON_SCHEMA = {
    "type": "object",
    "properties": {
        "action": {"type": "string", "enum": ["keep", "replace"]},
        "candidates": {"type": "array", "items": {"type": "string"}, "maxItems": 3},
    },
    "required": ["action", "candidates"],
    "additionalProperties": False,
}


@dataclass(frozen=True)
class Prediction:
    action: str
    candidates: tuple[str, ...]


@dataclass(frozen=True)
class ScoreResult:
    score: float
    schema_valid: bool
    action_correct: bool
    content_correct: bool
    protected_preserved: bool
    prediction: Prediction | None
    reason: str

    @property
    def success(self) -> bool:
        return self.schema_valid and self.action_correct and self.content_correct


def _normalize(value: str) -> str:
    value = unicodedata.normalize("NFKC", value)
    return re.sub(r"\s+", " ", value).strip().casefold()


def _input_payload(input_text: str) -> dict[str, Any]:
    payload = json.loads(input_text)
    if not isinstance(payload, dict) or not isinstance(payload.get("text"), str):
        raise ValueError("input must be a JSON object with string field text")
    return payload


def parse_prediction(response_text: str) -> Prediction:
    raw = response_text.strip()
    payload = json.loads(raw)
    if not isinstance(payload, dict) or set(payload) != OUTPUT_KEYS:
        raise ValueError("output must contain exactly action and candidates")

    action = payload["action"]
    candidates = payload["candidates"]
    if action not in ALLOWED_ACTIONS:
        raise ValueError("action must be keep or replace")
    if not isinstance(candidates, list) or not all(isinstance(item, str) for item in candidates):
        raise ValueError("candidates must be a string array")
    if any(not candidate.strip() for candidate in candidates):
        raise ValueError("candidates cannot be empty strings")
    if len({_normalize(candidate) for candidate in candidates}) != len(candidates):
        raise ValueError("candidates must be unique")
    if action == "keep" and candidates:
        raise ValueError("keep must have zero candidates")
    if action == "replace" and not 1 <= len(candidates) <= 3:
        raise ValueError("replace must have one to three candidates")
    return Prediction(action=action, candidates=tuple(candidates))


def _expected_action(expected_output: object, metadata: Mapping[str, Any]) -> str:
    declared = metadata.get("expected_action")
    if declared in ALLOWED_ACTIONS:
        return str(declared)
    if isinstance(expected_output, str):
        return parse_prediction(expected_output).action
    raise ValueError("expected action is missing")


def _accepted_candidates(expected_output: object, metadata: Mapping[str, Any]) -> tuple[str, ...]:
    declared = metadata.get("accepted_candidates")
    if isinstance(declared, list) and all(isinstance(item, str) for item in declared):
        return tuple(declared)
    if isinstance(expected_output, str):
        return parse_prediction(expected_output).candidates
    return ()


def _preserves_tokens(prediction: Prediction, protected_tokens: object) -> bool:
    if not isinstance(protected_tokens, list) or not protected_tokens:
        return True
    if prediction.action == "keep":
        return True
    return all(
        isinstance(token, str) and all(token in candidate for candidate in prediction.candidates)
        for token in protected_tokens
    )


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
        expected_action = _expected_action(expected_output, metadata)
    except (json.JSONDecodeError, TypeError, ValueError) as exc:
        return ScoreResult(0.0, False, False, False, False, None, f"invalid schema: {exc}")

    action_correct = prediction.action == expected_action
    protected_preserved = _preserves_tokens(prediction, metadata.get("protected_tokens"))
    score = 0.15

    if not action_correct:
        return ScoreResult(
            score,
            True,
            False,
            False,
            protected_preserved,
            prediction,
            "wrong action",
        )

    score += 0.25
    if expected_action == "keep":
        return ScoreResult(1.0, True, True, True, True, prediction, "correct keep")

    accepted = _accepted_candidates(expected_output, metadata)
    accepted_normalized = {_normalize(candidate) for candidate in accepted}
    candidate_normalized = [_normalize(candidate) for candidate in prediction.candidates]
    content_correct = any(candidate in accepted_normalized for candidate in candidate_normalized)

    if content_correct:
        score += 0.45
        if candidate_normalized[0] in accepted_normalized:
            score += 0.10
    elif accepted:
        best_similarity = max(
            SequenceMatcher(None, candidate, gold).ratio()
            for candidate in candidate_normalized
            for gold in accepted_normalized
        )
        score += 0.25 * best_similarity

    if protected_preserved:
        score += 0.05

    score = round(min(score, 1.0), 6)
    reason = "accepted replacement" if content_correct else "replacement not accepted"
    if not protected_preserved:
        reason = f"{reason}; protected token changed"
    return ScoreResult(
        score,
        True,
        True,
        content_correct,
        protected_preserved,
        prediction,
        reason,
    )
