import json
import unittest

from scoring import parse_prediction, score_completion


def input_json(text: str) -> str:
    return json.dumps({"text": text, "window_context_snippets": [], "personal_patterns": []})


class ParsePredictionTests(unittest.TestCase):
    def test_accepts_keep(self):
        prediction = parse_prediction('{"action":"keep","candidates":[]}')
        self.assertEqual(prediction.action, "keep")

    def test_rejects_commentary(self):
        with self.assertRaises(json.JSONDecodeError):
            parse_prediction('Keep it: {"action":"keep","candidates":[]}')

    def test_rejects_keep_with_candidate(self):
        with self.assertRaises(ValueError):
            parse_prediction('{"action":"keep","candidates":["changed"]}')

    def test_rejects_extra_property(self):
        with self.assertRaises(ValueError):
            parse_prediction('{"action":"keep","candidates":[],"why":"safe"}')


class ScoreCompletionTests(unittest.TestCase):
    def test_correct_keep_earns_full_reward(self):
        result = score_completion(
            input_text=input_json("usr/bin"),
            expected_output='{ "action": "keep", "candidates": [] }',
            metadata={"expected_action": "keep", "protected_tokens": ["usr/bin"]},
            response_text='{ "action": "keep", "candidates": [] }',
        )
        self.assertEqual(result.score, 1.0)
        self.assertTrue(result.success)

    def test_unnecessary_edit_is_severely_penalized(self):
        result = score_completion(
            input_text=input_json("q3_finl_v2.pdf"),
            expected_output='{ "action": "keep", "candidates": [] }',
            metadata={"expected_action": "keep", "protected_tokens": ["q3_finl_v2.pdf"]},
            response_text='{ "action": "replace", "candidates": ["q3_final_v2.pdf"] }',
        )
        self.assertEqual(result.score, 0.15)
        self.assertFalse(result.success)

    def test_accepted_replacement_earns_full_reward(self):
        result = score_completion(
            input_text=input_json("cnt cm tmrw"),
            expected_output='{ "action": "replace", "candidates": ["Can\'t come tomorrow."] }',
            metadata={
                "expected_action": "replace",
                "accepted_candidates": ["Can't come tomorrow."],
                "protected_tokens": [],
            },
            response_text='{ "action": "replace", "candidates": ["Can\'t come tomorrow."] }',
        )
        self.assertEqual(result.score, 1.0)
        self.assertTrue(result.success)

    def test_protected_token_change_loses_preservation_credit(self):
        result = score_completion(
            input_text=input_json("send AC742"),
            expected_output='{ "action": "replace", "candidates": ["Send AC742."] }',
            metadata={
                "expected_action": "replace",
                "accepted_candidates": ["Send AC742."],
                "protected_tokens": ["AC742"],
            },
            response_text='{ "action": "replace", "candidates": ["Send AC 742."] }',
        )
        self.assertFalse(result.protected_preserved)
        self.assertFalse(result.success)


if __name__ == "__main__":
    unittest.main()
