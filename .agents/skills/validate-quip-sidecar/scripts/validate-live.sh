#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../.." && pwd)
cd "$repo_root"

.agents/skills/validate-quip-sidecar/scripts/validate.sh

mistralrs_bin=${MISTRALRS_BIN:-"$HOME/.mistralrs/mistralrs"}
model_id=${QUIP_BASE_MODEL_ID:-Qwen/Qwen3.5-2B}
model_quant=${QUIP_BASE_MODEL_QUANT:-4}
model_label=${model_id#Qwen/}
export QUIP_BASE_MODEL_ID="$model_id"
export QUIP_BASE_MODEL_QUANT="$model_quant"
server_log=$(mktemp "${TMPDIR:-/tmp}/quip-live-model.XXXXXX")
responses=$(mktemp "${TMPDIR:-/tmp}/quip-live-responses.XXXXXX")
benchmark_json=$(mktemp "${TMPDIR:-/tmp}/quip-live-benchmark.XXXXXX")
benchmark_html=$(mktemp "${TMPDIR:-/tmp}/quip-live-profile.XXXXXX")
server_pid=
app_data=

cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -f "$server_log" "$responses" "$benchmark_json" "$benchmark_html"
  if [ -n "$app_data" ]; then
    rm -rf -- "$app_data"
  fi
}
trap cleanup EXIT HUP INT TERM

if curl -fsS http://127.0.0.1:1234/health >/dev/null 2>&1; then
  if ! curl -fsS http://127.0.0.1:1234/v1/models | grep -Fq "\"$model_id\""; then
    printf '%s\n' "A different model is already running on port 1234; expected $model_id." >&2
    exit 1
  fi
else
  if [ ! -x "$mistralrs_bin" ]; then
    printf '%s\n' "mistral.rs was not found at $mistralrs_bin" >&2
    exit 1
  fi
  "$mistralrs_bin" serve --host 127.0.0.1 -p 1234 --no-ui --disable-access-log \
    auto --quant "$model_quant" -m "$model_id" --max-seq-len 2048 >"$server_log" 2>&1 &
  server_pid=$!

  ready=0
  attempts=0
  while [ "$attempts" -lt 120 ]; do
    if curl -fsS http://127.0.0.1:1234/health >/dev/null 2>&1; then
      ready=1
      break
    fi
    if ! kill -0 "$server_pid" 2>/dev/null; then
      break
    fi
    attempts=$((attempts + 1))
    sleep 1
  done
  if [ "$ready" -ne 1 ]; then
    tail -n 40 "$server_log" >&2
    exit 1
  fi
fi

cargo build -p quip-inference-sidecar
sidecar=target/debug/quip-inference-sidecar
phrase_tester=target/debug/quip-phrase-tester
latency_tester=target/debug/quip-latency-tester

{
  printf '%s\n' '{"operation":"health"}'
  printf '%s\n' '{"operation":"predict","request":{"request_id":"live-validation-base","profile_id":"profile_default","model_variant":"base","draft":"cnt cm tmr","context_snippets":[],"personal_patterns":[]}}'
  printf '%s\n' '{"operation":"predict","request":{"request_id":"live-validation-protected","profile_id":"profile_default","model_variant":"base","draft":"https://freesolo.co/docs","context_snippets":[],"personal_patterns":[]}}'
  printf '%s\n' '{"operation":"predict","request":{"request_id":"live-validation-global","profile_id":"profile_default","model_variant":"global","draft":"cnt cm tmr","context_snippets":[],"personal_patterns":[]}}'
} | "$sidecar" --live >"$responses"

python3 - "$responses" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
responses = [json.loads(line) for line in path.read_text().splitlines() if line]
assert len(responses) == 4, responses

health, base, protected, global_result = responses
assert health == {
    "status": "ready",
    "fixture_available": True,
    "loaded": {"base": True, "global_adapter": False, "user_adapter": False},
}, health
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
assert global_result["request_id"] == "live-validation-global", global_result
assert global_result["status"] == "error", global_result
assert global_result["error"]["code"] == "adapter_not_loaded", global_result

for response in responses:
    print(json.dumps(response, separators=(",", ":")))
PY

phrase_output=$("$phrase_tester" --live "cnt cm tmr")
case "$phrase_output" in
  *"Live local inference — $model_label (4-bit, Metal)."*"Base: "*"[Live, "*"Global: error (adapter_not_loaded)"*) ;;
  *)
    printf '%s\n' "$phrase_output" >&2
    printf '%s\n' 'Live phrase tester output failed' >&2
    exit 1
    ;;
esac
printf '%s\n' "$phrase_output"

"$latency_tester" --label "validation-qwen" --warmup 1 --runs 2 \
  --phrase "cnt cm tmr" --html "$benchmark_html" --json >"$benchmark_json"
python3 - "$benchmark_json" "$benchmark_html" <<'PY'
import json
import pathlib
import sys

report = json.loads(pathlib.Path(sys.argv[1]).read_text())
profile_html = pathlib.Path(sys.argv[2]).read_text()
assert report["model_label"] == "validation-qwen", report
assert report["warmup_runs"] == 1, report
assert report["measured_runs"] == 2, report
assert len(report["samples"]) == 2, report
assert all("phrase" not in sample and "candidates" not in sample for sample in report["samples"]), report
assert all(sample["timing"]["backend_total_us"] > 0 for sample in report["samples"]), report
assert all(
    completion["tokens"]["completion_tokens"] > 0
    for sample in report["samples"]
    for completion in sample["timing"]["completions"]
), report
assert report["token_summary"]["mean_completion_tokens"] > 0, report
assert report["token_summary"]["mean_server_ms_per_output_token"] > 0, report
assert report["token_summary"]["mean_completion_ms_per_token"] > 0, report
assert report["token_summary"]["mean_prompt_prefill_ms"] > 0, report
assert report["token_summary"]["mean_completion_decode_ms"] > 0, report
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
  QUIP_MODEL_VARIANT=base \
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

require_app_output 'LIVE SELFTEST ok: sidecar health is ready with base Qwen loaded'
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
