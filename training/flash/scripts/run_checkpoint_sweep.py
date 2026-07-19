"""Benchmark many Freesolo checkpoints on one bounded evaluation slice."""

from __future__ import annotations

import argparse
import json
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from flash.client import client_from_config


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from benchmarking import (  # noqa: E402
    FreesoloTransport,
    ModelSpec,
    load_jsonl,
    model_runtime_summary,
    prediction_record,
    validate_dataset,
    write_jsonl,
)
from scripts.evaluate_predictions import evaluate  # noqa: E402


DEFAULT_CHECKPOINTS = (5, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100)


@dataclass(frozen=True)
class Target:
    label: str
    run_id: str


def parse_target(value: str) -> Target:
    if "=" not in value:
        raise argparse.ArgumentTypeError("target must use label=run-id")
    label, run_id = (part.strip() for part in value.split("=", 1))
    if not label or not run_id.startswith("flash-"):
        raise argparse.ArgumentTypeError("target must use label=flash-run-id")
    return Target(label=label, run_id=run_id)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", action="append", type=parse_target, required=True)
    parser.add_argument(
        "--dataset", type=Path, default=ROOT / "dataset" / "eval.jsonl"
    )
    parser.add_argument("--limit", type=int, default=50)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument(
        "--checkpoints", type=int, nargs="+", default=DEFAULT_CHECKPOINTS
    )
    parser.add_argument("--deployment-timeout", type=float, default=180.0)
    args = parser.parse_args()
    if args.limit < 1:
        parser.error("--limit must be positive")
    if not args.checkpoints or any(step < 1 for step in args.checkpoints):
        parser.error("--checkpoints must contain positive steps")
    labels = [target.label for target in args.target]
    run_ids = [target.run_id for target in args.target]
    if len(labels) != len(set(labels)) or len(run_ids) != len(set(run_ids)):
        parser.error("target labels and run IDs must be unique")
    return args


def deploy(target: Target, step: int) -> None:
    read_with_retry(
        lambda: client_from_config().deploy(f"{target.run_id}/step-{step}")
    )


def read_with_retry(read, attempts: int = 3):
    for attempt in range(1, attempts + 1):
        try:
            return read()
        except Exception:
            if attempt == attempts:
                raise
            time.sleep(2.0)


def ready_run_ids(targets: list[Target], step: int) -> set[str]:
    target_ids = {target.run_id for target in targets}
    ready = set()
    client = client_from_config()
    for item in read_with_retry(client.deployments):
        deployment = item.get("deployment")
        if (
            item.get("run_id") in target_ids
            and isinstance(deployment, dict)
            and deployment.get("checkpoint_step") == step
            and deployment.get("state") == "ready"
        ):
            ready.add(item["run_id"])
    return ready


def wait_until_ready(
    targets: list[Target], step: int, timeout_seconds: float
) -> None:
    deadline = time.monotonic() + timeout_seconds
    pending = {target.run_id for target in targets}
    target_by_run = {target.run_id: target for target in targets}
    deploy_attempts = {target.run_id: 1 for target in targets}
    seen_failures: set[tuple[str, object]] = set()
    client = client_from_config()
    while pending:
        deployments = read_with_retry(client.deployments)
        for item in deployments:
            deployment = item.get("deployment")
            run_id = item.get("run_id")
            if (
                run_id in pending
                and isinstance(deployment, dict)
                and deployment.get("checkpoint_step") == step
                and deployment.get("state") == "ready"
            ):
                pending.remove(run_id)
            elif (
                run_id in pending
                and isinstance(deployment, dict)
                and deployment.get("checkpoint_step") == step
                and deployment.get("state") == "failed"
            ):
                failure = (run_id, deployment.get("updated_at"))
                if failure in seen_failures:
                    continue
                seen_failures.add(failure)
                if deploy_attempts[run_id] >= 3:
                    raise RuntimeError(
                        f"step {step} deployment failed after 3 attempts for "
                        f"{run_id}: {deployment.get('detail')}"
                    )
                client.deploy(f"{target_by_run[run_id].run_id}/step-{step}")
                deploy_attempts[run_id] += 1
        if not pending:
            return
        if time.monotonic() >= deadline:
            raise TimeoutError(
                f"step {step} deployments did not become ready: {sorted(pending)}"
            )
        time.sleep(2.0)


def benchmark_target(
    target: Target,
    step: int,
    rows: list[dict],
    dataset_path: Path,
    output_dir: Path,
) -> dict:
    spec = ModelSpec(
        label=f"{target.label}-step-{step}",
        transport="freesolo",
        model=target.run_id,
    )
    predictions = []
    transport = FreesoloTransport(timeout_seconds=180.0)
    try:
        for row in rows:
            try:
                result = transport.complete(row, spec, 128)
                record = prediction_record(row=row, spec=spec, result=result)
            except Exception as error:
                record = prediction_record(row=row, spec=spec, error=error)
            predictions.append(record)
    finally:
        transport.close()

    predictions_path = output_dir / f"{spec.artifact_id}.jsonl"
    write_jsonl(predictions_path, predictions)
    return {
        "label": target.label,
        "run_id": target.run_id,
        "checkpoint_step": step,
        "predictions": str(predictions_path.relative_to(output_dir.parent)),
        "metrics": evaluate(dataset_path, predictions_path),
        "runtime": model_runtime_summary(predictions),
    }


def metric_leader(rows: list[dict]) -> dict:
    return max(
        rows,
        key=lambda row: (
            row["metrics"]["overall_success"],
            -row["metrics"]["unnecessary_edit_rate"],
            row["metrics"]["change_accuracy"],
            -row["checkpoint_step"],
        ),
    )


def main() -> int:
    args = parse_args()
    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=False)

    rows = load_jsonl(args.dataset.resolve())[: args.limit]
    validate_dataset(rows)
    evaluated_dataset = output_dir / "dataset.jsonl"
    write_jsonl(evaluated_dataset, rows)

    targets: list[Target] = args.target
    checkpoints = sorted(set(args.checkpoints))
    control = client_from_config()
    for target in targets:
        checkpoint_records = read_with_retry(
            lambda: control.checkpoints(target.run_id)
        )
        available = {
            int(item["step"])
            for item in checkpoint_records
            if isinstance(item.get("step"), int)
        }
        missing = set(checkpoints) - available
        if missing:
            raise ValueError(
                f"{target.run_id} is missing checkpoints: {sorted(missing)}"
            )

    results: list[dict] = []
    for step in checkpoints:
        ready = ready_run_ids(targets, step)
        pending_targets = [target for target in targets if target.run_id not in ready]
        print(
            f"checkpoint step {step}: deploying {len(pending_targets)} models",
            flush=True,
        )
        if pending_targets:
            with ThreadPoolExecutor(max_workers=len(pending_targets)) as executor:
                futures = [
                    executor.submit(deploy, target, step) for target in pending_targets
                ]
                for future in as_completed(futures):
                    future.result()
        wait_until_ready(targets, step, args.deployment_timeout)

        step_dir = output_dir / f"step-{step}"
        step_dir.mkdir()
        with ThreadPoolExecutor(max_workers=len(targets)) as executor:
            future_targets = {
                executor.submit(
                    benchmark_target,
                    target,
                    step,
                    rows,
                    evaluated_dataset,
                    step_dir,
                ): target
                for target in targets
            }
            step_results = []
            for future in as_completed(future_targets):
                result = future.result()
                step_results.append(result)
                metrics = result["metrics"]
                print(
                    f"  {result['label']}: success={metrics['overall_success']:.4f} "
                    f"unnecessary={metrics['unnecessary_edit_rate']:.4f}",
                    flush=True,
                )
        results.extend(sorted(step_results, key=lambda row: row["label"]))

    leaders = {
        target.label: metric_leader(
            [row for row in results if row["label"] == target.label]
        )
        for target in targets
    }
    summary = {
        "created_at": datetime.now(timezone.utc).isoformat(),
        "dataset": str(args.dataset.resolve()),
        "evaluated_dataset": evaluated_dataset.name,
        "examples": len(rows),
        "checkpoints": checkpoints,
        "results": results,
        "metric_leaders": leaders,
    }
    (output_dir / "summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    print(f"checkpoint sweep summary: {output_dir / 'summary.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
