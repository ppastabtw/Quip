"""Score the live local pipeline end to end: sidecar -> model server -> filters.

Drives the real `quip-inference-sidecar --live` process over its NDJSON
protocol, so results include the sidecar's scaffolding/truncation filters and
vote ranking, then scores candidates with the shared normalization from
`scoring`. Requires the local model server on QUIP_MODEL_ADDR (default
127.0.0.1:1234).

Usage:
  python3 scripts/evaluate_live.py [--eval-sample N] [--sidecar PATH] [--json]
"""

from __future__ import annotations

import argparse
import json
import re
import statistics
import subprocess
import sys
import unicodedata
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = ROOT.parents[1]
DEFAULT_SIDECAR = REPO_ROOT / "target" / "debug" / "quip-inference-sidecar"


def _normalize(value: str) -> str:
    """Match the scoring contract without importing Freesolo-only dependencies."""
    normalized = unicodedata.normalize("NFKC", value).strip().casefold()
    return re.sub(r"\s+", " ", normalized)


# Handpicked demo-critical cases. `accepted` empty means the correct behavior
# is returning no changed suggestion (keep the draft as typed).
SMOKE_CASES = [
    ("cnt cm tmrw", ["can't come tomorrow", "can't come tomorrow."]),
    ("omw", ["on my way", "on my way!"]),
    ("cu tmrw", ["see you tomorrow"]),
    ("ttyl", ["talk to you later"]),
    ("idk yet", ["i don't know yet"]),
    ("thx sm", ["thanks so much"]),
    ("brb in 5", ["be right back in 5"]),
    ("i went to the store instaed", ["i went to the store instead"]),
    ("speker", ["speaker"]),
    ("wjat's going", ["what's going"]),
    ("q3_finl_v2.pdf", []),
    ("https://freesolo.co/docs", []),
]


@dataclass
class CaseResult:
    draft: str
    accepted: list[str]
    candidates: list[str]
    votes: list[int]
    latency_ms: int | None
    error: str | None

    @property
    def target_changed(self) -> bool:
        return bool(self.accepted)

    @property
    def success(self) -> bool:
        if self.error is not None:
            return False
        if not self.target_changed:
            return not self.candidates
        if not self.candidates:
            return False
        accepted = {_normalize(text) for text in self.accepted}
        return _normalize(self.candidates[0]) in accepted

    @property
    def any_candidate_accepted(self) -> bool:
        accepted = {_normalize(text) for text in self.accepted}
        return any(_normalize(candidate) in accepted for candidate in self.candidates)


def eval_cases(sample: int) -> list[tuple[str, list[str]]]:
    path = ROOT / "dataset" / "eval.jsonl"
    cases = []
    with path.open(encoding="utf-8") as handle:
        for line in handle:
            if not line.strip():
                continue
            row = json.loads(line)
            model_input = row["input"]
            if isinstance(model_input, str):
                model_input = json.loads(model_input)
            draft = model_input["text"]
            metadata = row["metadata"]
            accepted = metadata["accepted_suggestions"] if metadata["target_changed"] else []
            cases.append((draft, accepted))
    step = max(1, len(cases) // sample) if sample else 1
    return cases[::step][:sample]


def run_cases(sidecar: Path, cases: list[tuple[str, list[str]]]) -> list[CaseResult]:
    requests = []
    for index, (draft, _) in enumerate(cases):
        requests.append(
            json.dumps(
                {
                    "operation": "predict",
                    "request": {
                        "request_id": f"live-eval-{index}",
                        "profile_id": "profile_default",
                        "model_variant": "base",
                        "draft": draft,
                        "context_snippets": [],
                        "personal_patterns": [],
                    },
                }
            )
        )
    process = subprocess.run(
        [str(sidecar), "--live"],
        input="\n".join(requests) + "\n",
        capture_output=True,
        text=True,
        timeout=120 * max(1, len(cases)),
    )
    if process.returncode != 0:
        raise RuntimeError(f"sidecar exited {process.returncode}: {process.stderr.strip()}")
    responses = [json.loads(line) for line in process.stdout.splitlines() if line.strip()]
    if len(responses) != len(cases):
        raise RuntimeError(f"expected {len(cases)} responses, got {len(responses)}")

    results = []
    for (draft, accepted), response in zip(cases, responses):
        if response.get("status") == "ok":
            results.append(
                CaseResult(
                    draft,
                    accepted,
                    response["candidates"],
                    response["votes"],
                    response["latency_ms"],
                    None,
                )
            )
        else:
            results.append(
                CaseResult(
                    draft,
                    accepted,
                    [],
                    [],
                    None,
                    response.get("error", {}).get("code"),
                )
            )
    return results


def report(results: list[CaseResult]) -> dict:
    latencies = [float(r.latency_ms) for r in results if r.latency_ms is not None]
    changed = [r for r in results if r.target_changed]
    unchanged = [r for r in results if not r.target_changed]
    return {
        "cases": len(results),
        "overall_success": round(sum(r.success for r in results) / len(results), 4),
        "changed_top1_success": round(
            sum(r.success for r in changed) / len(changed), 4
        ) if changed else None,
        "changed_any_candidate": round(
            sum(r.any_candidate_accepted for r in changed) / len(changed), 4
        ) if changed else None,
        "unchanged_kept": round(
            sum(r.success for r in unchanged) / len(unchanged), 4
        ) if unchanged else None,
        "errors": sum(1 for r in results if r.error is not None),
        "mean_latency_ms": round(statistics.mean(latencies), 1) if latencies else None,
        "p95_latency_ms": round(
            sorted(latencies)[max(0, int(len(latencies) * 0.95) - 1)], 1
        ) if latencies else None,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--eval-sample", type=int, default=0,
                        help="also run N evenly spaced rows from dataset/eval.jsonl")
    parser.add_argument("--sidecar", type=Path, default=DEFAULT_SIDECAR)
    parser.add_argument("--json", action="store_true", help="emit the full report as JSON")
    parser.add_argument(
        "--summary-json",
        action="store_true",
        help="emit aggregate JSON without drafts, targets, or candidates",
    )
    args = parser.parse_args()

    if not args.sidecar.is_file():
        print(f"sidecar binary not found: {args.sidecar} (cargo build -p quip-inference-sidecar)",
              file=sys.stderr)
        return 1

    cases = list(SMOKE_CASES)
    if args.eval_sample:
        cases.extend(eval_cases(args.eval_sample))
    results = run_cases(args.sidecar, cases)

    summary = report(results)
    if args.summary_json:
        print(json.dumps(summary, separators=(",", ":")))
    elif args.json:
        print(json.dumps({
            "summary": summary,
            "results": [
                {
                    "draft": r.draft,
                    "accepted": r.accepted,
                    "candidates": r.candidates,
                    "votes": r.votes,
                    "latency_ms": r.latency_ms,
                    "error": r.error,
                    "success": r.success,
                }
                for r in results
            ],
        }, indent=1))
    else:
        for r in results:
            mark = "PASS" if r.success else "FAIL"
            shown = r.error or (r.candidates[0] if r.candidates else "<no suggestion>")
            print(f"{mark}  {r.draft!r} -> {shown!r}  ({r.latency_ms} ms)")
        print()
        for key, value in summary.items():
            print(f"{key}: {value}")
    return 0 if summary["errors"] == 0 else 2


if __name__ == "__main__":
    raise SystemExit(main())
