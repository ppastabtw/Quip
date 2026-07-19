"""Deterministic validation before judging and before final publication."""

from __future__ import annotations

import re
import unicodedata
from collections import defaultdict
from typing import Sequence

from better_profanity import profanity

from .config import SyntheticConfig
from .models import CATEGORIES, CONTEXT_BEHAVIORS, ERROR_TYPES, WRITING_STYLES, Candidate, Judgment
from .scheduling import Slot


EMAIL_RE = re.compile(r"\b[^\s@]+@[^\s@]+\.[^\s@]+\b")
PHONE_RE = re.compile(r"(?<!\w)(?:\+?\d[\s().-]*){7,}(?!\w)")
URL_RE = re.compile(r"(?:https?://|www\.)", re.I)
CONTROL_RE = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]")


def normalized_text(value: str) -> str:
    return re.sub(r"\s+", " ", unicodedata.normalize("NFKC", value)).strip().casefold()


def _text_reasons(value: str, label: str, *, allow_newlines: bool) -> list[str]:
    reasons: list[str] = []
    try:
        value.encode("utf-8", errors="strict")
    except UnicodeEncodeError:
        reasons.append(f"{label}_malformed_unicode")
    if CONTROL_RE.search(value) or (not allow_newlines and ("\n" in value or "\r" in value)):
        reasons.append(f"{label}_control_or_multiline")
    if profanity.contains_profanity(value):
        reasons.append(f"{label}_profanity")
    if EMAIL_RE.search(value) or PHONE_RE.search(value) or URL_RE.search(value):
        reasons.append(f"{label}_sensitive_or_link_like")
    return reasons


def validate_candidate(
    candidate: Candidate,
    config: SyntheticConfig,
    *,
    expected_slot: Slot | None = None,
) -> list[str]:
    reasons: list[str] = []
    if expected_slot is not None:
        for field in ("slot_id", "category", "context_behavior", "group_id", "variant"):
            if getattr(candidate, field) != getattr(expected_slot, field):
                reasons.append(f"slot_{field}_mismatch")
    if candidate.category not in CATEGORIES:
        reasons.append("unknown_category")
    if candidate.context_behavior not in CONTEXT_BEHAVIORS:
        reasons.append("unknown_context_behavior")
    if candidate.error_type not in ERROR_TYPES:
        reasons.append("unknown_error_type")
    if candidate.writing_style not in WRITING_STYLES:
        reasons.append("unknown_writing_style")
    word_count = len(candidate.text.split())
    if not config.run.min_words <= word_count <= config.run.max_words:
        reasons.append("draft_word_count")
    if len(candidate.text) > config.run.max_draft_chars:
        reasons.append("draft_too_long")
    if len(candidate.context_snippets) > config.run.max_context_snippets:
        reasons.append("too_many_context_snippets")
    if candidate.context_behavior == "none" and candidate.context_snippets:
        reasons.append("no_context_behavior_has_context")
    if candidate.context_behavior != "none" and not candidate.context_snippets:
        reasons.append("context_behavior_missing_context")
    if candidate.category == "already_correct" and candidate.suggestion != candidate.text:
        reasons.append("already_correct_target_changed")
    if candidate.group_id is None and candidate.variant is not None:
        reasons.append("ungrouped_candidate_has_variant")
    if candidate.group_id is not None and candidate.variant is None:
        reasons.append("grouped_candidate_missing_variant")
    reasons.extend(_text_reasons(candidate.text, "draft", allow_newlines=False))
    reasons.extend(_text_reasons(candidate.suggestion, "suggestion", allow_newlines=False))
    for snippet in candidate.context_snippets:
        if len(snippet.visible_text) > config.run.max_context_chars:
            reasons.append("context_too_long")
        for field, value in (
            ("app", snippet.app_name),
            ("window", snippet.window_title),
            ("context", snippet.visible_text),
        ):
            reasons.extend(_text_reasons(value, field, allow_newlines=True))
    return sorted(set(reasons))


def contrast_group_rejections(candidates: Sequence[Candidate]) -> dict[str, list[str]]:
    groups: dict[str, list[Candidate]] = defaultdict(list)
    for candidate in candidates:
        if candidate.group_id is not None:
            groups[candidate.group_id].append(candidate)
    result: dict[str, list[str]] = {}
    expected = {"context_a", "context_b", "no_context", "irrelevant", "ambiguous", "conflicting"}
    for rows in groups.values():
        reasons: list[str] = []
        variants = {row.variant: row for row in rows}
        if set(variants) != expected or len(rows) != len(expected):
            reasons.append("contrast_group_incomplete_or_duplicate")
        else:
            base_texts = {variants[name].text for name in expected - {"conflicting"}}
            if len(base_texts) != 1:
                reasons.append("contrast_group_base_draft_mismatch")
            if variants["context_a"].suggestion == variants["context_b"].suggestion:
                reasons.append("contrast_useful_targets_not_distinct")
            control_targets = {
                variants[name].suggestion for name in ("no_context", "irrelevant", "ambiguous")
            }
            if len(control_targets) != 1:
                reasons.append("contrast_restraint_targets_mismatch")
            if variants["no_context"].context_snippets:
                reasons.append("contrast_no_context_has_context")
        if reasons:
            for row in rows:
                result[row.candidate_id] = reasons
    return result


def judgment_rejection_reasons(judgment: Judgment, config: SyntheticConfig) -> list[str]:
    reasons = list(judgment.failure_reasons)
    if config.thresholds.require_pass_flag and not judgment.passed:
        reasons.append("judge_pass_false")
    for name, score in judgment.scores.items():
        threshold = (
            config.thresholds.minimum_dataset_value
            if name == "dataset_value"
            else config.thresholds.minimum_score
        )
        if score < threshold:
            reasons.append(f"judge_{name}_below_threshold")
    return sorted(set(reasons))
