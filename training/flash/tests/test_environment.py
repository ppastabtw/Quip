import json
import unittest

from dataset_compiler.contract import CONTRACT
from environment import (
    HYBRID_SYSTEM_PROMPT,
    SYSTEM_PROMPT,
    QuipEnvironment,
    load_environment,
    model_input_json,
)
from scoring import model_text


class EnvironmentTests(unittest.TestCase):
    def test_loads_train_and_eval_splits(self):
        self.assertEqual(
            len(load_environment(split="train").dataset), CONTRACT.train_size
        )
        self.assertEqual(
            len(load_environment(split="eval").dataset), CONTRACT.eval_size
        )

    def test_prompt_contains_policy_and_input(self):
        environment = QuipEnvironment(split="train")
        example = environment.dataset[0]
        messages = environment.build_prompt_messages(example, "")
        self.assertEqual(messages[0]["role"], "system")
        self.assertIn("English text corrector", messages[0]["content"])
        self.assertIn("exactly one JSON object", messages[0]["content"])
        self.assertEqual(
            messages[1]["content"],
            json.dumps(
                json.loads(example.input),
                ensure_ascii=False,
                separators=(",", ":"),
            ),
        )
        self.assertEqual(set(json.loads(messages[1]["content"])), {"text"})

    def test_model_input_projects_finalized_fields_in_stable_order(self):
        value = model_input_json(
            {
                "text": "meet there tmrw",
                "personal_patterns": [{"shorthand": "tmrw", "expansion": "tomorrow"}],
                "context_snippets": [
                    {
                        "app_name": "Notes",
                        "window_title": "Trip planning",
                        "visible_text": "Tomorrow: meet at Union Station.",
                    }
                ],
            }
        )

        self.assertEqual(
            value,
            '{"context_snippets":[{"app_name":"Notes","window_title":"Trip planning","visible_text":"Tomorrow: meet at Union Station."}],"text":"meet there tmrw"}',
        )
        self.assertNotIn("personal_patterns", value)

    def test_prompt_has_policy_without_answer_shaped_text(self):
        self.assertIn("actual complete text", SYSTEM_PROMPT)
        self.assertIn("If no correction is needed", SYSTEM_PROMPT)
        self.assertNotIn("personal patterns", SYSTEM_PROMPT)
        self.assertNotIn("Suggestion text:", SYSTEM_PROMPT)
        self.assertNotIn("best full text", SYSTEM_PROMPT.lower())
        self.assertIn("Never replace an explicit value", SYSTEM_PROMPT)

    def test_gold_output_passes_environment_reward(self):
        environment = QuipEnvironment(split="eval")
        example = environment.dataset[0]
        reward = environment.score_response(example, model_text(example.output))
        self.assertEqual(reward.score, 1.0)
        self.assertTrue(reward.success)

    def test_hybrid_environment_preserves_text_and_adds_lexical_hints(self):
        content = json.loads(
            model_input_json(
                {
                    "text": "hteir going",
                    "personal_patterns": [
                        {"shorthand": "hteir", "expansion": "their"}
                    ],
                    "context_snippets": [
                        {
                            "app_name": "Notes",
                            "window_title": "Draft",
                            "visible_text": "They are going tomorrow.",
                        }
                    ],
                },
                lexical_hints=True,
            )
        )
        self.assertEqual(
            set(content), {"context_snippets", "text", "lexical_hints"}
        )
        self.assertEqual(content["text"], "hteir going")
        self.assertTrue(content["lexical_hints"])

        environment = QuipEnvironment(split="train", lexical_hints=True)
        example = environment.dataset[0]
        messages = environment.build_prompt_messages(example, "")
        self.assertEqual(messages[0]["content"], HYBRID_SYSTEM_PROMPT)
        self.assertIn("lexical_hints", json.loads(messages[1]["content"]))

    def test_model_input_combines_context_and_lexical_hints(self):
        content = json.loads(
            model_input_json(
                {
                    "text": "met at gat c14",
                    "context_snippets": [
                        {
                            "app_name": "Calendar",
                            "window_title": "Flight",
                            "visible_text": "Gate C14",
                        }
                    ],
                },
                lexical_hints=True,
            )
        )
        self.assertEqual(
            set(content), {"context_snippets", "text", "lexical_hints"}
        )
        self.assertEqual(content["context_snippets"][0]["visible_text"], "Gate C14")
        self.assertTrue(content["lexical_hints"])


if __name__ == "__main__":
    unittest.main()
