import json
import unittest

from scoring import parse_gold_output, parse_prediction, score_completion


def input_json(text: str) -> str:
    return json.dumps({"text": text})


class ParsePredictionTests(unittest.TestCase):
    def test_accepts_plain_text_suggestion(self):
        prediction = parse_prediction("call me")
        self.assertEqual(prediction.suggestion, "call me")

    def test_rejects_json_wrapped_suggestion(self):
        with self.assertRaises(ValueError):
            parse_prediction('{"suggestion":"call me"}')

    def test_rejects_scaffolding_label(self):
        with self.assertRaises(ValueError):
            parse_prediction("Suggestion: call me")

    def test_rejects_empty_reply(self):
        with self.assertRaises(ValueError):
            parse_prediction("   ")


class ParseGoldOutputTests(unittest.TestCase):
    def test_accepts_json_gold_output(self):
        prediction = parse_gold_output('{"suggestion":"call me"}')
        self.assertEqual(prediction.suggestion, "call me")

    def test_rejects_extra_property(self):
        with self.assertRaises(ValueError):
            parse_gold_output('{"suggestion":"call me","why":"safe"}')


class ScoreCompletionTests(unittest.TestCase):
    def test_contextual_input_uses_same_reward_contract(self):
        result = score_completion(
            input_text={
                "text": "meet there tmrw",
                "context_snippets": [
                    {
                        "app_name": "Notes",
                        "window_title": "Trip planning",
                        "visible_text": "Tomorrow: meet at Union Station.",
                    }
                ],
            },
            expected_output={"suggestion": "meet at Union Station tomorrow"},
            metadata={
                "target_changed": True,
                "accepted_suggestions": ["meet at Union Station tomorrow"],
            },
            response_text="meet at Union Station tomorrow",
        )
        self.assertTrue(result.success)
        self.assertEqual(result.score, 1.0)

    def test_contextual_input_rejects_malformed_snippets(self):
        result = score_completion(
            input_text={"text": "hi", "context_snippets": [{"visible_text": "x"}]},
            expected_output={"suggestion": "hi"},
            metadata={"target_changed": False, "accepted_suggestions": ["hi"]},
            response_text="hi",
        )
        self.assertFalse(result.schema_valid)

    def test_correct_unchanged_suggestion_earns_full_reward(self):
        result = score_completion(
            input_text=input_json("usr/bin"),
            expected_output='{ "suggestion": "usr/bin" }',
            metadata={
                "target_changed": False,
                "accepted_suggestions": ["usr/bin"],
            },
            response_text="usr/bin",
        )
        self.assertEqual(result.score, 1.0)
        self.assertTrue(result.success)

    def test_unnecessary_edit_is_severely_penalized(self):
        result = score_completion(
            input_text=input_json("q3_finl_v2.pdf"),
            expected_output='{ "suggestion": "q3_finl_v2.pdf" }',
            metadata={
                "target_changed": False,
                "accepted_suggestions": ["q3_finl_v2.pdf"],
            },
            response_text="q3_final_v2.pdf",
        )
        self.assertEqual(result.score, 0.15)
        self.assertFalse(result.success)

    def test_accepted_correction_earns_full_reward(self):
        result = score_completion(
            input_text=input_json("cnt cm tmrw"),
            expected_output='{ "suggestion": "Can\'t come tomorrow." }',
            metadata={
                "target_changed": True,
                "accepted_suggestions": ["Can't come tomorrow."],
            },
            response_text="Can't come tomorrow.",
        )
        self.assertEqual(result.score, 1.0)
        self.assertTrue(result.success)

if __name__ == "__main__":
    unittest.main()
