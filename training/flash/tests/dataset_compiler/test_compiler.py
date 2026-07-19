import random

from dataset_compiler.compiler import (
    V1_POLICY,
    V2_DRAFT_POLICY,
    V2_RUNTIME_10W_5K_POLICY,
    event_count_quotas,
    event_count_schedule,
    window_share_report,
)


def test_v2_runtime_10w_5k_policy_uses_requested_split_sizes():
    contract = V2_RUNTIME_10W_5K_POLICY.contract

    assert contract.expected_counts("train")["rows"] == 5000
    assert contract.expected_counts("eval")["rows"] == 1000
    assert contract.expected_counts("test")["rows"] == 1000
    assert contract.expected_counts("train")["window_sizes"] == {
        1: 160,
        2: 320,
        3: 400,
        4: 560,
        5: 640,
        6: 680,
        7: 600,
        8: 440,
        9: 360,
        10: 840,
    }
    assert contract.expected_counts("eval")["window_sizes"] == {
        1: 40,
        2: 80,
        3: 80,
        4: 120,
        5: 120,
        6: 120,
        7: 120,
        8: 80,
        9: 80,
        10: 160,
    }
    assert set(window_share_report(contract)["window_shares_by_split"]) == {
        "train",
        "eval",
        "test",
    }


def test_v1_window_share_report_keeps_legacy_scalar():
    assert window_share_report(V1_POLICY.contract) == {"window_share": 0.2}


def test_v1_schedule_does_not_consume_rng_state():
    actual_rng = random.Random(42)
    expected_rng = random.Random(42)

    assert event_count_schedule(360, policy=V1_POLICY, rng=actual_rng) == [1] * 360
    assert actual_rng.random() == expected_rng.random()


def test_v2_event_quota_is_bounded_and_centered_near_two():
    quotas = event_count_quotas(360, V2_DRAFT_POLICY)

    assert quotas == {1: 90, 2: 150, 3: 90, 4: 30}
    mean = sum(events * count for events, count in quotas.items()) / sum(quotas.values())
    assert abs(mean - 2.0) < 0.25


def test_v2_event_schedule_is_deterministic_and_preserves_quota():
    first = event_count_schedule(36, policy=V2_DRAFT_POLICY, rng=random.Random(7))
    second = event_count_schedule(36, policy=V2_DRAFT_POLICY, rng=random.Random(7))

    assert first == second
    assert {events: first.count(events) for events in range(1, 5)} == {
        1: 9,
        2: 15,
        3: 9,
        4: 3,
    }


def test_v2_one_word_schedule_caps_severity_at_two_events():
    schedule = event_count_schedule(
        36,
        policy=V2_DRAFT_POLICY,
        rng=random.Random(7),
        window_size=1,
    )

    assert {events: schedule.count(events) for events in range(1, 5)} == {
        1: 21,
        2: 15,
        3: 0,
        4: 0,
    }


def test_v2_two_word_schedule_caps_severity_at_three_events():
    schedule = event_count_schedule(
        36,
        policy=V2_DRAFT_POLICY,
        rng=random.Random(7),
        window_size=2,
    )

    assert {events: schedule.count(events) for events in range(1, 5)} == {
        1: 12,
        2: 15,
        3: 9,
        4: 0,
    }
