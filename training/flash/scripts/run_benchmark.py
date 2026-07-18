"""Run a reproducible Quip benchmark through Freesolo and Backboard."""

from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from benchmarking import (  # noqa: E402
    BackboardTransport,
    FreesoloTransport,
    load_config,
    load_jsonl,
    markdown_report,
    model_runtime_summary,
    prediction_record,
    select_models,
    validate_dataset,
    validate_freesolo_models,
    write_jsonl,
)
from benchmark_dashboard import render_dashboard  # noqa: E402
from scripts.evaluate_predictions import evaluate  # noqa: E402


DEFAULT_CONFIG = ROOT / "benchmarks" / "models.toml"
DEFAULT_OUTPUT_ROOT = ROOT.parents[1] / "artifacts" / "eval"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--limit", type=int)
    parser.add_argument("--model", action="append", dest="models")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument(
        "--allow-backboard",
        action="store_true",
        help="allow Backboard catalog and inference requests",
    )
    args = parser.parse_args()
    if args.limit is not None and args.limit < 1:
        parser.error("--limit must be positive")
    return args


def main() -> int:
    args = parse_args()
    config = load_config(args.config.resolve())
    specs = select_models(config.models, args.models)
    rows = load_jsonl(config.dataset)
    if args.limit is not None:
        rows = rows[: args.limit]
    validate_dataset(rows)

    freesolo_specs = tuple(spec for spec in specs if spec.transport == "freesolo")
    backboard_specs = tuple(spec for spec in specs if spec.transport == "backboard")
    if backboard_specs and not args.allow_backboard:
        print(
            "Backboard models are selected but network access is locked. "
            "Pass --allow-backboard only after the run is approved."
        )
        return 2
    validate_freesolo_models(freesolo_specs)
    backboard = (
        BackboardTransport(timeout_seconds=config.timeout_seconds)
        if backboard_specs
        else None
    )
    try:
        if backboard is not None:
            backboard.validate_models(backboard_specs)
        request_count = len(rows) * len(specs)
        print(
            f"benchmark plan: {len(rows)} examples x {len(specs)} models = "
            f"{request_count} requests"
        )
        for spec in specs:
            provider = f"/{spec.provider}" if spec.provider else ""
            print(f"  {spec.label}: {spec.transport}{provider} {spec.model}")
        if args.dry_run:
            print("dry run complete; no inference requests sent")
            return 0

        timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        output_dir = (
            args.output_dir.resolve()
            if args.output_dir is not None
            else DEFAULT_OUTPUT_ROOT / f"benchmark-{timestamp}"
        )
        output_dir.mkdir(parents=True, exist_ok=False)
        evaluated_dataset = output_dir / "dataset.jsonl"
        write_jsonl(evaluated_dataset, rows)
        freesolo = (
            FreesoloTransport(timeout_seconds=config.timeout_seconds)
            if freesolo_specs
            else None
        )
        summary_models = []
        total_errors = 0
        try:
            for spec in specs:
                transport = freesolo if spec.transport == "freesolo" else backboard
                if transport is None:
                    raise RuntimeError(f"transport was not initialized: {spec.transport}")
                predictions = []
                print(f"running {spec.label}")
                for index, row in enumerate(rows, 1):
                    try:
                        result = transport.complete(row, spec, config.max_tokens)
                        record = prediction_record(row=row, spec=spec, result=result)
                        detail = f"{record['latency_ms']}ms"
                    except Exception as error:
                        record = prediction_record(row=row, spec=spec, error=error)
                        detail = record["error"]
                        total_errors += 1
                    predictions.append(record)
                    print(
                        f"  {index}/{len(rows)} {record['example_id']} {detail}"
                    )
                predictions_path = output_dir / f"{spec.artifact_id}.jsonl"
                write_jsonl(predictions_path, predictions)
                summary_models.append(
                    {
                        "label": spec.label,
                        "transport": spec.transport,
                        "provider": spec.provider,
                        "model": spec.model,
                        "predictions": predictions_path.name,
                        "metrics": evaluate(evaluated_dataset, predictions_path),
                        "runtime": model_runtime_summary(predictions),
                    }
                )
        finally:
            if freesolo is not None:
                freesolo.close()

        summary = {
            "created_at": datetime.now(timezone.utc).isoformat(),
            "dataset": str(config.dataset),
            "evaluated_dataset": evaluated_dataset.name,
            "examples": len(rows),
            "models": summary_models,
        }
        (output_dir / "summary.json").write_text(
            json.dumps(summary, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
            encoding="utf-8",
        )
        (output_dir / "summary.md").write_text(
            markdown_report(summary), encoding="utf-8", newline="\n"
        )
        (output_dir / "index.html").write_text(
            render_dashboard(summary), encoding="utf-8", newline="\n"
        )
        print(f"benchmark dashboard: {output_dir / 'index.html'}")
        return 1 if total_errors else 0
    finally:
        if backboard is not None:
            backboard.close()


if __name__ == "__main__":
    raise SystemExit(main())
