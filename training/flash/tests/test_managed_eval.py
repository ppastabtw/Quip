import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "run_managed_eval.py"
SPEC = importlib.util.spec_from_file_location("run_managed_eval", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class ManagedEvalTests(unittest.TestCase):
    def test_request_disables_thinking_and_requires_json_replies(self):
        row = {"input": '{"text":"cnt cm tmrw"}'}
        payload = MODULE.request_payload(row, "Qwen/Qwen3.5-2B")
        self.assertFalse(payload["chat_template_kwargs"]["enable_thinking"])
        schema = payload["response_format"]["json_schema"]["schema"]
        self.assertEqual(schema["required"], ["suggestion"])
        self.assertFalse(schema["additionalProperties"])
        self.assertEqual(payload["temperature"], 0.7)

    def test_hybrid_request_keeps_raw_text_and_adds_hints(self):
        row = {"input": {"text": "hteir going tommorow"}}
        payload = MODULE.request_payload(row, "Qwen/Qwen3.5-2B", lexical_hints=True)
        content = payload["messages"][1]["content"]
        parsed = json.loads(content)
        self.assertEqual(parsed["text"], "hteir going tommorow")
        self.assertTrue(parsed["lexical_hints"])
        self.assertIn("uncertain hints", payload["messages"][0]["content"])

    def test_resume_skips_completed_examples(self):
        rows = [
            {"metadata": {"example_id": "done"}},
            {"metadata": {"example_id": "pending"}},
        ]
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "predictions.jsonl"
            output.write_text('{"example_id":"done"}\n', encoding="utf-8")
            remaining = MODULE.rows_to_run(rows, output, resume=True)
        self.assertEqual(
            [row["metadata"]["example_id"] for row in remaining], ["pending"]
        )


if __name__ == "__main__":
    unittest.main()
