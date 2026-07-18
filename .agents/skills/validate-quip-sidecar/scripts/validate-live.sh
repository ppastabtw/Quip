#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../.." && pwd)
cd "$repo_root"

.agents/skills/validate-quip-sidecar/scripts/validate.sh

mistralrs_bin=${MISTRALRS_BIN:-"$HOME/.mistralrs/mistralrs"}
server_log=$(mktemp "${TMPDIR:-/tmp}/quip-live-model.XXXXXX")
responses=$(mktemp "${TMPDIR:-/tmp}/quip-live-responses.XXXXXX")
server_pid=
app_data=

cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -f "$server_log" "$responses"
  if [ -n "$app_data" ]; then
    rm -rf -- "$app_data"
  fi
}
trap cleanup EXIT HUP INT TERM

if ! curl -fsS http://127.0.0.1:1234/health >/dev/null 2>&1; then
  if [ ! -x "$mistralrs_bin" ]; then
    printf '%s\n' "mistral.rs was not found at $mistralrs_bin" >&2
    exit 1
  fi
  "$mistralrs_bin" serve --host 127.0.0.1 -p 1234 --no-ui --disable-access-log \
    auto --quant 4 -m Qwen/Qwen3.5-2B --max-seq-len 2048 >"$server_log" 2>&1 &
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
  *"Live local inference — Qwen3.5-2B (4-bit, Metal)."*"Base: "*"[Live, "*"Global: error (adapter_not_loaded)"*) ;;
  *)
    printf '%s\n' "$phrase_output" >&2
    printf '%s\n' 'Live phrase tester output failed' >&2
    exit 1
    ;;
esac
printf '%s\n' "$phrase_output"

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
