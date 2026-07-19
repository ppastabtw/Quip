"""Load versioned prompts and construct compact machine-readable requests."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Sequence

from .config import SyntheticConfig
from .models import Candidate
from .scheduling import Slot


PROMPT_DIR = Path(__file__).resolve().parent / "prompts"
GENERATOR_PROMPT_PATH = PROMPT_DIR / "generator-v1.txt"
JUDGE_PROMPT_PATH = PROMPT_DIR / "judge-v1.txt"
GENERATOR_PROMPT_VERSION = "generator-v1"
JUDGE_PROMPT_VERSION = "judge-v1"


def generator_system_prompt() -> str:
    return GENERATOR_PROMPT_PATH.read_text(encoding="utf-8")


def judge_system_prompt() -> str:
    return JUDGE_PROMPT_PATH.read_text(encoding="utf-8")


def generation_request(
    config: SyntheticConfig,
    slots: Sequence[Slot],
    *,
    seed: int,
    diversity_reference: dict[str, object] | None = None,
) -> str:
    payload = {
        "task": "generate",
        "seed": seed,
        "constraints": {
            "draft_words": [config.run.min_words, config.run.max_words],
            "max_draft_characters": config.run.max_draft_chars,
            "max_context_snippets": config.run.max_context_snippets,
            "max_context_characters_per_snippet": config.run.max_context_chars,
            "preserve_slot_fields_exactly": True,
            "one_candidate_per_slot": True,
        },
        "slots": [slot.to_dict() for slot in slots],
    }
    if diversity_reference:
        payload["diversity_reference"] = diversity_reference
    return json.dumps(payload, ensure_ascii=False, sort_keys=True)


def judge_request(candidates: Sequence[Candidate]) -> str:
    payload = {
        "task": "judge",
        "candidates": [
            {
                "candidate_id": candidate.candidate_id,
                **candidate.generation_dict(),
            }
            for candidate in candidates
        ],
    }
    return json.dumps(payload, ensure_ascii=False, sort_keys=True)
