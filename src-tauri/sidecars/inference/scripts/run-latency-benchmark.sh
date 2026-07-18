#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../.." && pwd)
cd "$repo_root"

mistralrs_bin=${MISTRALRS_BIN:-"$HOME/.mistralrs/mistralrs"}
server_model=${QUIP_SERVER_MODEL:-Qwen/Qwen3.5-2B}
server_quant=${QUIP_SERVER_QUANT:-none}
server_max_seq_len=${QUIP_SERVER_MAX_SEQ_LEN:-2048}
server_ready_timeout_seconds=${QUIP_SERVER_READY_TIMEOUT_SECONDS:-600}
benchmark_port=${QUIP_BENCHMARK_PORT:-1240}
model_addr="127.0.0.1:$benchmark_port"
server_log=$(mktemp "${TMPDIR:-/tmp}/quip-latency-model.XXXXXX")
server_pid=

cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -f "$server_log"
}
trap cleanup EXIT HUP INT TERM

if curl -fsS "http://$model_addr/health" >/dev/null 2>&1; then
  printf '%s\n' "Using the existing model server at $model_addr." >&2
  printf '%s\n' 'QUIP_SERVER_MODEL only controls a server started by this script.' >&2
else
  if [ ! -x "$mistralrs_bin" ]; then
    printf '%s\n' "mistral.rs was not found at $mistralrs_bin" >&2
    exit 1
  fi

  case "$server_quant" in
    none|off|false)
      precision_label="native precision"
      printf '%s\n' "Starting $server_model ($precision_label, Metal) at $model_addr..." >&2
      "$mistralrs_bin" serve --host 127.0.0.1 -p "$benchmark_port" --no-ui --disable-access-log \
        auto -m "$server_model" --max-seq-len "$server_max_seq_len" >"$server_log" 2>&1 &
      ;;
    *)
      precision_label="$server_quant-bit"
      printf '%s\n' "Starting $server_model ($precision_label Metal) at $model_addr..." >&2
      "$mistralrs_bin" serve --host 127.0.0.1 -p "$benchmark_port" --no-ui --disable-access-log \
        auto --quant "$server_quant" -m "$server_model" --max-seq-len "$server_max_seq_len" \
        >"$server_log" 2>&1 &
      ;;
  esac
  server_pid=$!

  ready=0
  attempts=0
  while [ "$attempts" -lt "$server_ready_timeout_seconds" ]; do
    if curl -fsS "http://$model_addr/health" >/dev/null 2>&1; then
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

cargo build --quiet -p quip-inference-sidecar
case "$server_quant" in
  none|off|false) precision_label="native precision" ;;
  *) precision_label="$server_quant-bit" ;;
esac
QUIP_MODEL_ADDR="$model_addr" \
QUIP_MODEL_LABEL=${QUIP_MODEL_LABEL:-"$server_model ($precision_label)"} \
target/debug/quip-latency-tester "$@"
