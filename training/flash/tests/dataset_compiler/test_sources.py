import json
import random

from dataset_compiler import sources


def test_massive_parser_samples_one_random_window_per_source(tmp_path):
    path = tmp_path / "en-US.jsonl"
    rows = [
        {
            "id": str(index),
            "locale": "en-US",
            "partition": "train",
            "scenario": "general",
            "intent": "general_query",
            "utt": f"please check the weather for item {index}",
        }
        for index in range(100)
    ]
    path.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")
    parsed = sources.parse_massive(path)
    windows = sources.sample_massive_windows(
        parsed,
        partition="train",
        required_by_size={size: 2 for size in range(1, 6)},
        rng=random.Random(42),
        buffer_rows=0,
    )
    assert {size: len(values) for size, values in windows.items()} == {
        size: 2 for size in range(1, 6)
    }
    assert all(item.source_partition == "train" for values in windows.values() for item in values)
    families = [item.family_id for values in windows.values() for item in values]
    assert len(families) == len(set(families))


def test_runtime_chunk_sampling_uses_five_word_boundaries(tmp_path):
    path = tmp_path / "en-US.jsonl"
    row = {
        "id": "runtime-boundary",
        "locale": "en-US",
        "partition": "train",
        "scenario": "general",
        "intent": "general_query",
        "utt": "please check the weather today call me",
    }
    path.write_text(json.dumps(row) + "\n", encoding="utf-8")
    parsed = sources.parse_massive(path)
    windows = sources.sample_massive_windows(
        parsed,
        partition="train",
        required_by_size={1: 0, 2: 1, 3: 0, 4: 0, 5: 0},
        rng=random.Random(42),
        buffer_rows=0,
        sampling="runtime_chunks",
    )
    assert windows[2][0].text == "call me"
    assert windows[2][0].source_record_id.endswith(":w2:s5")


def test_runtime_chunk_sampling_supports_ten_word_boundaries(tmp_path):
    path = tmp_path / "en-US.jsonl"
    row = {
        "id": "ten-word-boundary",
        "locale": "en-US",
        "partition": "train",
        "scenario": "general",
        "intent": "general_query",
        "utt": "please check the weather and then call my sister after lunch",
    }
    path.write_text(json.dumps(row) + "\n", encoding="utf-8")

    windows = sources.sample_massive_windows(
        sources.parse_massive(path),
        partition="train",
        required_by_size={size: int(size == 1) for size in range(1, 11)},
        rng=random.Random(42),
        buffer_rows=0,
        sampling="runtime_chunks",
        window_sizes=tuple(range(1, 11)),
    )

    assert windows[1][0].text == "lunch"
    assert windows[1][0].source_record_id.endswith(":w1:s10")


def test_source_buffer_is_optional_when_required_pool_is_available(tmp_path):
    path = tmp_path / "en-US.jsonl"
    row = {
        "id": "one-runtime-window",
        "locale": "en-US",
        "partition": "train",
        "scenario": "general",
        "intent": "general_query",
        "utt": "call me",
    }
    path.write_text(json.dumps(row) + "\n", encoding="utf-8")
    parsed = sources.parse_massive(path)

    windows = sources.sample_massive_windows(
        parsed,
        partition="train",
        required_by_size={1: 0, 2: 1, 3: 0, 4: 0, 5: 0},
        rng=random.Random(42),
        buffer_rows={1: 0, 2: 5, 3: 0, 4: 0, 5: 0},
        sampling="runtime_chunks",
    )

    assert len(windows[2]) == 1
