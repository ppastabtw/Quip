"""Exact, normalized, and lightweight shape-aware near deduplication."""

from __future__ import annotations

import hashlib
import re
from difflib import SequenceMatcher

from .models import Candidate, compact_json
from .validation import normalized_text


NUMBER_RE = re.compile(r"\b(?:\d+[A-Za-z-]*|[A-Z]\d+)\b")
ENTITY_RE = re.compile(r"\b(?:[A-Z][\w'’-]*)(?:\s+[A-Z][\w'’-]*)*\b")


def normalized_key(candidate: Candidate) -> str:
    return compact_json(
        {
            "input": {
                "text": normalized_text(candidate.text),
                "context_snippets": [
                    {
                        "app_name": normalized_text(item.app_name),
                        "window_title": normalized_text(item.window_title),
                        "visible_text": normalized_text(item.visible_text),
                    }
                    for item in candidate.context_snippets
                ],
            },
            "suggestion": normalized_text(candidate.suggestion),
        }
    )


def shape_text(candidate: Candidate) -> str:
    joined = " | ".join(
        [candidate.text, candidate.suggestion]
        + [f"{item.app_name} {item.window_title} {item.visible_text}" for item in candidate.context_snippets]
    )
    joined = NUMBER_RE.sub("<number>", joined)
    joined = ENTITY_RE.sub("<entity>", joined)
    return normalized_text(joined)


def _simhash(value: str) -> int:
    tokens = re.findall(r"[\w<>']+", value)
    features = tokens + [" ".join(tokens[index : index + 2]) for index in range(len(tokens) - 1)]
    vector = [0] * 64
    for feature in features:
        digest = int.from_bytes(hashlib.blake2b(feature.encode("utf-8"), digest_size=8).digest(), "big")
        for bit in range(64):
            vector[bit] += 1 if digest & (1 << bit) else -1
    return sum((1 << bit) for bit, score in enumerate(vector) if score >= 0)


class Deduplicator:
    def __init__(self, *, near_threshold: float) -> None:
        self.near_threshold = near_threshold
        self.exact: set[str] = set()
        self.normalized: set[str] = set()
        self.rows: list[tuple[Candidate, str, int, bool]] = []
        self.bands: dict[tuple[int, int], set[int]] = {}
        self.reference_exact: set[str] = set()
        self.reference_normalized: set[str] = set()

    def seed_reference(self, candidate: Candidate) -> None:
        exact = compact_json({"input": candidate.input_dict(), "suggestion": candidate.suggestion})
        normalized = normalized_key(candidate)
        if exact in self.exact or normalized in self.normalized:
            return
        self._register(candidate, exact=exact, normalized=normalized, reference=True)
        self.reference_exact.add(exact)
        self.reference_normalized.add(normalized)

    def rejection_reason(self, candidate: Candidate) -> str | None:
        exact = compact_json({"input": candidate.input_dict(), "suggestion": candidate.suggestion})
        if exact in self.exact:
            return "reference_exact_duplicate" if exact in self.reference_exact else "exact_duplicate"
        normalized = normalized_key(candidate)
        if normalized in self.normalized:
            return (
                "reference_normalized_duplicate"
                if normalized in self.reference_normalized
                else "normalized_duplicate"
            )
        shape = shape_text(candidate)
        simhash = _simhash(shape)
        possible: set[int] = set()
        for band in range(4):
            possible.update(self.bands.get((band, (simhash >> (band * 16)) & 0xFFFF), ()))
        for index in possible:
            other, other_shape, _, reference = self.rows[index]
            if candidate.group_id is not None and candidate.group_id == other.group_id:
                continue
            if SequenceMatcher(None, shape, other_shape).ratio() >= self.near_threshold:
                return "reference_near_duplicate" if reference else "near_duplicate"
        self._register(candidate, exact=exact, normalized=normalized, reference=False)
        return None

    def _register(
        self,
        candidate: Candidate,
        *,
        exact: str,
        normalized: str,
        reference: bool,
    ) -> None:
        shape = shape_text(candidate)
        simhash = _simhash(shape)
        index = len(self.rows)
        self.rows.append((candidate, shape, simhash, reference))
        self.exact.add(exact)
        self.normalized.add(normalized)
        for band in range(4):
            self.bands.setdefault((band, (simhash >> (band * 16)) & 0xFFFF), set()).add(index)
