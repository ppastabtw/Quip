"""Pure parsing and reward logic shared by training and offline evaluation."""

from __future__ import annotations

import json
import re
import unicodedata
from dataclasses import dataclass
from difflib import SequenceMatcher
from typing import Any, Mapping, Sequence

from freesolo.utils.core import serialize_value


OUTPUT_KEYS = {"suggestion"}
OUTPUT_JSON_SCHEMA = {
    "type": "object",
    "properties": {"suggestion": {"type": "string", "minLength": 1}},
    "required": ["suggestion"],
    "additionalProperties": False,
}
OUTPUT_RESPONSE_FORMAT = {
    "type": "json_schema",
    "json_schema": {
        "name": "quip_prediction",
        "schema": OUTPUT_JSON_SCHEMA,
    },
}


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


def model_text(value: object) -> str:
    """Render a dataset value exactly as Freesolo presents it to the model."""
    return serialize_value(value)


def model_input_payload(input_value: object) -> dict[str, Any]:
    """Validate the model-facing optional-field input contract."""
    payload = json.loads(input_value) if isinstance(input_value, str) else input_value
    if not isinstance(payload, dict) or not isinstance(payload.get("text"), str):
        raise ValueError("input must contain string field text")
    allowed_fields = {"text", "context_snippets", "lexical_hints"}
    unknown_fields = set(payload).difference(allowed_fields)
    if unknown_fields:
        raise ValueError("input contains unsupported model fields")

    if "context_snippets" in payload:
        context_snippets = payload["context_snippets"]
        if context_snippets is None or context_snippets == []:
            payload = dict(payload)
            del payload["context_snippets"]
        elif not isinstance(context_snippets, list) or len(context_snippets) != 1:
            raise ValueError("context_snippets must contain exactly one snippet")
        else:
            snippet = context_snippets[0]
            if (
                not isinstance(snippet, dict)
                or set(snippet) != {"app_name", "window_title", "visible_text"}
                or not all(
                    isinstance(snippet.get(key), str) and snippet[key]
                    for key in ("app_name", "window_title", "visible_text")
                )
            ):
                raise ValueError("context snippet fields are invalid")

    if "lexical_hints" in payload:
        lexical_hints = payload["lexical_hints"]
        if not isinstance(lexical_hints, list):
            raise ValueError("lexical_hints must be a list")
        for hint in lexical_hints:
            if (
                not isinstance(hint, dict)
                or set(hint) != {"token", "candidates"}
                or not isinstance(hint.get("token"), str)
                or not hint["token"]
                or not isinstance(hint.get("candidates"), list)
                or not hint["candidates"]
                or not all(
                    isinstance(candidate, str) and candidate
                    for candidate in hint["candidates"]
                )
            ):
                raise ValueError("lexical hint fields are invalid")
    return dict(payload)


def _parse_suggestion_payload(value: object, *, label: str) -> Prediction:
    payload = json.loads(value.strip()) if isinstance(value, str) else value
    if not isinstance(payload, dict) or set(payload) != OUTPUT_KEYS:
        raise ValueError(f"{label} must contain exactly suggestion")
    suggestion = payload["suggestion"]
    if not isinstance(suggestion, str) or not suggestion.strip():
        raise ValueError(f"{label} suggestion must be a non-empty string")
    return Prediction(suggestion=suggestion)


def parse_prediction(response_text: str) -> Prediction:
    """Parse one strict JSON model completion."""
    return _parse_suggestion_payload(response_text, label="model output")


def parse_gold_output(output_value: object) -> Prediction:
    """Parse the same strict object from JSONL text or its loaded form."""
    return _parse_suggestion_payload(output_value, label="gold output")


def rank_candidate_items(
    suggestions: Sequence[Mapping[str, Any]], original: str
) -> list[dict[str, Any]]:
    """Rank changed suggestions exactly as the five-completion product surface does."""
    groups: dict[str, list[Mapping[str, Any]]] = {}
    for item in suggestions:
        suggestion = item.get("suggestion")
        if not isinstance(suggestion, str):
            raise ValueError("candidate suggestion must be a string")
        if suggestion == original:
            continue
        groups.setdefault(suggestion, []).append(item)

    ordered_groups = sorted(
        groups.values(),
        key=lambda group: (-len(group), int(group[0].get("index", 0))),
    )
    candidates: list[dict[str, Any]] = []
    for rank, group in enumerate(ordered_groups, start=1):
        candidate = dict(group[0])
        candidate.update(
            {
                "rank": rank,
                "vote_count": len(group),
                "completion_indices": [item.get("index") for item in group],
            }
        )
        candidates.append(candidate)
    return candidates


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
    input_text: object,
    expected_output: object,
    metadata: Mapping[str, Any],
    response_text: str,
) -> ScoreResult:
    try:
        prediction = parse_prediction(response_text)
        input_payload = model_input_payload(input_text)
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
