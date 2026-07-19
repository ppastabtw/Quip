#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../../.." && pwd)
cd "$repo_root"

runtime_dir=${QUIP_MLX_RUNTIME_DIR:-"$repo_root/artifacts/runtime/mlx-vlm"}
runtime_python="$runtime_dir/bin/python"
global_model_preset=${QUIP_GLOBAL_MODEL_PRESET:-4b}
case "$global_model_preset" in
  4b)
    preset_source_adapter="$repo_root/artifacts/adapters/quip-qwen3.5-4b-step70"
    preset_mlx_adapter="$repo_root/artifacts/adapters/quip-qwen3.5-4b-step70-mlx"
    preset_global_model_id=mlx-community/Qwen3.5-4B-MLX-8bit
    ;;
  0.8b)
    preset_source_adapter="$repo_root/artifacts/adapters/quip-qwen3.5-0.8b-step80"
    preset_mlx_adapter="$repo_root/artifacts/adapters/quip-qwen3.5-0.8b-step80-mlx"
    preset_global_model_id=mlx-community/Qwen3.5-0.8B-MLX-8bit
    ;;
  2b)
    preset_source_adapter="$repo_root/artifacts/adapters/quip-qwen3.5-2b-step80"
    preset_mlx_adapter="$repo_root/artifacts/adapters/quip-qwen3.5-2b-step80-mlx"
    preset_global_model_id=mlx-community/Qwen3.5-2B-MLX-8bit
    ;;
  *)
    printf '%s\n' 'QUIP_GLOBAL_MODEL_PRESET must be 0.8b, 2b, or 4b.' >&2
    exit 1
    ;;
esac
source_adapter=${QUIP_GLOBAL_PEFT_ADAPTER:-$preset_source_adapter}
mlx_adapter=${QUIP_GLOBAL_ADAPTER_DIR:-$preset_mlx_adapter}
global_model_id=${QUIP_GLOBAL_MODEL_ID:-$preset_global_model_id}

if [ ! -f "$source_adapter/adapter_model.safetensors" ] || [ ! -f "$source_adapter/adapter_config.json" ]; then
  printf '%s\n' "The exported PEFT adapter is missing from $source_adapter" >&2
  exit 1
fi

if [ ! -x "$runtime_python" ]; then
  command -v uv >/dev/null 2>&1 || {
    printf '%s\n' 'uv is required to install the pinned MLX runtime.' >&2
    exit 1
  }
  uv venv --python 3.12 "$runtime_dir"
  uv pip install --python "$runtime_python" 'mlx-vlm==0.6.5'
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
