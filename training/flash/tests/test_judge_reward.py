import json
import unittest

from grpo_judge.judge_reward import judge_correction, parse_judge_verdict


class JudgeRewardTests(unittest.TestCase):
    def test_parses_weighted_judge_verdict(self):
        verdict = parse_judge_verdict(
            json.dumps(
                {
                    "correction_quality": 4,
                    "meaning_preservation": 4,
                    "tone_preservation": 3,
                    "minimality": 3,
                    "acceptable": True,
                    "reason": "valid alternative",
                }
            )
        )

        self.assertTrue(verdict.acceptable)
        self.assertEqual(verdict.score, 0.925)

    def test_unacceptable_verdict_cannot_exceed_partial_credit(self):
        verdict = parse_judge_verdict(
            json.dumps(
                {
                    "correction_quality": 4,
                    "meaning_preservation": 4,
                    "tone_preservation": 4,
                    "minimality": 4,
                    "acceptable": False,
                    "reason": "meaning changed",
                }
            )
        )

        self.assertEqual(verdict.score, 0.49)

    def test_calls_freesolo_judge_with_structured_rubric(self):
        captured = {}

        def fake_generate(**kwargs):
            captured.update(kwargs)
            return json.dumps(
                {
                    "correction_quality": 4,
                    "meaning_preservation": 4,
                    "tone_preservation": 4,
                    "minimality": 4,
                    "acceptable": True,
                    "reason": "correct",
                }
            )

        verdict = judge_correction(
            input_value={"text": "meet there tmrw"},
            candidate="meet there tomorrow",
            accepted_suggestions=("meet there tomorrow",),
            model="Qwen/Qwen3.6-35B-A3B",
            api_key="test-key",
            base_url="https://example.test/v1",
            generate=fake_generate,
        )

        self.assertEqual(verdict.score, 1.0)
        self.assertEqual(captured["model"], "Qwen/Qwen3.6-35B-A3B")
        self.assertEqual(captured["api_key"], "test-key")
        self.assertEqual(captured["base_url"], "https://example.test/v1")
        self.assertEqual(captured["temperature"], 0.0)
        self.assertEqual(captured["response_format"]["type"], "json_schema")
        self.assertEqual(
            captured["extra_body"],
            {"chat_template_kwargs": {"enable_thinking": False}},
        )


if __name__ == "__main__":
    unittest.main()
