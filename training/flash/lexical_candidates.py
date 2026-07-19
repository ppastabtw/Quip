"""Deterministic dictionary candidates for Quip's hybrid correction protocol."""

from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from functools import lru_cache
from typing import Any, Iterable

from wordfreq import top_n_list, zipf_frequency

from scoring import model_input_payload


WORD_RE = re.compile(r"[A-Za-z]+(?:'[A-Za-z]+)?")
DICTIONARY_WORD_RE = re.compile(r"[a-z]+(?:'[a-z]+)?")
QWERTY_ROWS = ("qwertyuiop", "asdfghjkl", "zxcvbnm")
PROTECTED_NEIGHBORS = set("_/@\\")


def _keyboard_neighbors() -> dict[str, set[str]]:
    positions = {
        character: (row_index, column_index)
        for row_index, row in enumerate(QWERTY_ROWS)
        for column_index, character in enumerate(row)
    }
    neighbors: dict[str, set[str]] = {character: set() for character in positions}
    for character, (row_index, column_index) in positions.items():
        for other, (other_row, other_column) in positions.items():
            if character != other and abs(row_index - other_row) <= 1 and abs(
                column_index - other_column
            ) <= 1:
                neighbors[character].add(other)
    return neighbors


KEYBOARD_NEIGHBORS = _keyboard_neighbors()


def levenshtein_distance(left: str, right: str) -> int:
    """Return ordinary Levenshtein distance for metric index lookup."""
    if len(left) > len(right):
        left, right = right, left
    previous = list(range(len(left) + 1))
    for right_index, right_character in enumerate(right, 1):
        current = [right_index]
        for left_index, left_character in enumerate(left, 1):
            current.append(
                min(
                    current[-1] + 1,
                    previous[left_index] + 1,
                    previous[left_index - 1]
                    + (left_character != right_character),
                )
            )
        previous = current
    return previous[-1]


def weighted_damerau_levenshtein(left: str, right: str) -> float:
    """Score edits, discounting adjacent keyboard substitutions and swaps."""
    rows = len(left) + 1
    columns = len(right) + 1
    distance = [[0.0] * columns for _ in range(rows)]
    for row in range(rows):
        distance[row][0] = float(row)
    for column in range(columns):
        distance[0][column] = float(column)

    for row in range(1, rows):
        for column in range(1, columns):
            left_character = left[row - 1]
            right_character = right[column - 1]
            if left_character == right_character:
                substitution_cost = 0.0
            elif right_character in KEYBOARD_NEIGHBORS.get(left_character, set()):
                substitution_cost = 0.55
            else:
                substitution_cost = 1.0
            best = min(
                distance[row - 1][column] + 1.0,
                distance[row][column - 1] + 1.0,
                distance[row - 1][column - 1] + substitution_cost,
            )
            if (
                row > 1
                and column > 1
                and left[row - 1] == right[column - 2]
                and left[row - 2] == right[column - 1]
            ):
                best = min(best, distance[row - 2][column - 2] + 0.65)
            distance[row][column] = best
    return distance[-1][-1]


@dataclass
class _BkNode:
    word: str
    children: dict[int, "_BkNode"] = field(default_factory=dict)

    def add(self, word: str) -> None:
        node = self
        while True:
            distance = levenshtein_distance(word, node.word)
            child = node.children.get(distance)
            if child is None:
                node.children[distance] = _BkNode(word)
                return
            node = child

    def query(self, word: str, maximum_distance: int, results: list[str]) -> None:
        distance = levenshtein_distance(word, self.word)
        if distance <= maximum_distance:
            results.append(self.word)
        lower = distance - maximum_distance
        upper = distance + maximum_distance
        for edge, child in self.children.items():
            if lower <= edge <= upper:
                child.query(word, maximum_distance, results)


class LexicalCandidateGenerator:
    """Generate bounded spelling hints without consulting correction targets."""

    def __init__(
        self,
        *,
        vocabulary_size: int = 30_000,
        minimum_zipf: float = 3.0,
        maximum_candidates: int = 3,
        vocabulary: Iterable[str] | None = None,
    ) -> None:
        raw_vocabulary = (
            vocabulary
            if vocabulary is not None
            else top_n_list("en", vocabulary_size, ascii_only=True)
        )
        words = sorted(
            {
                word.casefold()
                for word in raw_vocabulary
                if DICTIONARY_WORD_RE.fullmatch(word.casefold())
                and len(word) >= 3
                and zipf_frequency(word.casefold(), "en") >= minimum_zipf
            }
        )
        if not words:
            raise ValueError("lexical candidate vocabulary is empty")
        self.words = frozenset(words)
        self.minimum_zipf = minimum_zipf
        self.maximum_candidates = maximum_candidates
        self.root = _BkNode(words[0])
        for word in words[1:]:
            self.root.add(word)

    @lru_cache(maxsize=20_000)
    def candidates_for_token(self, token: str) -> tuple[str, ...]:
        normalized = token.casefold()
        if len(normalized) < 3 or normalized in self.words:
            return ()
        lookup_distance = 1 if len(normalized) <= 3 else 2
        matches: list[str] = []
        self.root.query(normalized, lookup_distance, matches)
        if len(normalized) <= 4:
            maximum_weighted_distance = 1.10
        elif len(normalized) <= 6:
            maximum_weighted_distance = 1.65
        else:
            maximum_weighted_distance = 2.0
        ranked = []
        for candidate in matches:
            weighted_distance = weighted_damerau_levenshtein(normalized, candidate)
            if weighted_distance > maximum_weighted_distance:
                continue
            ranked.append(
                (
                    weighted_distance,
                    -zipf_frequency(candidate, "en"),
                    abs(len(candidate) - len(normalized)),
                    candidate,
                )
            )
        ranked.sort()
        return tuple(item[-1] for item in ranked[: self.maximum_candidates])

    def hints_for_text(
        self, text: str, *, protected_words: Iterable[str] = ()
    ) -> list[dict[str, Any]]:
        protected = {word.casefold() for word in protected_words}
        hints: list[dict[str, Any]] = []
        seen: set[str] = set()
        for match in WORD_RE.finditer(text):
            token = match.group()
            normalized = token.casefold()
            if normalized in seen or normalized in protected:
                continue
            if token[0].isupper() and match.start() != 0:
                continue
            if _is_protected_surface(text, match.start(), match.end()):
                continue
            candidates = self.candidates_for_token(token)
            if candidates:
                hints.append({"token": token, "candidates": list(candidates)})
                seen.add(normalized)
        return hints


def _is_protected_surface(text: str, start: int, end: int) -> bool:
    before = text[start - 1] if start else ""
    after = text[end] if end < len(text) else ""
    if before in PROTECTED_NEIGHBORS or after in PROTECTED_NEIGHBORS:
        return True
    if before.isdigit() or after.isdigit():
        return True
    if before == "." and start > 1 and text[start - 2].isalnum():
        return True
    if after == "." and end + 1 < len(text) and text[end + 1].isalnum():
        return True
    return False


def parse_model_input(value: object) -> dict[str, Any]:
    payload = json.loads(value) if isinstance(value, str) else value
    return model_input_payload(payload)


def enrich_model_input(
    value: object,
    generator: LexicalCandidateGenerator,
    *,
    protected_words: Iterable[str] = (),
) -> dict[str, Any]:
    payload = parse_model_input(value)
    payload["lexical_hints"] = generator.hints_for_text(
        payload["text"], protected_words=protected_words
    )
    return payload


@lru_cache(maxsize=1)
def default_generator() -> LexicalCandidateGenerator:
    return LexicalCandidateGenerator()
