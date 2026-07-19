"""Strict value objects used at every synthetic-data pipeline boundary."""

from __future__ import annotations

import hashlib
import json
from dataclasses import asdict, dataclass
from typing import Any, Mapping


CATEGORIES = (
    "vague_reference",
    "entity_spelling",
    "phonetic_resolution",
    "abbreviation_acronym",
    "valid_but_wrong_word",
    "omitted_specific",
    "ordered_context",
    "number_date_identifier",
    "canonical_naming",
    "domain_terminology",
    "multiple_clues",
    "multiple_entities",
    "context_fields",
    "same_draft_different_context",
    "context_no_context_pair",
    "irrelevant_misleading_context",
    "explicit_draft_override",
    "ambiguous_context",
    "stale_erroneous_context",
    "already_correct",
    "tone_preservation",
)

CONTEXT_BEHAVIORS = ("useful", "irrelevant", "ambiguous", "conflicting", "none")

WRITING_STYLES = (
    "lowercase_texting",
    "conversational",
    "formal_workplace",
    "terse_fragment",
    "student",
    "voice_dictation",
    "typo_heavy",
    "subtle_single_error",
    "slang",
    "emoji",
    "non_native_english",
)

ERROR_TYPES = (
    "adjacent_key",
    "deletion",
    "insertion",
    "duplication",
    "transposition",
    "missing_space",
    "extra_space",
    "phonetic",
    "autocorrect",
    "real_word_substitution",
    "homophone",
    "capitalization",
    "partial_entity",
    "shorthand",
    "acronym_ambiguity",
    "compressed_wording",
    "number_transposition",
    "omitted_referent",
    "none",
)

SCORE_DIMENSIONS = (
    "correctness",
    "context_grounding",
    "context_usefulness",
    "meaning_preservation",
    "tone_preservation",
    "minimality",
    "naturalness",
    "category_validity",
    "dataset_value",
)


def compact_json(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def stable_id(prefix: str, value: object, length: int = 20) -> str:
    digest = hashlib.sha256(compact_json(value).encode("utf-8")).hexdigest()
    return f"{prefix}{digest[:length]}"


def _required_string(value: object, name: str, *, allow_empty: bool = False) -> str:
    if not isinstance(value, str) or (not allow_empty and not value.strip()):
        raise ValueError(f"{name} must be a {'string' if allow_empty else 'non-empty string'}")
    return value


@dataclass(frozen=True)
class ContextSnippet:
    app_name: str
    window_title: str
    visible_text: str

    @classmethod
    def from_mapping(cls, value: object) -> "ContextSnippet":
        if not isinstance(value, Mapping) or set(value) != {
            "app_name",
            "window_title",
            "visible_text",
        }:
            raise ValueError("context snippet must contain app_name, window_title, visible_text")
        return cls(
            _required_string(value["app_name"], "context app_name"),
            _required_string(value["window_title"], "context window_title", allow_empty=True),
            _required_string(value["visible_text"], "context visible_text", allow_empty=True),
        )

    def to_dict(self) -> dict[str, str]:
        return asdict(self)


@dataclass(frozen=True)
class Candidate:
    slot_id: str
    category: str
    context_behavior: str
    group_id: str | None
    variant: str | None
    domain: str
    error_type: str
    writing_style: str
    text: str
    context_snippets: tuple[ContextSnippet, ...]
    suggestion: str
    rationale: str
    candidate_id: str = ""

    @classmethod
    def from_mapping(cls, value: object, *, run_id: str) -> "Candidate":
        if not isinstance(value, Mapping):
            raise ValueError("candidate must be an object")
        required = {
            "slot_id",
            "category",
            "context_behavior",
            "group_id",
            "variant",
            "domain",
            "error_type",
            "writing_style",
            "input",
            "output",
            "rationale",
        }
        if set(value) != required:
            raise ValueError(
                "candidate keys must be exactly: " + ", ".join(sorted(required))
            )
        input_value = value["input"]
        if not isinstance(input_value, Mapping) or not set(input_value) <= {
            "text",
            "context_snippets",
        } or "text" not in input_value:
            raise ValueError("candidate input must contain text and optional context_snippets")
        context_value = input_value.get("context_snippets", [])
        if not isinstance(context_value, list):
            raise ValueError("context_snippets must be a list")
        output_value = value["output"]
        if not isinstance(output_value, Mapping) or set(output_value) != {"suggestion"}:
            raise ValueError("candidate output must contain exactly suggestion")
        group_id = value["group_id"]
        variant = value["variant"]
        if group_id is not None:
            group_id = _required_string(group_id, "group_id")
        if variant is not None:
            variant = _required_string(variant, "variant")
        base = cls(
            slot_id=_required_string(value["slot_id"], "slot_id"),
            category=_required_string(value["category"], "category"),
            context_behavior=_required_string(value["context_behavior"], "context_behavior"),
            group_id=group_id,
            variant=variant,
            domain=_required_string(value["domain"], "domain"),
            error_type=_required_string(value["error_type"], "error_type"),
            writing_style=_required_string(value["writing_style"], "writing_style"),
            text=_required_string(input_value["text"], "input.text"),
            context_snippets=tuple(ContextSnippet.from_mapping(row) for row in context_value),
            suggestion=_required_string(output_value["suggestion"], "output.suggestion"),
            rationale=_required_string(value["rationale"], "rationale"),
        )
        identity = {
            "run_id": run_id,
            "slot_id": base.slot_id,
            "input": base.input_dict(),
            "output": {"suggestion": base.suggestion},
        }
        return cls(**{**asdict(base), "context_snippets": base.context_snippets, "candidate_id": stable_id("syn_", identity)})

    @classmethod
    def from_record(cls, value: Mapping[str, Any]) -> "Candidate":
        candidate = cls.from_mapping(
            {key: value[key] for key in value if key not in {"candidate_id", "run_id", "generation"}},
            run_id=_required_string(value.get("run_id"), "run_id"),
        )
        declared = value.get("candidate_id")
        if declared != candidate.candidate_id:
            raise ValueError("candidate_id does not match candidate content")
        return candidate

    def input_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {"text": self.text}
        if self.context_snippets:
            result["context_snippets"] = [snippet.to_dict() for snippet in self.context_snippets]
        return result

    def generation_dict(self) -> dict[str, Any]:
        return {
            "slot_id": self.slot_id,
            "category": self.category,
            "context_behavior": self.context_behavior,
            "group_id": self.group_id,
            "variant": self.variant,
            "domain": self.domain,
            "error_type": self.error_type,
            "writing_style": self.writing_style,
            "input": self.input_dict(),
            "output": {"suggestion": self.suggestion},
            "rationale": self.rationale,
        }

    def audit_record(self, *, run_id: str, generation: Mapping[str, Any]) -> dict[str, Any]:
        return {
            "candidate_id": self.candidate_id,
            "run_id": run_id,
            **self.generation_dict(),
            "generation": dict(generation),
        }


@dataclass(frozen=True)
class Judgment:
    candidate_id: str
    passed: bool
    scores: dict[str, int]
    failure_reasons: tuple[str, ...]
    notes: str

    @classmethod
    def from_mapping(cls, value: object) -> "Judgment":
        if not isinstance(value, Mapping) or set(value) != {
            "candidate_id",
            "pass",
            "scores",
            "failure_reasons",
            "notes",
        }:
            raise ValueError("judgment has invalid keys")
        scores = value["scores"]
        if not isinstance(scores, Mapping) or set(scores) != set(SCORE_DIMENSIONS):
            raise ValueError("judgment scores have invalid dimensions")
        parsed_scores: dict[str, int] = {}
        for name in SCORE_DIMENSIONS:
            score = scores[name]
            if isinstance(score, bool) or not isinstance(score, int) or not 1 <= score <= 5:
                raise ValueError(f"judgment score {name} must be an integer from 1 to 5")
            parsed_scores[name] = score
        reasons = value["failure_reasons"]
        if not isinstance(reasons, list) or not all(isinstance(item, str) and item.strip() for item in reasons):
            raise ValueError("failure_reasons must be a list of non-empty strings")
        passed = value["pass"]
        if not isinstance(passed, bool):
            raise ValueError("judgment pass must be boolean")
        return cls(
            candidate_id=_required_string(value["candidate_id"], "candidate_id"),
            passed=passed,
            scores=parsed_scores,
            failure_reasons=tuple(reasons),
            notes=_required_string(value["notes"], "notes", allow_empty=True),
        )

    def to_dict(self) -> dict[str, Any]:
        return {
            "candidate_id": self.candidate_id,
            "pass": self.passed,
            "scores": self.scores,
            "failure_reasons": list(self.failure_reasons),
            "notes": self.notes,
        }
