"""Map shared Phase 0 prediction fixtures into valid Flash rows."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = ROOT.parents[1]


def map_exchange(exchange: dict) -> dict | None:
    request = exchange.get("request")
    result = exchange.get("result")
    if not isinstance(request, dict) or not isinstance(result, dict):
        return None
    if result.get("status") != "ok" or request.get("model_variant") == "base":
        return None

    payload = {"text": request["draft"]}
    suggestion = next(iter(result["candidates"]), request["draft"])
    output = {"suggestion": suggestion}
    protected_tokens = []
    if exchange["case_id"] == "protected_global":
        protected_tokens = ["usr/bin", "q3_finl_v2.pdf"]
    return {
        "input": payload,
        "output": output,
        "metadata": {
            "example_id": f"phase0_{exchange['case_id']}",
            "category": exchange["case_id"],
            "target_changed": suggestion != request["draft"],
            "accepted_suggestions": [suggestion],
            "protected_tokens": protected_tokens,
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--source",
        type=Path,
        default=REPO_ROOT / "docs" / "fixtures" / "phase-0-examples.json",
    )
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    source = json.loads(args.source.read_text(encoding="utf-8"))
    rows = [
        row
        for exchange in source["prediction_exchanges"]
        if (row := map_exchange(exchange)) is not None
    ]
    rendered = "".join(json.dumps(row, separators=(",", ":"), ensure_ascii=False) + "\n" for row in rows)
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    else:
        print(rendered, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
