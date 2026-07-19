#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../.." && pwd)
cd "$repo_root"

runs=${QUIP_COMPARE_RUNS:-9}
warmups=${QUIP_COMPARE_WARMUPS:-3}
eval_sample=${QUIP_COMPARE_EVAL_SAMPLE:-20}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --runs)
      runs=$2
      shift 2
      ;;
    --warmups)
      warmups=$2
      shift 2
      ;;
    --eval-sample)
      eval_sample=$2
      shift 2
      ;;
    --help|-h)
      printf '%s\n' 'Usage: compare-global-models.sh [--runs N] [--warmups N] [--eval-sample N]'
      exit 0
      ;;
    *)
      printf '%s\n' "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

case "$runs:$warmups:$eval_sample" in
  *[!0-9:]*|0:*|*::*|*:)
    printf '%s\n' 'Runs must be positive; warmups and eval sample must be non-negative integers.' >&2
    exit 1
    ;;
esac

comparison_dir=$(mktemp -d "${TMPDIR:-/tmp}/quip-model-comparison.XXXXXX")
cleanup() {
  rm -rf "$comparison_dir"
}
trap cleanup EXIT HUP INT TERM

printf '%s\n' 'Building the inference tools once...' >&2
cargo build --quiet -p quip-inference-sidecar

for preset in 0.8b 2b 4b; do
  (
    export QUIP_GLOBAL_MODEL_PRESET=$preset
    export QUIP_GLOBAL_MODEL_ADDR=127.0.0.1:1235
    export QUIP_APC_ENABLED=1
    export QUIP_STREAMING=true
    export QUIP_EARLY_EXIT_AGREEMENT=3

    . "$repo_root/src-tauri/sidecars/inference/scripts/live-model-runtime.sh"
    quip_live_runtime_init
    trap quip_live_runtime_cleanup EXIT HUP INT TERM

    printf '%s\n' "Testing $preset with APC, streaming, and 3-vote early exit..." >&2
    quip_start_global_server >&2

    "$repo_root/target/debug/quip-latency-tester" \
      --address "$global_model_addr" \
      --model-id "$global_model_id" \
      --output-contract "$global_output_contract" \
      --label "$preset" \
      --runs "$runs" \
      --warmup "$warmups" \
      --completions 5 \
      --streaming \
      --early-exit-agreement 3 \
      --phrase 'cancel next meetihng' \
      --phrase 'is the commute ttime to' \
      --phrase 'contropversy' \
      --json >"$comparison_dir/$preset-latency.json"

    QUIP_MODEL_ADDR="$global_model_addr" \
      QUIP_MODEL_ID="$global_model_id" \
      QUIP_MODEL_OUTPUT_CONTRACT="$global_output_contract" \
      python3 "$repo_root/training/flash/scripts/evaluate_live.py" \
        --eval-sample "$eval_sample" \
        --sidecar "$repo_root/target/debug/quip-inference-sidecar" \
        --summary-json >"$comparison_dir/$preset-eval.json"
  )
done

python3 - "$comparison_dir" "$runs" "$warmups" "$eval_sample" <<'PY'
import json
import sys
from pathlib import Path

directory = Path(sys.argv[1])
runs, warmups, eval_sample = sys.argv[2:]

print("Quip tuned-model comparison")
print(f"Settings: APC on; streaming on; early exit at 3/5 votes; {warmups} warmups; {runs} measured latency runs; {eval_sample} eval rows plus smoke cases.")
print()
print("| Model | Contract | Median | p95 | Eval success | Changed top-1 | Unchanged kept |")
print("| --- | --- | ---: | ---: | ---: | ---: | ---: |")
for preset in ("0.8b", "2b", "4b"):
    latency = json.loads((directory / f"{preset}-latency.json").read_text())
    evaluation = json.loads((directory / f"{preset}-eval.json").read_text())
    round_trip = next(
        row for row in latency["summary"]
        if row["scope"] == "inference" and row["stage"] == "sidecar_round_trip"
    )
    percent = lambda value: "n/a" if value is None else f"{value * 100:.1f}%"
    print(
        f"| {preset} | {latency['output_contract']} | "
        f"{round_trip['median_ms']:.0f} ms | {round_trip['p95_ms']:.0f} ms | "
        f"{percent(evaluation['overall_success'])} | "
        f"{percent(evaluation['changed_top1_success'])} | "
        f"{percent(evaluation['unchanged_kept'])} |"
    )
PY
