#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../.." && pwd)
cd "$repo_root"

. "$repo_root/src-tauri/sidecars/inference/scripts/live-model-runtime.sh"
quip_live_runtime_init

cleanup() {
  quip_live_runtime_cleanup
}
trap cleanup EXIT HUP INT TERM

quip_start_live_models

printf '%s\n' 'Local Global adapter server is ready. Set QUIP_START_BASE_MODEL=1 for a simultaneous Base comparison.'
cargo run --quiet -p quip-inference-sidecar --bin quip-phrase-tester -- --live "$@"
