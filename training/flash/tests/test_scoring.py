import json
import unittest

from scoring import parse_prediction, score_completion


def input_json(text: str) -> str:
    return json.dumps({"text": text})


class ParsePredictionTests(unittest.TestCase):
    def test_accepts_single_suggestion(self):
        prediction = parse_prediction('{"suggestion":"call me"}')
        self.assertEqual(prediction.suggestion, "call me")

    def test_rejects_commentary(self):
        with self.assertRaises(json.JSONDecodeError):
            parse_prediction('Suggestion: {"suggestion":"call me"}')

    def test_rejects_empty_suggestion(self):
        with self.assertRaises(ValueError):
            parse_prediction('{"suggestion":""}')

    def test_rejects_extra_property(self):
        with self.assertRaises(ValueError):
            parse_prediction('{"suggestion":"call me","why":"safe"}')


class ScoreCompletionTests(unittest.TestCase):
    def test_correct_unchanged_suggestion_earns_full_reward(self):
        result = score_completion(
            input_text=input_json("usr/bin"),
            expected_output='{ "suggestion": "usr/bin" }',
            metadata={
                "target_changed": False,
                "accepted_suggestions": ["usr/bin"],
                "protected_tokens": ["usr/bin"],
            },
            response_text='{ "suggestion": "usr/bin" }',
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
                "protected_tokens": ["q3_finl_v2.pdf"],
            },
            response_text='{ "suggestion": "q3_final_v2.pdf" }',
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
                "protected_tokens": [],
            },
            response_text='{ "suggestion": "Can\'t come tomorrow." }',
        )
        self.assertEqual(result.score, 1.0)
        self.assertTrue(result.success)

    def test_protected_token_change_fails_success(self):
        result = score_completion(
            input_text=input_json("send AC742"),
            expected_output='{ "suggestion": "Send AC742." }',
            metadata={
                "target_changed": True,
                "accepted_suggestions": ["Send AC 742."],
                "protected_tokens": ["AC742"],
            },
            response_text='{ "suggestion": "Send AC 742." }',
        )
        self.assertFalse(result.protected_preserved)
        self.assertFalse(result.success)


if __name__ == "__main__":
    unittest.main()
