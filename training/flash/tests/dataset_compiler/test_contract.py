from collections import Counter
from pathlib import Path

import pytest

from dataset_compiler import contract
from tests.dataset_compiler.helpers import candidate


def test_filters_pii_markup_code_and_word_boundaries():
    assert contract.text_rejection_reason("call me") is None
    assert contract.word_count("one two three four five") == 5
    assert contract.text_rejection_reason("one two three four five six") == "word_count"
    assert contract.text_rejection_reason("me@example.com") == "email"
    assert contract.text_rejection_reason("call 416 555 0199") == "phone"
    assert contract.text_rejection_reason("@person hello") == "handle"
    assert contract.text_rejection_reason("&lt;#&gt;") == "markup"
    assert contract.text_rejection_reason("def run()") == "code"


def test_teacher_targets_reject_existing_shorthand_and_noise():
    assert contract.teacher_target_rejection_reason("Please call me.") is None
    assert contract.teacher_target_rejection_reason("Pls call me") == "teacher_target_shorthand"
    assert contract.teacher_target_rejection_reason("call me 2nite") == "teacher_target_characters"
    assert contract.teacher_target_rejection_reason("TXT ME") == "teacher_target_all_caps"


def test_uci_quality_uses_sourced_frequency_statistics():
    frequencies = Counter({"please": 20, "call": 20, "me": 20, "rareword": 1})
    assert (
        contract.uci_quality_rejection_reason(
            "Please call me",
            frequencies,
            single_keep=False,
        )
        is None
    )
    assert (
        contract.uci_quality_rejection_reason(
            "Please rareword",
            frequencies,
            single_keep=False,
        )
        == "uci_rare_token"
    )


def test_stable_ids_depend_on_source_generation_input_and_output():
    generation = {"method": "sourced", "teacher_model": None}
    first = contract.make_row(candidate(), generation=generation)
    second = contract.make_row(candidate(), generation=generation)
    changed = contract.make_row(candidate(text="please text me"), generation=generation)
    assert first["metadata"]["example_id"] == second["metadata"]["example_id"]
    assert first["metadata"]["example_id"] != changed["metadata"]["example_id"]


def test_generation_metadata_accepts_only_sourced_or_backboard_shapes():
    path = Path("fixture.jsonl")
    contract.validate_generation_metadata(
        {"method": "sourced", "teacher_model": None}, path, 1
    )
    contract.validate_generation_metadata(
        {
            "method": "backboard_augmentation",
            "teacher_model": "z-ai/glm-5.2",
            "verifier_model": "openai/gpt-5.6-luna",
        },
        path,
        1,
    )
    with pytest.raises(ValueError):
        contract.validate_generation_metadata(
            {"method": "backboard_augmentation", "teacher_model": "z-ai/glm-5.2"},
            path,
            1,
        )
