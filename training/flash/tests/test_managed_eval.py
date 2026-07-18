import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "run_managed_eval.py"
SPEC = importlib.util.spec_from_file_location("run_managed_eval", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class ManagedEvalTests(unittest.TestCase):
    def test_request_disables_thinking_and_requires_schema(self):
        row = {"input": '{"text":"cnt cm tmrw"}'}
        payload = MODULE.request_payload(row, "Qwen/Qwen3.5-2B")
        self.assertFalse(payload["chat_template_kwargs"]["enable_thinking"])
        self.assertEqual(payload["response_format"]["type"], "json_schema")
        self.assertEqual(payload["response_format"]["json_schema"]["schema"]["required"], ["action", "candidates"])


if __name__ == "__main__":
    unittest.main()
