#!/usr/bin/env python3
"""Convert a Hugging Face PEFT LoRA into MLX-VLM's adapter format."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import mlx.core as mx


PEFT_PREFIX = "base_model.model.model."
LANGUAGE_PREFIX = "language_model."
MLX_LANGUAGE_PREFIX = "language_model.model."
LORA_SUFFIXES = {
    ".lora_A.weight": ".lora_a",
    ".lora_B.weight": ".lora_b",
}


def convert_key(key: str) -> tuple[str, str]:
    if not key.startswith(PEFT_PREFIX):
        raise ValueError(f"unsupported PEFT tensor prefix: {key}")
    body = key.removeprefix(PEFT_PREFIX)
    if not body.startswith(LANGUAGE_PREFIX):
        raise ValueError(f"adapter tensor is outside the language model: {key}")
    body = MLX_LANGUAGE_PREFIX + body.removeprefix(LANGUAGE_PREFIX)
    for peft_suffix, mlx_suffix in LORA_SUFFIXES.items():
        if body.endswith(peft_suffix):
            module = body.removesuffix(peft_suffix)
            return module + mlx_suffix, module
    raise ValueError(f"unsupported PEFT LoRA tensor name: {key}")


def convert(source: Path, destination: Path) -> None:
    source_config = json.loads((source / "adapter_config.json").read_text())
    if source_config.get("peft_type") != "LORA":
        raise ValueError("adapter_config.json does not describe a LoRA")
    rank = source_config.get("r")
    alpha = source_config.get("lora_alpha")
    dropout = source_config.get("lora_dropout", 0.0)
    if not isinstance(rank, int) or rank <= 0:
        raise ValueError("adapter rank must be a positive integer")
    if not isinstance(alpha, (int, float)) or alpha <= 0:
        raise ValueError("adapter alpha must be positive")

    source_weights = mx.load(str(source / "adapter_model.safetensors"))
    converted: dict[str, mx.array] = {}
    modules: set[str] = set()
    module_parts: dict[str, set[str]] = {}
    skipped_visual = 0
    for key, tensor in source_weights.items():
        if key.startswith(PEFT_PREFIX + "visual."):
            # Freesolo targeted all matching module names, including Qwen's
            # vision tower, but text-only SFT never ran that tower. Its LoRA B
            # matrices therefore remain exactly zero and contribute no delta.
            if key.endswith(".lora_B.weight") and bool(mx.any(tensor != 0).item()):
                raise ValueError("vision-tower LoRA contains trained weights")
            skipped_visual += 1
            continue
        converted_key, module = convert_key(key)
        converted[converted_key] = tensor.T
        modules.add(module)
        module_parts.setdefault(module, set()).add(converted_key.rsplit(".", 1)[-1])

    incomplete = sorted(
        module for module, parts in module_parts.items() if parts != {"lora_a", "lora_b"}
    )
    if incomplete:
        raise ValueError(f"LoRA modules are missing A/B tensor pairs: {incomplete[:5]}")
    if len(converted) + skipped_visual != len(source_weights):
        raise ValueError("adapter conversion did not account for every tensor")

    destination.mkdir(parents=True, exist_ok=True)
    mx.save_safetensors(str(destination / "adapters.safetensors"), converted)
    mlx_config = {
        "fine_tune_type": "lora",
        "num_layers": -1,
        "lora_parameters": {
            "rank": rank,
            "dropout": float(dropout),
            "scale": float(alpha) / rank,
            "keys": sorted(modules),
        },
        "source_format": "huggingface_peft",
        "source_base_model": source_config.get("base_model_name_or_path"),
        "omitted_zero_delta_visual_tensors": skipped_visual,
    }
    (destination / "adapter_config.json").write_text(
        json.dumps(mlx_config, indent=2, sort_keys=True) + "\n"
    )
    print(
        f"Converted {len(modules)} language LoRA modules / {len(converted)} tensors "
        f"to {destination}; omitted {skipped_visual} zero-delta vision tensors"
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    convert(args.source.resolve(), args.destination.resolve())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
