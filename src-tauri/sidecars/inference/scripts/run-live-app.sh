#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../.." && pwd)
cd "$repo_root"

export PATH="$HOME/.cargo/bin:$PATH"
mistralrs_bin=${MISTRALRS_BIN:-"$HOME/.mistralrs/mistralrs"}
model_id=${QUIP_BASE_MODEL_ID:-Qwen/Qwen3.5-2B}
model_quant=${QUIP_BASE_MODEL_QUANT:-4}
export QUIP_BASE_MODEL_ID="$model_id"
export QUIP_BASE_MODEL_QUANT="$model_quant"
server_log=$(mktemp "${TMPDIR:-/tmp}/quip-live-app-model.XXXXXX")
server_pid=

cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -f "$server_log"
}
trap cleanup EXIT HUP INT TERM

if curl -fsS http://127.0.0.1:1234/health >/dev/null 2>&1; then
  if ! curl -fsS http://127.0.0.1:1234/v1/models | grep -Fq "\"$model_id\""; then
    printf '%s\n' "A different model is already running on port 1234; stop it before loading $model_id." >&2
    exit 1
  fi
else
  if [ ! -x "$mistralrs_bin" ]; then
    printf '%s\n' "mistral.rs was not found at $mistralrs_bin" >&2
    exit 1
  fi
  printf '%s\n' "Starting local $model_id (prebuilt $model_quant-bit UQFF on Metal)..."
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

printf '%s\n' 'Building the inference sidecar...'
cargo build -p quip-inference-sidecar
printf '%s\n' 'Launching Quip with live Base inference. Use Settings to return to fixture mode.'

QUIP_INFERENCE_SIDECAR="$repo_root/target/debug/quip-inference-sidecar" \
QUIP_BACKEND_MODE=live \
QUIP_MODEL_VARIANT=base \
npm run tauri dev
