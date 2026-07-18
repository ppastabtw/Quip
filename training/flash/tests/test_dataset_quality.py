import json
import tempfile
import unittest
from pathlib import Path

from dataset_quality import (
    build_dataset_quality_report,
    cross_split_diagnostics,
    levenshtein_distance,
)


ROOT = Path(__file__).resolve().parents[1]


def row(input_text: str, target: str, family: str, partition: str) -> dict:
    return {
        "input": {"text": input_text},
        "output": {"suggestion": target},
        "metadata": {
            "family_id": family,
            "source_partition": partition,
            "target_changed": input_text != target,
            "window_size": len(input_text.split()),
            "category": "fixture",
            "source_dataset": "fixture",
            "source_record_id": f"fixture:{partition}:{family}:w1:s0",
            "generation": {"method": "sourced"},
        },
    }


class DatasetQualityTests(unittest.TestCase):
    def test_levenshtein_distance(self):
        self.assertEqual(levenshtein_distance("kitten", "sitting"), 3)
        self.assertEqual(levenshtein_distance("same", "same"), 0)
        self.assertEqual(levenshtein_distance("", "abc"), 3)

    def test_cross_split_report_finds_overlap_and_conflicts(self):
        report = cross_split_diagnostics(
            {
                "train": [row("o", "to", "train-1", "train")],
                "eval": [row("o", "i", "eval-1", "dev")],
                "test": [row("safe", "safe", "test-1", "test")],
            },
            sample_limit=10,
        )
        self.assertEqual(
            report["pairs"]["train:eval"]["normalized_input_overlap"]["count"],
            1,
        )
        self.assertEqual(report["conflicting_input_targets"]["count"], 1)
        self.assertEqual(
            report["conflicting_input_targets"]["sample"][0]["input"], "o"
        )

    def test_current_dataset_matches_its_build_report(self):
        report = build_dataset_quality_report(
            ROOT / "dataset", source_records_path=None, sample_limit=3
        )
        self.assertTrue(report["build_report_matches"]["all"])
        self.assertEqual(report["splits"]["train"]["rows"], 2000)
        self.assertEqual(report["splits"]["eval"]["rows"], 200)
        self.assertEqual(report["splits"]["test"]["rows"], 200)
        self.assertGreater(
            report["splits"]["train"]["ambiguous_valid_word_candidates"]["count"],
            0,
        )

    def test_source_scenarios_are_optional(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            dataset = root / "dataset"
            dataset.mkdir()
            fixtures = {
                "train": [row("hello", "hello", "1", "train")],
                "eval": [row("weather", "weather", "2", "dev")],
                "test": [row("music", "music", "3", "test")],
            }
            for split, rows in fixtures.items():
                (dataset / f"{split}.jsonl").write_text(
                    "".join(json.dumps(value) + "\n" for value in rows),
                    encoding="utf-8",
                )

            report = build_dataset_quality_report(
                dataset, source_records_path=root / "missing.jsonl"
            )
            self.assertFalse(report["source_domain_index"]["available"])
            self.assertEqual(report["splits"]["train"]["source_scenarios"], {})


if __name__ == "__main__":
    unittest.main()
