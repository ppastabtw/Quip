import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from grpo_judge import environment as GRPO_ENVIRONMENT
from grpo_judge.judge_reward import JudgeVerdict
from scoring import model_text


def _dataset_row() -> dict:
    return {
        "input": {"text": "meet there tmrw"},
        "output": {"suggestion": "meet there tomorrow"},
        "metadata": {
            "accepted_suggestions": ["meet there tomorrow"],
            "target_changed": True,
        },
    }


class JudgeEnvironmentTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            suffix=".jsonl",
            delete=False,
        )
        temporary.write(json.dumps(_dataset_row()) + "\n")
        temporary.close()
        self.dataset_path = Path(temporary.name)

    def tearDown(self):
        self.dataset_path.unlink(missing_ok=True)

    def test_exact_suggestion_gets_full_reward_without_judge(self):
        environment = GRPO_ENVIRONMENT.QuipJudgeEnvironment(
            dataset_path=str(self.dataset_path)
        )
        example = environment.dataset[0]

        with patch.object(
            GRPO_ENVIRONMENT,
            "judge_correction",
            side_effect=AssertionError("judge should not be called"),
        ):
            reward = environment.score_response(example, model_text(example.output))

        self.assertEqual(reward.score, 1.0)
        self.assertTrue(reward.success)

    def test_judge_can_reward_a_valid_non_exact_alternative(self):
        environment = GRPO_ENVIRONMENT.QuipJudgeEnvironment(
            dataset_path=str(self.dataset_path)
        )
        example = environment.dataset[0]
        verdict = JudgeVerdict(
            correction_quality=4,
            meaning_preservation=4,
            tone_preservation=4,
            minimality=4,
            acceptable=True,
            reason="valid alternative",
        )

        with patch.object(GRPO_ENVIRONMENT, "judge_correction", return_value=verdict):
            reward = environment.score_response(
                example,
                json.dumps({"suggestion": "meet tomorrow"}),
            )

        self.assertEqual(reward.score, 0.99)
        self.assertTrue(reward.success)
        self.assertEqual(reward.reason, "valid alternative")

    def test_judge_prompt_uses_unified_context_and_lexical_input(self):
        row = _dataset_row()
        row["input"] = {
            "text": "met at gat c14",
            "context_snippets": [
                {
                    "app_name": "Calendar",
                    "window_title": "Flight",
                    "visible_text": "Gate C14",
                }
            ],
        }
        self.dataset_path.write_text(json.dumps(row) + "\n", encoding="utf-8")
        environment = GRPO_ENVIRONMENT.QuipJudgeEnvironment(
            dataset_path=str(self.dataset_path), lexical_hints=True
        )

        messages = environment.build_prompt_messages(environment.dataset[0], "")
        payload = json.loads(messages[1]["content"])
        self.assertEqual(
            set(payload), {"context_snippets", "text", "lexical_hints"}
        )
        self.assertEqual(payload["context_snippets"][0]["visible_text"], "Gate C14")
        self.assertTrue(payload["lexical_hints"])


if __name__ == "__main__":
    unittest.main()
