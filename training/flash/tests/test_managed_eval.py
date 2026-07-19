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
    def test_request_disables_thinking_and_uses_plain_text_replies(self):
        row = {"input": '{"text":"cnt cm tmrw"}'}
        payload = MODULE.request_payload(row, "Qwen/Qwen3.5-2B")
        self.assertFalse(payload["chat_template_kwargs"]["enable_thinking"])
        self.assertNotIn("response_format", payload)
        self.assertEqual(payload["temperature"], 0.7)


if __name__ == "__main__":
    unittest.main()
