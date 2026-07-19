#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../.." && pwd)
cd "$repo_root"

.agents/skills/validate-quip-sidecar/scripts/validate.sh

. "$repo_root/src-tauri/sidecars/inference/scripts/live-model-runtime.sh"
quip_live_runtime_init
model_label=${global_model_id##*/}
responses=$(mktemp "${TMPDIR:-/tmp}/quip-live-responses.XXXXXX")
benchmark_json=$(mktemp "${TMPDIR:-/tmp}/quip-live-benchmark.XXXXXX")
benchmark_html=$(mktemp "${TMPDIR:-/tmp}/quip-live-profile.XXXXXX")
app_data=

cleanup() {
  quip_live_runtime_cleanup
  rm -f "$responses" "$benchmark_json" "$benchmark_html"
  if [ -n "$app_data" ]; then
    rm -rf -- "$app_data"
  fi
}
trap cleanup EXIT HUP INT TERM

cargo build -p quip-inference-sidecar
sidecar=target/debug/quip-inference-sidecar
phrase_tester=target/debug/quip-phrase-tester
latency_tester=target/debug/quip-latency-tester

quip_start_base_server
{
  printf '%s\n' '{"operation":"predict","request":{"request_id":"live-validation-base","profile_id":"profile_default","model_variant":"base","draft":"cnt cm tmr","context_snippets":[],"personal_patterns":[]}}'
  printf '%s\n' '{"operation":"predict","request":{"request_id":"live-validation-protected","profile_id":"profile_default","model_variant":"base","draft":"https://freesolo.co/docs","context_snippets":[],"personal_patterns":[]}}'
} | env -u QUIP_GLOBAL_MODEL_ADDR -u QUIP_GLOBAL_MODEL_ID "$sidecar" --live >"$responses"

python3 - "$responses" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
base, protected = [json.loads(line) for line in path.read_text().splitlines() if line]
assert base["request_id"] == "live-validation-base", base
assert base["status"] == "ok", base
assert base["model_variant"] == "base", base
assert base["backend"] == "live", base
assert "action" not in base, base
assert isinstance(base["latency_ms"], int) and base["latency_ms"] > 0, base
assert 0 <= len(base["candidates"]) <= 5, base
assert len(base["candidates"]) == len(set(base["candidates"])), base
assert "cnt cm tmr" not in base["candidates"], base
assert protected["request_id"] == "live-validation-protected", protected
assert protected["status"] == "ok", protected
assert protected["backend"] == "live", protected
assert "action" not in protected, protected
assert 0 <= len(protected["candidates"]) <= 5, protected
assert len(protected["candidates"]) == len(set(protected["candidates"])), protected
assert "https://freesolo.co/docs" not in protected["candidates"], protected
for response in (base, protected):
    print(json.dumps(response, separators=(",", ":")))
PY

quip_stop_base_server
quip_start_global_server
QUIP_DEMO_WARMUP_RUNS=1 quip_warm_global_model "$sidecar"
{
  printf '%s\n' '{"operation":"health"}'
  printf '%s\n' "{\"operation\":\"predict\",\"request\":{\"request_id\":\"live-validation-global\",\"profile_id\":\"profile_default\",\"model_variant\":\"global\",\"draft\":\"$global_validation_draft\",\"context_snippets\":[],\"personal_patterns\":[]}}"
  printf '%s\n' '{"operation":"predict","request":{"request_id":"live-validation-personal","profile_id":"profile_default","model_variant":"global_plus_personal","draft":"cnt cm tmr","context_snippets":[],"personal_patterns":[]}}'
} | "$sidecar" --live >"$responses"

python3 - "$responses" "$global_validation_draft" "$global_validation_expected" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
draft = sys.argv[2]
expected = sys.argv[3]
responses = [json.loads(line) for line in path.read_text().splitlines() if line]
assert len(responses) == 3, responses

health, global_result, personal_result = responses
assert health == {
    "status": "ready",
    "fixture_available": True,
    "loaded": {"base": False, "global_adapter": True, "user_adapter": False},
}, health
assert global_result["request_id"] == "live-validation-global", global_result
assert global_result["status"] == "ok", global_result
assert global_result["model_variant"] == "global", global_result
assert global_result["backend"] == "live", global_result
assert "action" not in global_result, global_result
assert isinstance(global_result["latency_ms"], int) and global_result["latency_ms"] > 0, global_result
assert 0 <= len(global_result["candidates"]) <= 5, global_result
assert len(global_result["candidates"]) == len(set(global_result["candidates"])), global_result
assert expected.casefold() in [candidate.casefold() for candidate in global_result["candidates"]], global_result
assert draft not in global_result["candidates"], global_result
assert personal_result["request_id"] == "live-validation-personal", personal_result
assert personal_result["status"] == "error", personal_result
assert personal_result["error"]["code"] == "adapter_not_loaded", personal_result

for response in responses:
    print(json.dumps(response, separators=(",", ":")))
PY

phrase_output=$("$phrase_tester" --live "$global_validation_draft")
case "$phrase_output" in
  *"Live local inference — $model_label (Metal)."*"Base: error (live_inference_failed)"*"Global: "*"[Live, "*"$global_validation_expected"*) ;;
  *)
    printf '%s\n' "$phrase_output" >&2
    printf '%s\n' 'Live phrase tester output failed' >&2
    exit 1
    ;;
esac
printf '%s\n' "$phrase_output"

"$latency_tester" --address "$global_model_addr" --model-id "$global_model_id" \
  --output-contract "$global_output_contract" \
  --label "validation-qwen" --warmup 1 --runs 2 \
  --phrase "cnt cm tmr" --html "$benchmark_html" --json >"$benchmark_json"
python3 - "$benchmark_json" "$benchmark_html" <<'PY'
import json
import pathlib
import sys

report = json.loads(pathlib.Path(sys.argv[1]).read_text())
profile_html = pathlib.Path(sys.argv[2]).read_text()
assert report["model_label"] == "validation-qwen", report
assert report["output_contract"] in {"plain_text", "json_suggestion"}, report
assert report["warmup_runs"] == 1, report
assert report["measured_runs"] == 2, report
assert len(report["samples"]) == 2, report
assert all("phrase" not in sample and "candidates" not in sample for sample in report["samples"]), report
assert all(sample["timing"]["backend_total_us"] > 0 for sample in report["samples"]), report
profiled_completions = [
    completion
    for sample in report["samples"]
    for completion in sample["timing"]["completions"]
    if completion["tokens"] is not None
]
if profiled_completions:
    assert all(
        completion["tokens"]["completion_tokens"] > 0
        for completion in profiled_completions
    ), report
    assert report["token_summary"]["mean_completion_tokens"] > 0, report
    assert report["token_summary"]["mean_server_ms_per_output_token"] > 0, report
else:
    # MLX-VLM's streaming responses currently omit OpenAI usage data. The
    # transport and end-to-end stage timings remain real and required below.
    assert report["config"]["streaming"] is True, report
    assert report["token_summary"] is None, report
assert "validation-qwen" in profile_html, profile_html[:500]
assert "Latency decomposition" in profile_html, profile_html[:500]
stages = {(item["scope"], item["stage"]): item for item in report["summary"]}
for key in [
    ("inference", "sidecar_round_trip"),
    ("inference", "backend_total"),
    ("inference", "completion_batch"),
    ("inference", "normalization_ranking"),
    ("inference", "sidecar_protocol_process"),
    ("completion", "server_wait_ttfb"),
    ("completion", "response_decode"),
]:
    assert key in stages, (key, stages)
assert stages[("inference", "backend_total")]["median_ms"] > 0, stages
assert stages[("completion", "server_wait_ttfb")]["median_ms"] > 0, stages
print(
    "Live latency benchmark: "
    f"backend median {stages[('inference', 'backend_total')]['median_ms']:.3f} ms; "
    f"TTFB median {stages[('completion', 'server_wait_ttfb')]['median_ms']:.3f} ms"
)
PY

cargo build -p quip
app_data=$(mktemp -d "${TMPDIR:-/tmp}/quip-live-app.XXXXXX")
if ! app_output=$(env \
  QUIP_DATA_DIR="$app_data" \
  QUIP_INFERENCE_SIDECAR="$repo_root/$sidecar" \
  QUIP_BACKEND_MODE=live \
  QUIP_MODEL_VARIANT=global \
  QUIP_SELFTEST_LIVE=1 \
  "$repo_root/target/debug/quip" 2>&1); then
  printf '%s\n' "$app_output" >&2
  printf '%s\n' 'Live Tauri app self-test failed' >&2
  exit 1
fi
require_app_output() {
  case "$app_output" in
    *"$1"*) ;;
    *)
      printf '%s\n' "$app_output" >&2
      printf '%s\n' "Live Tauri app self-test output was missing: $1" >&2
      exit 1
      ;;
  esac
}

require_app_output 'LIVE SELFTEST ok: sidecar health is ready with global model artifacts loaded'
require_app_output 'LIVE SELFTEST ok: app rendered live candidates'
require_app_output 'LIVE SELFTEST ok: app metrics recorded one schema-valid live result'
require_app_output 'LIVE SELFTEST PASS'

case "$app_output" in
  *"LIVE SELFTEST FAIL"*)
    printf '%s\n' "$app_output" >&2
    printf '%s\n' 'Live Tauri app self-test reported a failure' >&2
    exit 1
    ;;
  *) ;;
esac
printf '%s\n' "$app_output"

printf '%s\n' 'Quip live inference integration passed'
