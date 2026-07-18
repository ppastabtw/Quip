from dataclasses import replace
from pathlib import Path

import pytest

from dataset_compiler import contract
from tests.dataset_compiler.helpers import candidate


def test_contract_has_equal_window_sizes_and_ten_percent_unchanged():
    train = contract.CONTRACT.expected_counts("train")
    eval_split = contract.CONTRACT.expected_counts("eval")
    assert train["window_sizes"] == {1: 400, 2: 400, 3: 400, 4: 400, 5: 400}
    assert train["unchanged_by_size"] == {1: 40, 2: 40, 3: 40, 4: 40, 5: 40}
    assert train["changed"] == 1800
    assert eval_split["window_sizes"] == {1: 40, 2: 40, 3: 40, 4: 40, 5: 40}
    assert eval_split["unchanged_by_size"] == {1: 4, 2: 4, 3: 4, 4: 4, 5: 4}
    assert eval_split["changed"] == 180


def test_source_filter_rejects_profanity_and_personal_data():
    assert contract.source_rejection_reason("please call me") is None
    assert contract.source_rejection_reason("this is damn bad") == "profanity"
    assert contract.source_rejection_reason("me@example.com") == "email"
    assert contract.source_rejection_reason("call 416 555 0199") == "phone"
    assert contract.source_rejection_reason("@person hello") == "handle"


def test_window_filter_enforces_requested_size_only():
    assert contract.window_rejection_reason("one two three", expected_size=3) is None
    assert contract.window_rejection_reason("one two three", expected_size=2) == "window_size"


def test_draft_filter_allows_spacing_changes_and_ambiguity_filter_rejects_common_words():
    assert contract.draft_rejection_reason("onetwo three") is None
    assert contract.draft_rejection_reason("one twoo three") is None
    assert contract.draft_rejection_reason("one two ") == "irregular_whitespace"
    assert (
        contract.ambiguity_rejection_reason("on", minimum_zipf_frequency=3.0)
        == "all_tokens_common_valid_words"
    )
    assert contract.ambiguity_rejection_reason("on", minimum_zipf_frequency=None) is None
    vocabulary = {"on", "turn", "he"}
    assert (
        contract.ambiguity_rejection_reason(
            "on teh",
            minimum_zipf_frequency=3.0,
            valid_vocabulary=vocabulary,
        )
        is None
    )
    assert (
        contract.ambiguity_rejection_reason(
            "turn on he",
            minimum_zipf_frequency=3.0,
            valid_vocabulary=vocabulary,
        )
        == "all_tokens_common_valid_words"
    )


def test_stable_ids_depend_on_source_generation_input_and_output():
    generation = {"method": "sourced"}
    first = contract.make_row(candidate(), generation=generation)
    second = contract.make_row(candidate(), generation=generation)
    changed = contract.make_row(candidate(text="please text me"), generation=generation)
    assert first["metadata"]["example_id"] == second["metadata"]["example_id"]
    assert first["metadata"]["example_id"] != changed["metadata"]["example_id"]


def test_window_size_comes_from_clean_target_after_spacing_mutation():
    row = contract.make_row(
        replace(
            candidate(text="pleasetext me"),
            target="please text me",
            target_changed=True,
        ),
        generation={
            "method": "qwerty_augmentation",
            "seed": 1,
            "requested_events": 1,
            "operations": [
                {
                    "event": 1,
                    "operator": "spacing",
                    "index": 6,
                    "source": " ",
                    "replacement": "",
                }
            ],
        },
    )
    assert row["metadata"]["window_size"] == 3


def test_generation_metadata_accepts_sourced_and_augmentation_shapes():
    path = Path("fixture.jsonl")
    contract.validate_generation_metadata({"method": "sourced"}, path, 1)
    contract.validate_generation_metadata(
        {
            "method": "qwerty_augmentation",
            "seed": 42,
            "requested_events": 1,
            "operations": [
                {
                    "event": 1,
                    "operator": "deletion",
                    "index": 2,
                    "source": "e",
                    "replacement": "",
                }
            ],
        },
        path,
        1,
    )
    contract.validate_generation_metadata(
        {
            "method": "qwerty_augmentation",
            "seed": 42,
            "requested_events": 2,
            "operations": [
                {
                    "event": event,
                    "operator": "deletion",
                    "index": event,
                    "source": "e",
                    "replacement": "",
                }
                for event in (1, 2)
            ],
        },
        path,
        1,
    )


def test_cross_split_validator_rejects_injected_surface_leakage():
    train_row = contract.make_row(
        candidate(index=1, text="shared draft"),
        generation={"method": "sourced"},
    )
    eval_row = contract.make_row(
        replace(
            candidate(index=2, text="shared draft"),
            source_partition="dev",
        ),
        generation={"method": "sourced"},
    )

    with pytest.raises(ValueError, match="normalized input or target leakage"):
        contract.validate_cross_split_isolation(
            {"train": [train_row], "eval": [eval_row]},
            require_normalized_surface_isolation=True,
        )
