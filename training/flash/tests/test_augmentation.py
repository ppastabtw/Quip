import unittest

from augmentation import (
    DEFAULT_WEIGHTS,
    OPERATOR_NAMES,
    augment_text,
    normalize_weights,
    qwerty_neighbors,
)


def only(operator: str) -> dict[str, int]:
    return {name: int(name == operator) for name in OPERATOR_NAMES}


class QwertyTopologyTests(unittest.TestCase):
    def test_horizontal_neighbors_are_favored_over_vertical_and_diagonal(self):
        neighbors = qwerty_neighbors("f")

        self.assertGreater(neighbors["d"], neighbors["r"])
        self.assertGreater(neighbors["r"], neighbors["t"])
        self.assertEqual(qwerty_neighbors("F")["D"], neighbors["d"])


class AugmentationTests(unittest.TestCase):
    def test_same_seed_and_controls_reproduce_text_and_trace(self):
        first = augment_text("Meet me by the station.", seed=9402, event_count=5)
        second = augment_text("Meet me by the station.", seed=9402, event_count=5)

        self.assertEqual(first, second)
        self.assertNotEqual(first["augmented"], first["original"])
        self.assertEqual(first["applied_events"], 5)
        self.assertEqual(len(first["operations"]), 5)

    def test_each_operator_can_be_selected_when_viable(self):
        cases = {
            "substitution": "hello",
            "deletion": "hello",
            "insertion": "hello",
            "transposition": "hello",
            "repeat": "hello",
            "spacing": "hello world",
            "vowel_deletion": "hello",
            "phonetic_rewrite": "phone",
        }
        for operator, text in cases.items():
            with self.subTest(operator=operator):
                result = augment_text(text, seed=7, weights=only(operator))
                self.assertNotEqual(result["augmented"], text)
                self.assertEqual(result["operations"][0]["operator"], operator)

    def test_multiple_events_never_revisit_a_prior_surface(self):
        result = augment_text("Meet me by the station.", seed=9402, event_count=8)
        surfaces = [result["original"]]
        surfaces.extend(operation["after"] for operation in result["operations"])

        self.assertEqual(len(surfaces), len(set(surfaces)))

    def test_impossible_weighted_operator_returns_a_no_op(self):
        result = augment_text("a", seed=7, event_count=3, weights=only("transposition"))

        self.assertEqual(result["augmented"], "a")
        self.assertEqual(result["applied_events"], 0)
        self.assertEqual(result["operations"], [])

    def test_weights_are_normalized_without_changing_operator_order(self):
        normalized = normalize_weights(DEFAULT_WEIGHTS)

        self.assertEqual(tuple(normalized), OPERATOR_NAMES)
        self.assertAlmostEqual(sum(normalized.values()), 1.0)
        self.assertAlmostEqual(normalized["substitution"], 0.59)

    def test_legacy_weight_profile_defaults_new_zero_weight_operators(self):
        legacy = {
            name: weight
            for name, weight in DEFAULT_WEIGHTS.items()
            if name != "vowel_deletion"
        }

        normalized = normalize_weights(legacy)

        self.assertEqual(normalized["vowel_deletion"], 0.0)

    def test_letter_neighbor_mode_avoids_symbol_replacements(self):
        result = augment_text(
            "on my shopping",
            seed=23,
            event_count=8,
            weights=only("substitution"),
            letter_neighbors_only=True,
        )

        self.assertTrue(all(character.isalpha() or character.isspace() for character in result["augmented"]))

    def test_rejects_invalid_event_count_and_weight_profile(self):
        with self.assertRaisesRegex(ValueError, "event_count must be between"):
            augment_text("hello", seed=1, event_count=0)
        with self.assertRaisesRegex(ValueError, "at least one operator"):
            augment_text("hello", seed=1, weights={name: 0 for name in OPERATOR_NAMES})

    def test_rejects_non_finite_weights(self):
        for value in (float("nan"), float("inf"), float("-inf")):
            with self.subTest(value=value):
                weights = dict(DEFAULT_WEIGHTS)
                weights["substitution"] = value
                with self.assertRaisesRegex(ValueError, "weight substitution must be finite"):
                    normalize_weights(weights)


if __name__ == "__main__":
    unittest.main()
