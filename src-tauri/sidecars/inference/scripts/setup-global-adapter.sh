#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../.." && pwd)
cd "$repo_root"

runtime_dir=${QUIP_MLX_RUNTIME_DIR:-"$repo_root/artifacts/runtime/mlx-vlm"}
runtime_python="$runtime_dir/bin/python"
global_model_preset=${QUIP_GLOBAL_MODEL_PRESET:-2b}
case "$global_model_preset" in
  2b)
    preset_adapter_repo=ppasta/quip-v2-context-mega
    preset_adapter_revision=297ac9b68fce60ff34a9d415dea7d0376441e9a0
    preset_source_adapter="$repo_root/artifacts/adapters/quip-v2-context-mega"
    preset_mlx_adapter="$repo_root/artifacts/adapters/quip-v2-context-mega-mlx"
    preset_global_model_id=mlx-community/Qwen3.5-2B-MLX-4bit
    ;;
  *)
    printf '%s\n' 'Quip is locked to QUIP_GLOBAL_MODEL_PRESET=2b.' >&2
    exit 1
    ;;
esac
source_adapter=${QUIP_GLOBAL_PEFT_ADAPTER:-$preset_source_adapter}
mlx_adapter=${QUIP_GLOBAL_ADAPTER_DIR:-$preset_mlx_adapter}
global_model_id=${QUIP_GLOBAL_MODEL_ID:-$preset_global_model_id}
adapter_repo=${QUIP_GLOBAL_ADAPTER_REPO:-$preset_adapter_repo}
adapter_revision=${QUIP_GLOBAL_ADAPTER_REVISION:-$preset_adapter_revision}

if [ ! -x "$runtime_python" ]; then
  command -v uv >/dev/null 2>&1 || {
    printf '%s\n' 'uv is required to install the pinned MLX runtime.' >&2
    exit 1
  }
  uv venv --python 3.12 "$runtime_dir"
  uv pip install --python "$runtime_python" 'mlx-vlm==0.6.5'
fi

if [ ! -f "$source_adapter/adapter_model.safetensors" ] || [ ! -f "$source_adapter/adapter_config.json" ]; then
  "$runtime_python" - "$adapter_repo" "$adapter_revision" "$source_adapter" <<'PY'
import sys
from huggingface_hub import snapshot_download

repo_id, revision, local_dir = sys.argv[1:]
path = snapshot_download(repo_id=repo_id, revision=revision, local_dir=local_dir)
print(f"Downloaded Global PEFT adapter: {repo_id}@{revision} ({path})")
PY
fi

"$runtime_python" \
  src-tauri/sidecars/inference/scripts/convert-peft-adapter-to-mlx.py \
  "$source_adapter" "$mlx_adapter"

"$runtime_python" - "$global_model_id" <<'PY'
import sys
from huggingface_hub import snapshot_download

model_id = sys.argv[1]
path = snapshot_download(model_id)
print(f"MLX base model ready: {model_id} ({path})")
PY

printf '%s\n' "Global adapter ready: $mlx_adapter"
