"""Run a base model or deployed adapter against a Quip dataset through Freesolo."""

from __future__ import annotations

import argparse
import json
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import httpx
from flash.client.config import load_credentials
from flash.serve.deploy import serving_openai_base_url


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from environment import SYSTEM_PROMPT  # noqa: E402
from scoring import OUTPUT_RESPONSE_FORMAT, model_text  # noqa: E402


COMPLETION_COUNT = 5
PRODUCT_TEMPERATURE = 0.7


def load_jsonl(path: Path) -> list[dict]:
    with path.open(encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def request_payload(row: dict, model: str) -> dict:
    return {
        "model": model,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": model_text(row["input"])},
        ],
        "temperature": PRODUCT_TEMPERATURE,
        "max_tokens": 128,
        "response_format": OUTPUT_RESPONSE_FORMAT,
        "chat_template_kwargs": {"enable_thinking": False},
    }


def run(dataset: Path, output: Path, model: str, limit: int | None = None) -> int:
    _, api_key = load_credentials()
    if not api_key:
        raise RuntimeError("Flash login is required before managed evaluation")

    rows = load_jsonl(dataset)
    if limit is not None:
        rows = rows[:limit]
    output.parent.mkdir(parents=True, exist_ok=True)

    headers = {"Authorization": f"Bearer {api_key}", "Content-Type": "application/json"}
    with httpx.Client(base_url=serving_openai_base_url(), headers=headers, timeout=180.0) as client:
        with output.open("w", encoding="utf-8", newline="\n") as handle:
            for index, row in enumerate(rows, 1):
                started = time.perf_counter()
                payload = request_payload(row, model)

                def complete() -> str:
                    response = client.post("/chat/completions", json=payload)
                    response.raise_for_status()
                    body = response.json()
                    content = body["choices"][0]["message"]["content"]
                    if not isinstance(content, str):
                        raise ValueError("serving response content must be a string")
                    return content

                with ThreadPoolExecutor(max_workers=COMPLETION_COUNT) as pool:
                    responses = list(pool.map(lambda _: complete(), range(COMPLETION_COUNT)))
                prediction = {
                    "example_id": row["metadata"]["example_id"],
                    "responses": responses,
                    "latency_ms": round((time.perf_counter() - started) * 1000),
                    "model": model,
                }
                handle.write(json.dumps(prediction, ensure_ascii=False) + "\n")
                print(f"{index}/{len(rows)} {prediction['example_id']} {prediction['latency_ms']}ms")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default="Qwen/Qwen3.5-2B")
    parser.add_argument("--dataset", type=Path, default=ROOT / "dataset" / "eval.jsonl")
    parser.add_argument(
        "--output",
        type=Path,
        default=ROOT.parents[1] / "artifacts" / "eval" / "base-qwen-2b.jsonl",
    )
    parser.add_argument("--limit", type=int)
    args = parser.parse_args()
    return run(args.dataset, args.output, args.model, args.limit)


if __name__ == "__main__":
    raise SystemExit(main())
