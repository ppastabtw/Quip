import unittest

from dataset_compiler.contract import CONTRACT
from environment import QuipEnvironment, load_environment
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
        self.assertIn("one full-text suggestion", messages[0]["content"])
        self.assertEqual(messages[1]["content"], example.input)

    def test_gold_output_passes_environment_reward(self):
        environment = QuipEnvironment(split="eval")
        example = environment.dataset[0]
        reward = environment.score_response(example, model_text(example.output))
        self.assertEqual(reward.score, 1.0)
        self.assertTrue(reward.success)


if __name__ == "__main__":
    unittest.main()
