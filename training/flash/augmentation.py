"""Deterministic US QWERTY typing-error augmentation."""

from __future__ import annotations

import math
import random
import re
from collections.abc import Mapping
from typing import Any


OPERATOR_NAMES = (
    "substitution",
    "deletion",
    "insertion",
    "transposition",
    "repeat",
    "spacing",
    "vowel_deletion",
    "phonetic_rewrite",
)
DEFAULT_WEIGHTS: dict[str, float] = {
    "substitution": 59.0,
    "deletion": 16.0,
    "insertion": 10.0,
    "transposition": 2.0,
    "repeat": 8.0,
    "spacing": 5.0,
    "vowel_deletion": 0.0,
    "phonetic_rewrite": 0.0,
}
MAX_EVENTS = 10

_KEYBOARD_ROWS = (
    ("`1234567890-=", 0.0),
    ("qwertyuiop[]\\", 0.25),
    ("asdfghjkl;'", 0.5),
    ("zxcvbnm,./", 1.0),
)
_KEY_POSITIONS = {
    key: (row_index, column + offset)
    for row_index, (row, offset) in enumerate(_KEYBOARD_ROWS)
    for column, key in enumerate(row)
}
_VOWELS = frozenset("aeiouAEIOU")
_PHONETIC_PATTERNS = (
    (re.compile("tion", re.I), "shun"),
    (re.compile("ph", re.I), "f"),
    (re.compile("ck", re.I), "k"),
    (re.compile("qu", re.I), "kw"),
    (re.compile("c(?=[eiy])", re.I), "s"),
    (re.compile("(?<!t)c(?![eiyk])", re.I), "k"),
)


def qwerty_neighbors(key: str) -> dict[str, int]:
    """Return nearby US QWERTY keys with horizontal, vertical, diagonal weights."""
    if len(key) != 1:
        return {}
    lower = key.lower()
    position = _KEY_POSITIONS.get(lower)
    if position is None:
        return {}

    row_index, x_position = position
    neighbors: dict[str, int] = {}
    row = _KEYBOARD_ROWS[row_index][0]
    column = row.index(lower)
    for candidate_column in (column - 1, column + 1):
        if 0 <= candidate_column < len(row):
            neighbors[row[candidate_column]] = 6

    for other_row_index in (row_index - 1, row_index + 1):
        if not 0 <= other_row_index < len(_KEYBOARD_ROWS):
            continue
        other_row, other_offset = _KEYBOARD_ROWS[other_row_index]
        distances = [abs((index + other_offset) - x_position) for index in range(len(other_row))]
        closest = min(distances)
        for index, distance in enumerate(distances):
            if distance > 1.0:
                continue
            weight = 3 if abs(distance - closest) < 1e-9 else 1
            candidate = other_row[index]
            neighbors[candidate] = max(neighbors.get(candidate, 0), weight)

    if key.isupper():
        return {candidate.upper(): weight for candidate, weight in neighbors.items()}
    return neighbors


def normalize_weights(weights: Mapping[str, int | float] | None = None) -> dict[str, float]:
    """Validate an operator profile and normalize it to a probability distribution."""
    supplied = DEFAULT_WEIGHTS if weights is None else weights
    supplied_names = set(supplied)
    operator_names = set(OPERATOR_NAMES)
    missing_required = {
        name for name in operator_names - supplied_names if DEFAULT_WEIGHTS[name] > 0
    }
    if supplied_names - operator_names or missing_required:
        raise ValueError(f"weights must contain exactly: {', '.join(OPERATOR_NAMES)}")

    numeric: dict[str, float] = {}
    for name in OPERATOR_NAMES:
        value = supplied.get(name, DEFAULT_WEIGHTS[name])
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise ValueError(f"weight {name} must be a number")
        number = float(value)
        if not math.isfinite(number):
            raise ValueError(f"weight {name} must be finite")
        if number < 0 or number > 1_000:
            raise ValueError(f"weight {name} must be between 0 and 1000")
        numeric[name] = number
    total = sum(numeric.values())
    if total <= 0:
        raise ValueError("at least one operator weight must be greater than zero")
    return {name: numeric[name] / total for name in OPERATOR_NAMES}


def _weighted_choice(rng: random.Random, weights: Mapping[str, int | float]) -> str:
    total = float(sum(weights.values()))
    target = rng.random() * total
    cumulative = 0.0
    for name, weight in weights.items():
        cumulative += float(weight)
        if target < cumulative:
            return name
    return next(reversed(weights))


def _weighted_neighbor(
    rng: random.Random, key: str, *, letter_neighbors_only: bool
) -> str:
    neighbors = qwerty_neighbors(key)
    if letter_neighbors_only and key.isalpha():
        neighbors = {
            candidate: weight
            for candidate, weight in neighbors.items()
            if candidate.isalpha()
        }
    return _weighted_choice(rng, neighbors)


def _viable_operators(text: str) -> set[str]:
    keyboard_indices = [index for index, char in enumerate(text) if qwerty_neighbors(char)]
    transpose_indices = [index for index in range(len(text) - 1) if text[index] != text[index + 1]]
    spacing_indices = [
        index
        for index, char in enumerate(text)
        if char.isspace()
    ] + [
        index
        for index in range(1, len(text))
        if not text[index - 1].isspace() and not text[index].isspace()
    ]
    viable = set()
    if keyboard_indices:
        viable.update({"substitution", "insertion"})
    if text:
        viable.add("deletion")
    if transpose_indices:
        viable.add("transposition")
    if any(not char.isspace() for char in text):
        viable.add("repeat")
    if spacing_indices:
        viable.add("spacing")
    if _vowel_deletion_indices(text):
        viable.add("vowel_deletion")
    if _phonetic_rewrites(text):
        viable.add("phonetic_rewrite")
    return viable


def _vowel_deletion_indices(text: str) -> list[int]:
    indices: list[int] = []
    word_start = 0
    while word_start < len(text):
        if not text[word_start].isalpha():
            word_start += 1
            continue
        word_end = word_start
        while word_end < len(text) and text[word_end].isalpha():
            word_end += 1
        if word_end - word_start >= 3:
            indices.extend(
                index
                for index in range(word_start, word_end)
                if text[index] in _VOWELS
            )
        word_start = word_end
    return indices


def _phonetic_rewrites(text: str) -> list[tuple[int, int, str]]:
    rewrites: list[tuple[int, int, str]] = []
    for pattern, replacement in _PHONETIC_PATTERNS:
        for match in pattern.finditer(text):
            source = match.group(0)
            rendered = replacement
            if source.isupper():
                rendered = replacement.upper()
            elif source[:1].isupper():
                rendered = replacement.capitalize()
            if rendered != source:
                rewrites.append((match.start(), match.end(), rendered))
    return rewrites


def _apply_operator(
    text: str,
    operator: str,
    rng: random.Random,
    *,
    letter_neighbors_only: bool,
) -> tuple[str, int, str, str]:
    if operator in {"substitution", "insertion"}:
        indices = [index for index, char in enumerate(text) if qwerty_neighbors(char)]
        index = rng.choice(indices)
        neighbor = _weighted_neighbor(
            rng,
            text[index],
            letter_neighbors_only=letter_neighbors_only,
        )
        if operator == "substitution":
            return text[:index] + neighbor + text[index + 1 :], index, text[index], neighbor
        insertion_index = index + rng.randrange(2)
        return text[:insertion_index] + neighbor + text[insertion_index:], insertion_index, "", neighbor

    if operator == "deletion":
        index = rng.randrange(len(text))
        return text[:index] + text[index + 1 :], index, text[index], ""

    if operator == "transposition":
        indices = [index for index in range(len(text) - 1) if text[index] != text[index + 1]]
        index = rng.choice(indices)
        source = text[index : index + 2]
        replacement = source[1] + source[0]
        return text[:index] + replacement + text[index + 2 :], index, source, replacement

    if operator == "repeat":
        indices = [index for index, char in enumerate(text) if not char.isspace()]
        index = rng.choice(indices)
        return text[:index] + text[index] + text[index:], index, "", text[index]

    if operator == "spacing":
        removal_indices = [index for index, char in enumerate(text) if char.isspace()]
        insertion_indices = [
            index
            for index in range(1, len(text))
            if not text[index - 1].isspace() and not text[index].isspace()
        ]
        choices = [(index, "remove") for index in removal_indices]
        choices.extend((index, "insert") for index in insertion_indices)
        index, action = rng.choice(choices)
        if action == "remove":
            return text[:index] + text[index + 1 :], index, text[index], ""
        return text[:index] + " " + text[index:], index, "", " "

    if operator == "vowel_deletion":
        index = rng.choice(_vowel_deletion_indices(text))
        return text[:index] + text[index + 1 :], index, text[index], ""

    if operator == "phonetic_rewrite":
        start, end, replacement = rng.choice(_phonetic_rewrites(text))
        source = text[start:end]
        return text[:start] + replacement + text[end:], start, source, replacement

    raise ValueError(f"unknown operator: {operator}")


def augment_text(
    text: str,
    *,
    seed: int,
    event_count: int = 1,
    weights: Mapping[str, int | float] | None = None,
    letter_neighbors_only: bool = False,
) -> dict[str, Any]:
    """Apply deterministic mutations and return the augmented text plus an audit trace."""
    if not isinstance(text, str):
        raise TypeError("text must be a string")
    if isinstance(seed, bool) or not isinstance(seed, int):
        raise TypeError("seed must be an integer")
    if isinstance(event_count, bool) or not isinstance(event_count, int):
        raise TypeError("event_count must be an integer")
    if not 1 <= event_count <= MAX_EVENTS:
        raise ValueError(f"event_count must be between 1 and {MAX_EVENTS}")

    normalized = normalize_weights(weights)
    rng = random.Random(seed)
    augmented = text
    seen = {text}
    operations: list[dict[str, Any]] = []
    for event in range(1, event_count + 1):
        accepted: tuple[str, str, int, str, str] | None = None
        for _ in range(32):
            viable = _viable_operators(augmented)
            available_weights = {
                name: normalized[name]
                for name in OPERATOR_NAMES
                if name in viable and normalized[name] > 0
            }
            if not available_weights:
                break
            operator = _weighted_choice(rng, available_weights)
            candidate, index, source, replacement = _apply_operator(
                augmented,
                operator,
                rng,
                letter_neighbors_only=letter_neighbors_only,
            )
            if candidate not in seen:
                accepted = (operator, candidate, index, source, replacement)
                break
        if accepted is None:
            break
        operator, candidate, index, source, replacement = accepted
        before = augmented
        augmented = candidate
        seen.add(augmented)
        operations.append(
            {
                "event": event,
                "operator": operator,
                "index": index,
                "source": source,
                "replacement": replacement,
                "before": before,
                "after": augmented,
            }
        )

    return {
        "original": text,
        "augmented": augmented,
        "seed": seed,
        "requested_events": event_count,
        "applied_events": len(operations),
        "weights": normalized,
        "operations": operations,
    }
