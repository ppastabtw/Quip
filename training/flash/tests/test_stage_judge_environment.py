import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from scripts.stage_grpo_judge_environment import SOURCE_FILES, stage_environment


class StageJudgeEnvironmentTests(unittest.TestCase):
    def test_stage_is_self_contained_and_loads_mixed_dataset(self):
        with tempfile.TemporaryDirectory() as directory:
            stage = stage_environment(Path(directory) / "stage")
            expected = {destination for _, destination in SOURCE_FILES}
            actual = {
                path.relative_to(stage).as_posix()
                for path in stage.rglob("*")
                if path.is_file()
            }
            self.assertEqual(actual, expected)
            dataset = stage / "dataset" / "train.jsonl"
            with dataset.open(encoding="utf-8") as handle:
                self.assertEqual(sum(1 for line in handle if line.strip()), 5240)

            environment = os.environ.copy()
            environment.pop("PYTHONPATH", None)
            completed = subprocess.run(
                [
                    sys.executable,
                    "-c",
                    (
                        "import environment; "
                        "loaded = environment.QuipJudgeEnvironment(); "
                        "assert len(loaded.dataset) == 5240; "
                        "assert loaded.lexical_hints is False"
                    ),
                ],
                cwd=stage,
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)


if __name__ == "__main__":
    unittest.main()
