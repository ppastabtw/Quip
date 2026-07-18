import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "evaluate_predictions.py"
SPEC = importlib.util.spec_from_file_location("evaluate_predictions", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class EvaluationTests(unittest.TestCase):
    def test_gold_predictions_score_perfectly(self):
        dataset = MODULE.load_jsonl(ROOT / "dataset" / "eval.jsonl")
        predictions = [
            {
                "example_id": row["metadata"]["example_id"],
                "response": row["output"],
                "latency_ms": 100,
            }
            for row in dataset
        ]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "predictions.jsonl"
            path.write_text("".join(json.dumps(item) + "\n" for item in predictions), encoding="utf-8")
            report = MODULE.evaluate(ROOT / "dataset" / "eval.jsonl", path)
        self.assertEqual(report["overall_success"], 1.0)
        self.assertEqual(report["unnecessary_edit_rate"], 0.0)
        self.assertEqual(report["mean_latency_ms"], 100.0)

    def test_wrong_change_counts_as_unnecessary_edit(self):
        prediction = {
            "example_id": "eval_009",
            "response": '{"suggestion":"/opt/homebrew/bun"}',
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "predictions.jsonl"
            path.write_text(json.dumps(prediction) + "\n", encoding="utf-8")
            report = MODULE.evaluate(ROOT / "dataset" / "eval.jsonl", path)
        self.assertGreater(report["unnecessary_edit_rate"], 0.0)
        self.assertEqual(report["missing_predictions"], 15)


if __name__ == "__main__":
    unittest.main()
