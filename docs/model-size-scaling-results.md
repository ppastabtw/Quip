# Quip V0 model-size scaling results

Date: 2026-07-18

## Method

Every model used the same published `ariobarin/quip` environment, structured
2,000-row training split, seed 42, LoRA rank 32 and alpha 64, learning rate
`1e-5`, batch size 8, and 100 training steps. Checkpoints 5, 10, 20, 30, 40,
50, 60, 70, 80, 90, and 100 were screened on the same 50-row evaluation
slice. The selected checkpoint for each model was then evaluated against all
200 test rows.

The test split SHA-256 is
`ac4f6dfd691e6d673bf2c8956d909fe6fb58024dc9646fdd7d82db67de8b1561`.

## Selected checkpoints

| Model | Run | Checkpoint | Eval success | Selection reason |
| --- | --- | ---: | ---: | --- |
| Qwen3.5-0.8B | `flash-1784407019-6244dc2d` | 80 | 64.0% | highest score |
| MiniCPM5-1B | `flash-1784407019-d7be93ce` | 80 | 58.0% | earliest point tied for highest score |
| Qwen3.5-2B | `flash-1784407019-30a5dacb` | 80 | 76.0% | earliest point on the 80, 90, and 100 plateau |
| Qwen3.5-4B | `flash-1784407019-5b6236c7` | 70 | 84.0% | earliest point on the 70, 80, and 90 plateau |
| Qwen3.5-9B | `flash-1784407450-eaec60c4` | 30 | 86.0% | highest score with zero unnecessary edits |
| Qwen3.6-35B-A3B | `flash-1784407019-15a83346` | 60 | 86.0% | earliest point on the 60 through 100 plateau |

## Test results

| Model | Base success | Tuned success | Gain | Change accuracy | Unnecessary edits | Mean latency |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Qwen3.5-0.8B | 40.5% | 68.0% | +27.5 points | 94.5% | 0.0% | 508.42 ms |
| MiniCPM5-1B | 26.5% | 53.5% | +27.0 points | 87.5% | 0.0% | 443.49 ms |
| Qwen3.5-2B | 55.0% | 73.0% | +18.0 points | 95.5% | 10.0% | 580.71 ms |
| Qwen3.5-4B | 68.0% | 84.0% | +16.0 points | 98.5% | 0.0% | 771.11 ms |
| Qwen3.5-9B | 70.0% | 82.0% | +12.0 points | 94.0% | 0.0% | 1,674.06 ms |
| Qwen3.6-35B-A3B | 69.5% | 87.5% | +18.0 points | 99.5% | 0.0% | 751.23 ms |

Every selected checkpoint produced 100% schema-valid outputs.

Qwen3.6-35B-A3B step 60 is the quality leader on this benchmark. Qwen3.5-4B
step 70 is the strongest smaller dense model. Qwen3.5-9B is slower and less
accurate than both on this managed evaluation.

The scaling experiment does not replace the existing Qwen3.5-2B step-50 Quip
V0 Base decision. The new 2B step 80 improves success from 71% to 73%, but its
unnecessary-edit rate rises from 5% to 10%.

## Published adapters and cost

| Model | Immutable adapter revision | Billed |
| --- | --- | ---: |
| Qwen3.5-0.8B | `flash-1784407019-6244dc2d@step-80.90b7bf246c10211eabe74585bdd00c284bc34798` | $0.06 |
| MiniCPM5-1B | `flash-1784407019-d7be93ce@step-80.90b7bf246c10211eabe74585bdd00c284bc34798` | $0.06 |
| Qwen3.5-2B | `flash-1784407019-30a5dacb@step-80.90b7bf246c10211eabe74585bdd00c284bc34798` | $0.09 |
| Qwen3.5-4B | `flash-1784407019-5b6236c7@step-70.90b7bf246c10211eabe74585bdd00c284bc34798` | $0.15 |
| Qwen3.5-9B | `flash-1784407450-eaec60c4@step-30.6026acfba6c0ca8a4eeb8966f9fbc5f71cd2e12c` | $0.36 |
| Qwen3.6-35B-A3B | `flash-1784407019-15a83346@step-60.6026acfba6c0ca8a4eeb8966f9fbc5f71cd2e12c` | $3.31 |
| Total | | $4.03 |

## Interpretation boundary

This benchmark is a narrow synthetic correction task and is easier than the
five-completion decision Quip makes in its UI. The next data and evaluation
iteration will add more diverse multi-error augmentation, compressed phrases,
phonetic spellings, ambiguous-label reduction, stronger split isolation, and
evaluation closer to the candidate-bar decision.

Headline success rates will probably fall as the protocol becomes harder and
more realistic. That metric reset is not automatically a model regression. The
target is better real Quip behavior and more trustworthy measurement.

## Failure inspection

Selected-checkpoint failure counts were 64 for 0.8B, 93 for MiniCPM 1B, 54 for
2B, 32 for 4B, 36 for 9B, and 25 for 35B-A3B. Remaining failures are mainly
ambiguous short windows and plausible but unaccepted substitutions. Examples
include `threw book` becoming `throw book` instead of the benchmark target
`three book`, and `sey` becoming `say` instead of `set`.

The failed pre-model-load 9B run `flash-1784407019-3c53006e` is excluded. The
partial 35B-A3B evaluation without the `-final` artifact suffix is also
excluded.

## Reproduction surfaces

- Training configs: `training/flash/configs/sft-v0-*.toml`
- Checkpoint evaluator: `training/flash/scripts/run_checkpoint_sweep.py`
- Dataset policy: `docs/training-data-contract.md`
- Dataset identity: `training/flash/dataset/build_report.json`
- Data-quality roadmap: `docs/training-data-roadmap.md`
