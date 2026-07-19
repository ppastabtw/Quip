#!/bin/sh

# Sourced by the live launchers and validator. The caller must set repo_root.

quip_live_runtime_init() {
  : "${repo_root:?repo_root must be set before sourcing live-model-runtime.sh}"

  base_model_id=${QUIP_BASE_MODEL_ID:-mlx-community/Qwen3.5-4B-MLX-8bit}
  base_model_addr=${QUIP_MODEL_ADDR:-127.0.0.1:1234}

  global_model_preset=${QUIP_GLOBAL_MODEL_PRESET:-4b}
  case "$global_model_preset" in
    4b)
      preset_global_model_id=mlx-community/Qwen3.5-4B-MLX-8bit
      preset_global_adapter_dir="$repo_root/artifacts/adapters/quip-qwen3.5-4b-step70-mlx"
      preset_global_output_contract=plain_text
      global_validation_draft=contropversy
      global_validation_expected=controversy
      ;;
    0.8b)
      preset_global_model_id=mlx-community/Qwen3.5-0.8B-MLX-8bit
      preset_global_adapter_dir="$repo_root/artifacts/adapters/quip-qwen3.5-0.8b-step80-mlx"
      preset_global_output_contract=json_suggestion
      global_validation_draft='cancel next meetihng'
      global_validation_expected='cancel next meeting'
      ;;
    2b)
      preset_global_model_id=mlx-community/Qwen3.5-2B-MLX-8bit
      preset_global_adapter_dir="$repo_root/artifacts/adapters/quip-qwen3.5-2b-step80-mlx"
      preset_global_output_contract=json_suggestion
      global_validation_draft='is the commute ttime to'
      global_validation_expected='is the commute time to'
      ;;
    *)
      printf '%s\n' 'QUIP_GLOBAL_MODEL_PRESET must be 0.8b, 2b, or 4b.' >&2
      return 1
      ;;
  esac

  global_model_id=${QUIP_GLOBAL_MODEL_ID:-$preset_global_model_id}
  global_model_addr=${QUIP_GLOBAL_MODEL_ADDR:-127.0.0.1:1235}
  global_adapter_dir=${QUIP_GLOBAL_ADAPTER_DIR:-$preset_global_adapter_dir}
  global_output_contract=${QUIP_GLOBAL_OUTPUT_CONTRACT:-$preset_global_output_contract}
  mlx_python=${QUIP_MLX_PYTHON:-"$repo_root/artifacts/runtime/mlx-vlm/bin/python"}
  apc_enabled=${QUIP_APC_ENABLED:-1}
  live_streaming=${QUIP_STREAMING:-true}
  early_exit_agreement=${QUIP_EARLY_EXIT_AGREEMENT:-3}

  base_server_log=$(mktemp "${TMPDIR:-/tmp}/quip-base-model.XXXXXX")
  global_server_log=$(mktemp "${TMPDIR:-/tmp}/quip-global-model.XXXXXX")
  base_server_pid=
  global_server_pid=

  export QUIP_BASE_MODEL_ID="$base_model_id"
  export QUIP_MODEL_ADDR="$base_model_addr"
  export QUIP_MODEL_ID="$base_model_id"
  export QUIP_GLOBAL_MODEL_ADDR="$global_model_addr"
  export QUIP_GLOBAL_MODEL_ID="$global_model_id"
  export QUIP_GLOBAL_ADAPTER_DIR="$global_adapter_dir"
  export QUIP_GLOBAL_OUTPUT_CONTRACT="$global_output_contract"
  export APC_ENABLED="$apc_enabled"
  export QUIP_STREAMING="$live_streaming"
  export QUIP_EARLY_EXIT_AGREEMENT="$early_exit_agreement"
}

quip_live_runtime_cleanup() {
  quip_stop_base_server
  quip_stop_global_server
  rm -f "${base_server_log:-}" "${global_server_log:-}"
}

quip_stop_base_server() {
  if [ -n "${base_server_pid:-}" ]; then
    kill "$base_server_pid" 2>/dev/null || true
    wait "$base_server_pid" 2>/dev/null || true
    base_server_pid=
  fi
}

quip_stop_global_server() {
  if [ -n "${global_server_pid:-}" ]; then
    kill "$global_server_pid" 2>/dev/null || true
    wait "$global_server_pid" 2>/dev/null || true
    global_server_pid=
  fi
}

quip_wait_for_server() {
  server_name=$1
  server_addr=$2
  server_pid=$3
  server_log=$4
  attempts=0
  while [ "$attempts" -lt 180 ]; do
    if curl -fsS "http://$server_addr/health" >/dev/null 2>&1; then
      return 0
    fi
    if ! kill -0 "$server_pid" 2>/dev/null; then
      break
    fi
    attempts=$((attempts + 1))
    sleep 1
  done
  printf '%s\n' "$server_name did not become ready:" >&2
  tail -n 60 "$server_log" >&2
  return 1
}

quip_start_base_server() {
  if curl -fsS "http://$base_model_addr/health" >/dev/null 2>&1; then
    base_health=$(curl -fsS "http://$base_model_addr/health")
    if ! printf '%s' "$base_health" | grep -Fq "\"loaded_model\":\"$base_model_id\"" || \
       ! printf '%s' "$base_health" | grep -Fq '"loaded_adapter":null'; then
      printf '%s\n' "A different Base model is running at $base_model_addr; expected $base_model_id." >&2
      return 1
    fi
    return 0
  fi
  if [ ! -x "$mlx_python" ]; then
    printf '%s\n' 'The local MLX runtime is missing.' >&2
    printf '%s\n' 'Run src-tauri/sidecars/inference/scripts/setup-global-adapter.sh first.' >&2
    return 1
  fi

  base_host=${base_model_addr%:*}
  base_port=${base_model_addr##*:}
  printf '%s\n' "Starting local Base $base_model_id on Metal..."
  "$mlx_python" -m mlx_vlm.server \
    --model "$base_model_id" \
    --host "$base_host" \
    --port "$base_port" \
    >"$base_server_log" 2>&1 &
  base_server_pid=$!
  quip_wait_for_server "Base model server" "$base_model_addr" "$base_server_pid" "$base_server_log"
}

quip_start_global_server() {
  if curl -fsS "http://$global_model_addr/health" >/dev/null 2>&1; then
    global_health=$(curl -fsS "http://$global_model_addr/health")
    if ! printf '%s' "$global_health" | grep -Fq "\"loaded_model\":\"$global_model_id\"" || \
       ! printf '%s' "$global_health" | grep -Fq "\"loaded_adapter\":\"$global_adapter_dir\""; then
      printf '%s\n' "A different Global model or adapter is running at $global_model_addr." >&2
      return 1
    fi
    return 0
  fi
  if [ ! -x "$mlx_python" ] || [ ! -f "$global_adapter_dir/adapters.safetensors" ]; then
    printf '%s\n' 'The local MLX runtime or converted Global adapter is missing.' >&2
    printf '%s\n' 'Run src-tauri/sidecars/inference/scripts/setup-global-adapter.sh first.' >&2
    return 1
  fi

  global_host=${global_model_addr%:*}
  global_port=${global_model_addr##*:}
  printf '%s\n' "Starting local Global $global_model_id with the $global_model_preset Quip adapter..."
  "$mlx_python" -m mlx_vlm.server \
    --model "$global_model_id" \
    --adapter-path "$global_adapter_dir" \
    --host "$global_host" \
    --port "$global_port" \
    >"$global_server_log" 2>&1 &
  global_server_pid=$!
  quip_wait_for_server "Global adapter server" "$global_model_addr" "$global_server_pid" "$global_server_log"
}

quip_start_live_models() {
  quip_start_global_server
  if [ "${QUIP_START_BASE_MODEL:-0}" = "1" ]; then
    quip_start_base_server
  fi
}

quip_warm_global_model() {
  sidecar_bin=$1
  warmup_runs=${QUIP_DEMO_WARMUP_RUNS:-3}
  if [ "$warmup_runs" -eq 0 ]; then
    return 0
  fi
  if [ ! -x "$sidecar_bin" ]; then
    printf '%s\n' "The inference sidecar is missing from $sidecar_bin" >&2
    return 1
  fi

  printf '%s\n' "Warming the $global_model_preset Global model ($warmup_runs requests)..."
  warmup_index=0
  while [ "$warmup_index" -lt "$warmup_runs" ]; do
    printf '{"operation":"predict","request":{"request_id":"demo-warmup-%s","profile_id":"profile_default","model_variant":"base","draft":"%s","context_snippets":[],"personal_patterns":[]}}\n' \
      "$warmup_index" "$global_validation_draft"
    warmup_index=$((warmup_index + 1))
  done | QUIP_MODEL_ADDR="$global_model_addr" \
    QUIP_MODEL_ID="$global_model_id" \
    QUIP_MODEL_OUTPUT_CONTRACT="$global_output_contract" \
    "$sidecar_bin" --live >/dev/null
}
