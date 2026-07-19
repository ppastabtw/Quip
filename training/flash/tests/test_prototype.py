import json
import http.client
import threading
import unittest

from http import HTTPStatus
from http.server import ThreadingHTTPServer
from unittest.mock import patch

from augmentation import DEFAULT_WEIGHTS as DEFAULT_AUGMENTATION_WEIGHTS
from prototype.server import (
    COMPLETION_COUNT,
    DEFAULT_MODEL,
    INDEX_PATH,
    MODEL_OPTIONS,
    PlaygroundHandler,
    PlaygroundError,
    request_payload,
    run_augmentation,
    run_prediction,
    validate_request,
)
from environment import SYSTEM_PROMPT


class PrototypeEndpointTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), PlaygroundHandler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join(timeout=2)

    def post(self, path, payload):
        connection = http.client.HTTPConnection(
            "127.0.0.1", self.server.server_address[1], timeout=2
        )
        try:
            connection.request(
                "POST",
                path,
                body=json.dumps(payload),
                headers={"Content-Type": "application/json"},
            )
            response = connection.getresponse()
            return response.status, json.loads(response.read())
        finally:
            connection.close()

    @patch("prototype.server.load_credentials", side_effect=AssertionError("credential access"))
    def test_augment_route_returns_trace_without_credentials(self, _credentials):
        status, body = self.post(
            "/api/augment",
            {"text": "Correct typing", "seed": 44, "event_count": 2},
        )

        self.assertEqual(status, HTTPStatus.OK)
        self.assertEqual(body["original"], "Correct typing")
        self.assertEqual(body["seed"], 44)
        self.assertEqual(len(body["operations"]), 2)

    def test_augment_route_rejects_out_of_bounds_event_count(self):
        status, body = self.post(
            "/api/augment",
            {"text": "Correct typing", "seed": 44, "event_count": 11},
        )

        self.assertEqual(status, HTTPStatus.BAD_REQUEST)
        self.assertIn("event_count must be between", body["error"])


class PrototypeRequestTests(unittest.TestCase):
    def test_prototype_uses_exclusive_lab_and_model_tabs(self):
        html = INDEX_PATH.read_text(encoding="utf-8")

        self.assertIn('role="tablist"', html)
        self.assertIn('id="augment-tab" role="tab"', html)
        self.assertIn('id="model-tab" role="tab"', html)
        self.assertIn(
            'id="model-panel" role="tabpanel" aria-labelledby="model-tab" hidden',
            html,
        )
        self.assertIn("activateTab(document.querySelector('#model-tab'))", html)
        self.assertNotIn('id="suggestion-count"', html)
        self.assertIn("Every run requests five managed completions", html)
        self.assertIn('id="empty-candidates" hidden', html)
        self.assertIn("item.vote_count", html)
        self.assertNotIn('id="augment-seed" type="number" min="0" max="2147483647" step="1" value="42"', html)
        self.assertIn("crypto.getRandomValues(values)", html)
        self.assertIn("augmentSeed.value = randomSeed()", html)

    def test_augmentation_weights_have_explanatory_tooltips(self):
        html = INDEX_PATH.read_text(encoding="utf-8")

        explanations = {
            "substitution": "Replaces a character with a nearby key",
            "deletion": "Removes one character",
            "insertion": "Adds a nearby QWERTY key",
            "transpose": "Swaps two adjacent characters",
            "repeat": "Duplicates one non-space character",
            "spacing": "Adds a missing space or removes an existing one",
        }
        for operator, explanation in explanations.items():
            self.assertIn(f'aria-label="About {operator}"', html)
            self.assertIn(f'aria-describedby="tooltip-{operator}"', html)
            self.assertIn(explanation, html)

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
            }
        )

        self.assertEqual(model, "Qwen/Qwen3.5-2B")
        self.assertEqual(model_input, {"text": "cnt cm tmrw"})
        self.assertEqual(settings["temperature"], 0.9)
        self.assertEqual(settings["max_tokens"], 96)
        self.assertEqual(COMPLETION_COUNT, 5)

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
        schema = payload["response_format"]["json_schema"]["schema"]
        self.assertEqual(schema["required"], ["suggestion"])
        self.assertFalse(schema["additionalProperties"])

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
        with self.assertRaisesRegex(PlaygroundError, "max_tokens must be between"):
            validate_request(
                {
                    "model": "Qwen/Qwen3.5-2B",
                    "system_prompt": SYSTEM_PROMPT,
                    "draft": "hello",
                    "max_tokens": 513,
                }
            )

    @patch("prototype.server.load_credentials", side_effect=AssertionError("credential access"))
    def test_augmentation_is_deterministic_without_flash_credentials(self, _credentials):
        payload = {
            "text": "Meet me at seven.",
            "seed": 120,
            "event_count": 2,
            "weights": {
                "substitution": 59,
                "deletion": 16,
                "insertion": 10,
                "transposition": 2,
                "repeat": 8,
                "spacing": 5,
            },
        }

        first = run_augmentation(payload)
        second = run_augmentation(payload)

        self.assertEqual(first, second)
        self.assertEqual(first["original"], payload["text"])
        self.assertEqual(first["seed"], payload["seed"])
        self.assertEqual(len(first["operations"]), 2)

    def test_augmentation_rejects_invalid_endpoint_controls(self):
        with self.assertRaisesRegex(PlaygroundError, "seed must be between"):
            run_augmentation({"text": "hello", "seed": -1})
        with self.assertRaisesRegex(PlaygroundError, "weights must contain exactly"):
            run_augmentation({"text": "hello", "weights": {"deletion": 1}})
        weights = dict(DEFAULT_AUGMENTATION_WEIGHTS)
        weights["spacing"] = float("nan")
        with self.assertRaisesRegex(PlaygroundError, "weight spacing must be finite"):
            run_augmentation({"text": "hello", "weights": weights})

    @patch("prototype.server._run_suggestion")
    @patch("prototype.server.load_credentials", return_value=("account", "key"))
    def test_always_runs_five_and_ranks_each_unique_changed_completion(
        self, _credentials, run_one
    ):
        run_one.side_effect = lambda model, model_input, settings, headers, index: {
            "index": index,
            "latency_ms": 10 + index,
            "suggestion": f"suggestion {index}",
            "raw": f"suggestion {index}",
            "usage": {"prompt_tokens": 1, "completion_tokens": 1},
        }

        result = run_prediction(
            {
                "model": "Qwen/Qwen3.5-2B",
                "system_prompt": "Changed prompt for metadata.",
                "draft": "hello",
                "suggestion_count": 1,
            }
        )

        self.assertEqual(run_one.call_count, 5)
        self.assertEqual([item["index"] for item in result["suggestions"]], [1, 2, 3, 4, 5])
        self.assertEqual([item["rank"] for item in result["suggestions"]], [1, 2, 3, 4, 5])
        self.assertEqual([item["vote_count"] for item in result["suggestions"]], [1, 1, 1, 1, 1])
        self.assertEqual(result["completion_count"], 5)
        self.assertEqual(result["candidate_count"], 5)
        self.assertEqual(
            result["settings"]["system_prompt"], "Changed prompt for metadata."
        )
        self.assertNotIn("suggestion_count", result["settings"])

    @patch("prototype.server._run_suggestion")
    @patch("prototype.server.load_credentials", return_value=("account", "key"))
    def test_filters_exact_text_and_ranks_by_votes_then_earliest_index(
        self, _credentials, run_one
    ):
        outputs = {1: "alpha", 2: "hello", 3: "beta", 4: "beta", 5: "alpha"}
        run_one.side_effect = lambda model, model_input, settings, headers, index: {
            "index": index,
            "latency_ms": 10 + index,
            "suggestion": outputs[index],
            "raw": outputs[index],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1},
        }

        result = run_prediction(
            {
                "model": "Qwen/Qwen3.5-2B",
                "system_prompt": SYSTEM_PROMPT,
                "draft": "hello",
            }
        )

        self.assertEqual(result["completion_count"], 5)
        self.assertEqual(result["candidate_count"], 2)
        self.assertEqual(
            [item["suggestion"] for item in result["suggestions"]],
            ["alpha", "beta"],
        )
        self.assertEqual([item["rank"] for item in result["suggestions"]], [1, 2])
        self.assertEqual([item["vote_count"] for item in result["suggestions"]], [2, 2])
        self.assertEqual(
            [item["completion_indices"] for item in result["suggestions"]],
            [[1, 5], [3, 4]],
        )

    @patch("prototype.server._run_suggestion")
    @patch("prototype.server.load_credentials", return_value=("account", "key"))
    def test_all_exact_completions_return_successful_zero_candidate_state(
        self, _credentials, run_one
    ):
        run_one.side_effect = lambda model, model_input, settings, headers, index: {
            "index": index,
            "latency_ms": 10 + index,
            "suggestion": model_input["text"],
            "raw": model_input["text"],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1},
        }

        result = run_prediction(
            {
                "model": "Qwen/Qwen3.5-2B",
                "system_prompt": SYSTEM_PROMPT,
                "draft": "hello",
            }
        )

        self.assertEqual(result["completion_count"], 5)
        self.assertEqual(result["candidate_count"], 0)
        self.assertEqual(result["suggestions"], [])

if __name__ == "__main__":
    unittest.main()
