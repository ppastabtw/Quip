#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../.." && pwd)
cd "$repo_root"

export PATH="$HOME/.cargo/bin:$PATH"
. "$repo_root/src-tauri/sidecars/inference/scripts/live-model-runtime.sh"

# Match the prototype playground's prompt and diversity settings. The model
# server caches system/context layers, prefills the changing draft once, and
# forks that completed KV state into five concurrent sampling rows.
case ${QUIP_DEMO_DIVERSITY:-1} in
  1)
    export QUIP_TEMPERATURE=${QUIP_TEMPERATURE:-0.8}
    export QUIP_EARLY_EXIT_AGREEMENT=${QUIP_EARLY_EXIT_AGREEMENT:-5}
    ;;
  0)
    export QUIP_TEMPERATURE=${QUIP_TEMPERATURE:-0.1}
    export QUIP_EARLY_EXIT_AGREEMENT=${QUIP_EARLY_EXIT_AGREEMENT:-3}
    ;;
  *)
    printf '%s\n' 'QUIP_DEMO_DIVERSITY must be 0 or 1.' >&2
    exit 1
    ;;
esac
quip_live_runtime_init

cleanup() {
  quip_live_runtime_cleanup
}
trap cleanup EXIT HUP INT TERM

quip_start_live_models

printf '%s\n' 'Building the inference sidecar...'
cargo build -p quip-inference-sidecar
quip_warm_global_model "$repo_root/target/debug/quip-inference-sidecar"
printf '%s\n' 'Launching Quip with the live Global adapter. Use Settings to select Base or return to fixture mode.'

QUIP_INFERENCE_SIDECAR="$repo_root/target/debug/quip-inference-sidecar" \
QUIP_BACKEND_MODE=live \
QUIP_MODEL_VARIANT=global \
QUIP_SHOW="${QUIP_SHOW:-demo}" \
npm run tauri dev
