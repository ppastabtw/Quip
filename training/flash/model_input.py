"""Shared model-facing input serialization for training and evaluation."""

from __future__ import annotations

import json

from scoring import model_input_payload


def model_input_json(
    example_input: str | dict, *, lexical_hints: bool = False
) -> str:
    raw_input = json.loads(example_input) if isinstance(example_input, str) else example_input
    if not isinstance(raw_input, dict) or not isinstance(raw_input.get("text"), str):
        raise ValueError("example input must contain string field text")

    model_input = {"text": raw_input["text"]}
    context_snippets = raw_input.get("context_snippets")
    if context_snippets:
        model_input = {
            "context_snippets": context_snippets,
            "text": raw_input["text"],
        }
    if lexical_hints:
        from lexical_candidates import default_generator, enrich_model_input

        model_input = enrich_model_input(model_input, default_generator())
    return json.dumps(
        model_input_payload(model_input), ensure_ascii=False, separators=(",", ":")
    )
