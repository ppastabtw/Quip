import json
from dataclasses import replace
from pathlib import Path

import pytest
import httpx

from synthetic_data.config import ModelConfig, load_config
from synthetic_data.dedupe import Deduplicator
from synthetic_data.models import Candidate, Judgment
from synthetic_data.pipeline import SyntheticPipeline, _parse_judgments
from synthetic_data.provider import BackboardStructuredClient, MockStructuredClient
from synthetic_data.scheduling import (
    CONTRAST_VARIANTS,
    allocate_counts,
    make_slots,
    take_slots,
)
from synthetic_data.validation import (
    contrast_group_rejections,
    judgment_rejection_reasons,
    validate_candidate,
)


ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "configs" / "synthetic-context-v1.toml"


def candidate_mapping(**overrides):
    value = {
        "slot_id": "r01_s00000001",
        "category": "entity_spelling",
        "context_behavior": "useful",
        "group_id": None,
        "variant": None,
        "domain": "workplace",
        "error_type": "phonetic",
        "writing_style": "lowercase_texting",
        "input": {
            "text": "tell shivon ill call",
            "context_snippets": [
                {
                    "app_name": "Messages",
                    "window_title": "Siobhan",
                    "visible_text": "Siobhan: call after lunch",
                }
            ],
        },
        "output": {"suggestion": "tell Siobhan I'll call"},
        "rationale": "The recipient title supplies the exact spelling.",
    }
    value.update(overrides)
    return value


def passing_judgment(candidate_id: str) -> Judgment:
    return Judgment.from_mapping(
        {
            "candidate_id": candidate_id,
            "pass": True,
            "scores": {
                "correctness": 5,
                "context_grounding": 5,
                "context_usefulness": 5,
                "meaning_preservation": 5,
                "tone_preservation": 5,
                "minimality": 5,
                "naturalness": 5,
                "category_validity": 5,
                "dataset_value": 5,
            },
            "failure_reasons": [],
            "notes": "",
        }
    )


def test_config_and_largest_remainder_balancing_are_exact():
    config = load_config(CONFIG)
    counts = allocate_counts(101, config.behaviors)
    assert sum(counts.values()) == 101
    assert counts["useful"] > counts["irrelevant"] > counts["ambiguous"]
    assert sum(config.categories.values()) == pytest.approx(1.0)


def test_category_override_is_normalized_and_rejects_unknown():
    config = load_config(CONFIG)
    selected = config.with_overrides(categories=["entity_spelling", "phonetic_resolution"])
    assert selected.categories["entity_spelling"] == 0.5
    assert selected.categories["vague_reference"] == 0
    with pytest.raises(ValueError, match="unknown categories"):
        config.with_overrides(categories=["not-real"])


def test_scheduler_keeps_complete_contrast_groups_together():
    config = load_config(CONFIG)
    slots = make_slots(config)
    groups = {}
    for slot in slots:
        if slot.group_id:
            groups.setdefault(slot.group_id, []).append(slot)
    assert groups
    assert all({slot.variant for slot in rows} == {name for name, _ in CONTRAST_VARIANTS} for rows in groups.values())
    for rows in groups.values():
        assert len({slot.category for slot in rows}) == 1
        indexes = sorted(slots.index(slot) for slot in rows)
        assert indexes == list(range(indexes[0], indexes[0] + 6))


def test_scheduler_pairs_restraint_categories_with_compatible_behaviors():
    config = load_config(CONFIG)
    slots = make_slots(config)
    expected = {
        "irrelevant_misleading_context": "irrelevant",
        "explicit_draft_override": "conflicting",
        "ambiguous_context": "ambiguous",
        "stale_erroneous_context": "conflicting",
        "already_correct": "irrelevant",
    }
    for slot in slots:
        if slot.group_id is None and slot.category in expected:
            assert slot.context_behavior == expected[slot.category]


def test_slot_cap_is_exact_and_prefers_complete_groups():
    slots = make_slots(load_config(CONFIG).with_overrides(count=200))
    selected = take_slots(slots, 300)
    assert len(selected) == 300
    selected_groups = {}
    for slot in selected:
        if slot.group_id:
            selected_groups.setdefault(slot.group_id, []).append(slot)
    partial = [rows for rows in selected_groups.values() if len(rows) != 6]
    assert len(partial) <= 1


def test_candidate_schema_and_deterministic_validation():
    config = load_config(CONFIG)
    candidate = Candidate.from_mapping(candidate_mapping(), run_id="test")
    assert validate_candidate(candidate, config) == []
    malformed = candidate_mapping(output={"suggestion": "x", "commentary": "bad"})
    with pytest.raises(ValueError, match="exactly suggestion"):
        Candidate.from_mapping(malformed, run_id="test")
    no_context = Candidate.from_mapping(
        candidate_mapping(context_behavior="none"), run_id="test"
    )
    assert "no_context_behavior_has_context" in validate_candidate(no_context, config)


def test_judge_parsing_and_threshold_routing():
    candidate = Candidate.from_mapping(candidate_mapping(), run_id="test")
    judgment = passing_judgment(candidate.candidate_id)
    parsed = _parse_judgments(
        json.dumps({"judgments": [judgment.to_dict()]}),
        candidate_ids=[candidate.candidate_id],
    )
    assert parsed == (judgment,)
    low = Judgment.from_mapping(
        {
            **judgment.to_dict(),
            "scores": {**judgment.scores, "dataset_value": 3},
        }
    )
    assert "judge_dataset_value_below_threshold" in judgment_rejection_reasons(
        low, load_config(CONFIG)
    )


def test_exact_normalized_and_near_deduplication():
    first = Candidate.from_mapping(candidate_mapping(), run_id="test")
    exact = Candidate.from_mapping(candidate_mapping(slot_id="r01_s00000002"), run_id="test")
    normalized = Candidate.from_mapping(
        candidate_mapping(
            slot_id="r01_s00000003",
            input={
                "text": "  TELL SHIVON ILL CALL ",
                "context_snippets": [
                    {
                        "app_name": "messages",
                        "window_title": "siobhan",
                        "visible_text": "siobhan: call after lunch",
                    }
                ],
            },
            output={"suggestion": "TELL SIOBHAN I'LL CALL"},
        ),
        run_id="test",
    )
    dedupe = Deduplicator(near_threshold=0.9)
    assert dedupe.rejection_reason(first) is None
    assert dedupe.rejection_reason(exact) == "exact_duplicate"
    assert dedupe.rejection_reason(normalized) == "normalized_duplicate"


def test_reference_dataset_duplicates_are_identified_separately():
    reference = Candidate.from_mapping(candidate_mapping(), run_id="reference")
    duplicate = Candidate.from_mapping(
        candidate_mapping(slot_id="r01_s00000002"), run_id="new-run"
    )
    dedupe = Deduplicator(near_threshold=0.9)
    dedupe.seed_reference(reference)
    assert dedupe.rejection_reason(duplicate) == "reference_exact_duplicate"


def test_contrast_group_validation_detects_incomplete_group():
    candidate = Candidate.from_mapping(
        candidate_mapping(group_id="g1", variant="context_a"), run_id="test"
    )
    reasons = contrast_group_rejections([candidate])
    assert reasons[candidate.candidate_id] == ["contrast_group_incomplete_or_duplicate"]


def test_mock_pipeline_routes_artifacts_and_resumes(tmp_path):
    config = load_config(CONFIG).with_overrides(count=12)
    config = replace(config, run=replace(config.run, near_duplicate_threshold=1.0, max_generation_rounds=1))
    pipeline = SyntheticPipeline(
        config=config,
        client=MockStructuredClient(),
        output_dir=tmp_path,
        run_id="mock-test",
    )
    first = pipeline.generate()
    raw_count = len((tmp_path / "raw_candidates.jsonl").read_text(encoding="utf-8").splitlines())
    second = pipeline.generate()
    assert first["generated"] > 0
    assert second["generated"] == 0
    assert len((tmp_path / "raw_candidates.jsonl").read_text(encoding="utf-8").splitlines()) == raw_count
    assert pipeline.judge()["judged"] > 0
    summary = pipeline.build()
    assert summary["raw_candidates"] == raw_count
    for name in (
        "raw_responses.jsonl",
        "raw_candidates.jsonl",
        "local_validation.jsonl",
        "judge_results.jsonl",
        "accepted_examples.jsonl",
        "rejected_examples.jsonl",
        "train.jsonl",
        "summary.json",
        "state.json",
        "manifest.json",
    ):
        assert (tmp_path / name).is_file()
    for row in [json.loads(line) for line in (tmp_path / "train.jsonl").read_text(encoding="utf-8").splitlines()]:
        assert set(row) == {"input", "output", "metadata"}
        assert set(row["output"]) == {"suggestion"}


def test_final_selection_fills_from_individually_valid_group_variants(tmp_path):
    config = load_config(CONFIG).with_overrides(count=2)
    pipeline = SyntheticPipeline(
        config=config,
        client=MockStructuredClient(),
        output_dir=tmp_path,
        run_id="selection-test",
    )
    first = Candidate.from_mapping(
        candidate_mapping(group_id="partial-a", variant="context_a"),
        run_id="selection-test",
    )
    second = Candidate.from_mapping(
        candidate_mapping(
            slot_id="r01_s00000002",
            group_id="partial-b",
            variant="context_b",
            input={
                "text": "tell mateo ill call",
                "context_snippets": [
                    {
                        "app_name": "Messages",
                        "window_title": "Mateo",
                        "visible_text": "Mateo: call after lunch",
                    }
                ],
            },
            output={"suggestion": "tell Mateo I'll call"},
        ),
        run_id="selection-test",
    )
    selected = pipeline._select_final(
        [(first, passing_judgment(first.candidate_id)), (second, passing_judgment(second.candidate_id))]
    )
    assert {pair[0].candidate_id for pair in selected} == {
        first.candidate_id,
        second.candidate_id,
    }


def test_capacity_balanced_targets_exhaust_scarce_values_before_dominant_ones():
    targets = SyntheticPipeline._capacity_balanced_targets(
        8, {"scarce": 2, "common": 10}
    )
    assert targets == {"scarce": 2, "common": 6}


def test_pipeline_loads_reference_dataset_and_builds_rotating_diversity_context(
    tmp_path,
):
    reference_path = tmp_path / "prior.jsonl"
    reference_path.write_text(
        json.dumps(
            {
                "input": candidate_mapping()["input"],
                "output": candidate_mapping()["output"],
                "metadata": {
                    "example_id": "prior-1",
                    "category": "entity_spelling",
                    "synthetic": {
                        "context_behavior": "useful",
                        "domain": "workplace",
                        "error_type": "phonetic",
                        "writing_style": "lowercase_texting",
                    },
                },
            }
        )
        + "\n",
        encoding="utf-8",
    )
    config = load_config(CONFIG).with_overrides(count=2)
    pipeline = SyntheticPipeline(
        config=config,
        client=MockStructuredClient(),
        output_dir=tmp_path / "run",
        run_id="reference-test",
        reference_paths=[reference_path],
    )
    reference = pipeline._diversity_reference(make_slots(config)[:1])
    assert reference is not None
    assert reference["existing_rows"] == 1
    assert reference["coverage"]["categories"] == {"entity_spelling": 1}
    assert reference["avoid_examples"][0]["text"] == "tell shivon ill call"


def test_backboard_structured_client_uses_stateless_json_and_tracks_usage():
    requests = []

    def handler(request: httpx.Request) -> httpx.Response:
        if request.url.path == "/api/models":
            return httpx.Response(
                200,
                request=request,
                json={
                    "models": [
                        {
                            "provider": "fixture-provider",
                            "name": "fixture-model",
                            "input_cost_per_1m_tokens": 5,
                            "output_cost_per_1m_tokens": 30,
                        }
                    ]
                },
            )
        requests.append(json.loads(request.content))
        return httpx.Response(
            200,
            request=request,
            json={
                "status": "COMPLETED",
                "content": '{"candidates":[]}',
                "model_provider": "fixture-provider",
                "model_name": "fixture-model",
                "input_tokens": 100,
                "output_tokens": 10,
                "total_tokens": 110,
            },
        )

    model = ModelConfig("fixture-provider", "fixture-model", 0.75, 2048)
    client = BackboardStructuredClient(
        timeout_seconds=1,
        requests_per_minute=10_000,
        api_key="secret",
        transport=httpx.MockTransport(handler),
        sleep=lambda _: None,
    )
    try:
        client.validate_models([model])
        completion = client.complete(
            system_prompt="system",
            user_prompt='{"task":"generate"}',
            config=model,
        )
    finally:
        client.close()
    assert completion.estimated_cost_usd == 0.0008
    assert completion.total_tokens == 110
    assert requests == [
        {
            "content": '{"task":"generate"}',
            "system_prompt": "system",
            "llm_provider": "fixture-provider",
            "model_name": "fixture-model",
            "stream": False,
            "memory": "off",
            "web_search": "off",
            "json_output": True,
            "temperature": 0.75,
            "max_tokens": 2048,
        }
    ]
