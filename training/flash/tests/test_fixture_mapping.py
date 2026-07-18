import importlib.util
import json
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "map_phase0_fixtures.py"
SPEC = importlib.util.spec_from_file_location("map_phase0_fixtures", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class FixtureMappingTests(unittest.TestCase):
    def test_maps_only_successful_trained_model_exchanges(self):
        source = json.loads((ROOT.parents[1] / "docs" / "fixtures" / "phase-0-examples.json").read_text())
        rows = [
            row
            for exchange in source["prediction_exchanges"]
            if (row := MODULE.map_exchange(exchange))
        ]
        self.assertEqual(len(rows), 4)
        self.assertTrue(all(set(row) == {"input", "output", "metadata"} for row in rows))
        self.assertTrue(all("base" not in row["metadata"]["example_id"] for row in rows))
        self.assertTrue(
            all(set(json.loads(row["output"])) == {"suggestion"} for row in rows)
        )

    def test_protected_fixture_keeps_both_tokens(self):
        source = json.loads((ROOT.parents[1] / "docs" / "fixtures" / "phase-0-examples.json").read_text())
        scenario = next(
            item for item in source["prediction_exchanges"] if item["case_id"] == "protected_global"
        )
        row = MODULE.map_exchange(scenario)
        self.assertEqual(row["metadata"]["protected_tokens"], ["usr/bin", "q3_finl_v2.pdf"])


if __name__ == "__main__":
    unittest.main()
