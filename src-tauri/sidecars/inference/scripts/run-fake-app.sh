#!/bin/sh
# Launches Quip in live mode against the fake model sidecar: every draft gets
# candidates after ~600 ms, so the chunked-batch typing flow can be felt
# without mistral.rs or the Qwen download. Real path end to end — serial
# sidecar queue, off-lock inference — only the model is fake.
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../.." && pwd)
cd "$repo_root"

printf '%s\n' 'Launching Quip against the fake model sidecar (~600 ms per prediction).'

QUIP_INFERENCE_SIDECAR="$repo_root/src-tauri/sidecars/inference/scripts/fake-model-sidecar.py" \
QUIP_BACKEND_MODE=live \
QUIP_MODEL_VARIANT=base \
npm run tauri dev
