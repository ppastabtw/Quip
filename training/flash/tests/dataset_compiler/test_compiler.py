import random

from dataset_compiler.compiler import (
    V1_POLICY,
    V2_DRAFT_POLICY,
    event_count_quotas,
    event_count_schedule,
)


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
