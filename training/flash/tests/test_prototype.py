import json
import unittest

from unittest.mock import patch

from prototype.server import (
    DEFAULT_MODEL,
    MODEL_OPTIONS,
    PlaygroundError,
    request_payload,
    run_prediction,
    validate_request,
)
from environment import SYSTEM_PROMPT


class PrototypeRequestTests(unittest.TestCase):
    def test_model_catalog_has_unique_ids_and_contains_default(self):
        ids = [item["id"] for item in MODEL_OPTIONS]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertIn(DEFAULT_MODEL, ids)
        self.assertTrue(all(item["name"] for item in MODEL_OPTIONS))

    def test_builds_training_contract_input(self):
        model, model_input, settings = validate_request(
            {
                "model": "Qwen/Qwen3.5-2B",
                "system_prompt": "Return one suggestion.",
                "draft": "cnt cm tmrw",
                "temperature": 0.9,
                "max_tokens": 96,
                "suggestion_count": 4,
            }
        )

        self.assertEqual(model, "Qwen/Qwen3.5-2B")
        self.assertEqual(model_input, {"text": "cnt cm tmrw"})
        self.assertEqual(settings["temperature"], 0.9)
        self.assertEqual(settings["max_tokens"], 96)
        self.assertEqual(settings["suggestion_count"], 4)

        payload = request_payload(
            model,
            model_input,
            system_prompt=settings["system_prompt"],
            temperature=settings["temperature"],
            max_tokens=settings["max_tokens"],
        )
        encoded_input = payload["messages"][1]["content"]
        self.assertEqual(json.loads(encoded_input), model_input)
        self.assertEqual(payload["messages"][0]["content"], "Return one suggestion.")
        self.assertEqual(payload["temperature"], 0.9)
        self.assertEqual(payload["max_tokens"], 96)
        self.assertFalse(payload["chat_template_kwargs"]["enable_thinking"])
        self.assertEqual(payload["response_format"]["type"], "json_schema")

    def test_rejects_blank_draft(self):
        with self.assertRaisesRegex(PlaygroundError, "draft must be a non-empty string"):
            validate_request(
                {
                    "model": "Qwen/Qwen3.5-2B",
                    "system_prompt": SYSTEM_PROMPT,
                    "draft": "  ",
                }
            )

    def test_rejects_out_of_range_controls(self):
        with self.assertRaisesRegex(PlaygroundError, "suggestion_count must be between"):
            validate_request(
                {
                    "model": "Qwen/Qwen3.5-2B",
                    "system_prompt": SYSTEM_PROMPT,
                    "draft": "hello",
                    "suggestion_count": 6,
                }
            )

    @patch("prototype.server._run_suggestion")
    @patch("prototype.server.load_credentials", return_value=("account", "key"))
    def test_runs_each_suggestion_as_an_individual_completion(self, _credentials, run_one):
        run_one.side_effect = lambda model, model_input, settings, headers, index: {
            "index": index,
            "latency_ms": 10 + index,
            "suggestion": f"suggestion {index}",
            "changed": True,
            "raw": json.dumps({"suggestion": f"suggestion {index}"}),
            "usage": {"prompt_tokens": 1, "completion_tokens": 1},
        }

        result = run_prediction(
            {
                "model": "Qwen/Qwen3.5-2B",
                "system_prompt": SYSTEM_PROMPT,
                "draft": "hello",
                "suggestion_count": 3,
            }
        )

        self.assertEqual(run_one.call_count, 3)
        self.assertEqual([item["index"] for item in result["suggestions"]], [1, 2, 3])
        self.assertEqual(result["settings"]["suggestion_count"], 3)

if __name__ == "__main__":
    unittest.main()
