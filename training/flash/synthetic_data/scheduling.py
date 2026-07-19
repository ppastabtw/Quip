"""Deterministic balance and contrast-group scheduling."""

from __future__ import annotations

import math
import random
from dataclasses import asdict, dataclass
from typing import Iterable, Mapping, Sequence

from .config import SyntheticConfig


CONTRAST_VARIANTS = (
    ("context_a", "useful"),
    ("context_b", "useful"),
    ("no_context", "none"),
    ("irrelevant", "irrelevant"),
    ("ambiguous", "ambiguous"),
    ("conflicting", "conflicting"),
)

FORCED_CATEGORY_BEHAVIORS = {
    "irrelevant_misleading_context": "irrelevant",
    "explicit_draft_override": "conflicting",
    "ambiguous_context": "ambiguous",
    "stale_erroneous_context": "conflicting",
    "already_correct": "irrelevant",
}

GROUP_CATEGORIES = (
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
    "tone_preservation",
)


@dataclass(frozen=True)
class Slot:
    slot_id: str
    category: str
    context_behavior: str
    group_id: str | None
    variant: str | None

    def to_dict(self) -> dict[str, str | None]:
        return asdict(self)


def allocate_counts(total: int, weights: Mapping[str, float]) -> dict[str, int]:
    """Use largest remainders so integer allocations are exact and deterministic."""
    raw = {key: total * value for key, value in weights.items()}
    result = {key: math.floor(value) for key, value in raw.items()}
    remaining = total - sum(result.values())
    order = sorted(weights, key=lambda key: (-(raw[key] - result[key]), key))
    for key in order[:remaining]:
        result[key] += 1
    return result


def _expanded_counts(counts: Mapping[str, int]) -> list[str]:
    return [name for name in sorted(counts) for _ in range(counts[name])]


def make_slots(config: SyntheticConfig, *, round_number: int = 1) -> list[Slot]:
    target = math.ceil(config.run.target_count * config.run.oversample_factor)
    behavior_counts = allocate_counts(target, config.behaviors)
    category_counts = allocate_counts(target, config.categories)
    rng = random.Random(config.run.seed + round_number * 1_000_003)

    desired_group_rows = math.floor(target * config.run.contrast_share / len(CONTRAST_VARIANTS)) * len(CONTRAST_VARIANTS)
    max_groups = min(
        behavior_counts["useful"] // 2,
        *(behavior_counts[name] for name in ("none", "irrelevant", "ambiguous", "conflicting")),
    )
    max_groups = min(
        max_groups,
        sum(
            category_counts[name] // len(CONTRAST_VARIANTS)
            for name in GROUP_CATEGORIES
        ),
    )
    group_count = min(desired_group_rows // len(CONTRAST_VARIANTS), max_groups)

    blocks: list[list[Slot]] = []
    slot_number = 0
    for group_number in range(group_count):
        group_id = f"r{round_number:02d}_g{group_number:06d}"
        available_group_categories = [
            name for name in GROUP_CATEGORIES if category_counts[name] >= len(CONTRAST_VARIANTS)
        ]
        if not available_group_categories:
            raise ValueError("category weights cannot support the configured contrast share")
        preferred = [
            name
            for name in ("same_draft_different_context", "context_no_context_pair")
            if name in available_group_categories
        ]
        group_category = (
            preferred[group_number]
            if group_number < len(preferred)
            else max(available_group_categories, key=lambda name: (category_counts[name], rng.random()))
        )
        group_slots: list[Slot] = []
        for variant, behavior in CONTRAST_VARIANTS:
            slot_number += 1
            group_slots.append(
                Slot(
                    slot_id=f"r{round_number:02d}_s{slot_number:08d}",
                    category=group_category,
                    context_behavior=behavior,
                    group_id=group_id,
                    variant=variant,
                )
            )
            behavior_counts[behavior] -= 1
            category_counts[group_category] -= 1
        blocks.append(group_slots)

    assignments: list[tuple[str, str]] = []
    for category, behavior in FORCED_CATEGORY_BEHAVIORS.items():
        count = category_counts[category]
        if count > behavior_counts[behavior]:
            raise ValueError(
                f"category weight for {category} exceeds compatible {behavior} behavior capacity"
            )
        assignments.extend((category, behavior) for _ in range(count))
        category_counts[category] = 0
        behavior_counts[behavior] -= count

    remaining_categories = _expanded_counts(category_counts)
    remaining_behaviors = _expanded_counts(behavior_counts)
    if len(remaining_categories) != len(remaining_behaviors):
        raise AssertionError("category/behavior schedulers disagree on remaining rows")
    rng.shuffle(remaining_categories)
    rng.shuffle(remaining_behaviors)
    assignments.extend(zip(remaining_categories, remaining_behaviors))
    rng.shuffle(assignments)
    for category, behavior in assignments:
        slot_number += 1
        blocks.append(
            [Slot(
                slot_id=f"r{round_number:02d}_s{slot_number:08d}",
                category=category,
                context_behavior=behavior,
                group_id=None,
                variant=None,
            )]
        )
    rng.shuffle(blocks)
    return [slot for block in blocks for slot in block]


def batches(values: Sequence[Slot], size: int) -> Iterable[tuple[Slot, ...]]:
    current: list[Slot] = []
    index = 0
    while index < len(values):
        first = values[index]
        block = [first]
        index += 1
        if first.group_id is not None:
            while index < len(values) and values[index].group_id == first.group_id:
                block.append(values[index])
                index += 1
        if current and len(current) + len(block) > size:
            yield tuple(current)
            current = []
        current.extend(block)
    if current:
        yield tuple(current)


def take_slots(values: Sequence[Slot], count: int) -> list[Slot]:
    """Take an exact cap while preferring complete contiguous contrast groups."""
    if count < 0:
        raise ValueError("slot count cannot be negative")
    blocks: list[list[Slot]] = []
    index = 0
    while index < len(values):
        first = values[index]
        block = [first]
        index += 1
        if first.group_id is not None:
            while index < len(values) and values[index].group_id == first.group_id:
                block.append(values[index])
                index += 1
        blocks.append(block)
    selected: list[Slot] = []
    deferred: list[list[Slot]] = []
    remaining = count
    for block in blocks:
        if len(block) <= remaining:
            selected.extend(block)
            remaining -= len(block)
        else:
            deferred.append(block)
        if remaining == 0:
            return selected
    if remaining:
        for block in deferred:
            take = min(remaining, len(block))
            selected.extend(block[:take])
            remaining -= take
            if remaining == 0:
                return selected
    if remaining:
        raise ValueError(f"requested {count} slots but only {count - remaining} are available")
    return selected
