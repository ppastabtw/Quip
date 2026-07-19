import unittest

from lexical_candidates import (
    LexicalCandidateGenerator,
    enrich_model_input,
    weighted_damerau_levenshtein,
)


class LexicalCandidateTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.generator = LexicalCandidateGenerator(
            vocabulary=(
                "brown",
                "going",
                "quick",
                "their",
                "there",
                "tomorrow",
            )
        )

    def test_keyboard_swap_is_cheaper_than_two_edits(self):
        self.assertLess(weighted_damerau_levenshtein("teh", "the"), 1.0)

    def test_generates_ranked_dictionary_candidates(self):
        candidates = self.generator.candidates_for_token("hteir")
        self.assertIn("their", candidates)
        self.assertIn(
            "tomorrow", self.generator.candidates_for_token("tommorow")
        )

    def test_enrichment_preserves_raw_text(self):
        enriched = enrich_model_input(
            {"text": "hteir going tommorow"}, self.generator
        )
        self.assertEqual(enriched["text"], "hteir going tommorow")
        self.assertTrue(enriched["lexical_hints"])

    def test_ignores_common_words_and_protected_surfaces(self):
        hints = self.generator.hints_for_text("their q3_finl_v2.pdf /usr/bin")
        self.assertEqual(hints, [])

    def test_protected_dictionary_words_are_not_suggested(self):
        hints = self.generator.hints_for_text(
            "hteir going", protected_words=("hteir",)
        )
        self.assertEqual(hints, [])


if __name__ == "__main__":
    unittest.main()
