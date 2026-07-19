# Quip GRPO checkpoint trials

Date: 2026-07-19

## Question

Does a short warm-started GRPO continuation improve held-out Quip correction
quality over its source SFT checkpoint?

## Trial contract

- Base model: `Qwen/Qwen3.5-2B`
- SFT source run: `flash-1784436250-97876093`
- Source checkpoints: steps 40, 80, and 100
- Algorithm: GRPO
- Training examples: 200
- Training steps: 10
- Batch size: 8 prompts
- Group size: 8 completions per prompt
- Temperature: 1.0
- Structured output: strict `suggestion` JSON object
- Reward: existing dense deterministic Quip reward
- Evaluation: the same 200 held-out V2 examples for every model, five
  completions per example at temperature 0.7

The first checkpoint sweep used only the deterministic reward. The later
judge-backed lane uses Freesolo-managed Qwen3.6-35B-A3B serving and does not
require an OpenRouter secret.

## Managed runs

| Source | GRPO run | Result | Cost |
| --- | --- | --- | ---: |
| SFT step 40 | `flash-1784445486-fa199e58` | step 10 completed | $0.02095 |
| SFT step 80 | `flash-1784445488-9fee0443` | step 10 completed | $0.02095 |
| SFT step 100 | `flash-1784445493-ad3865ff` | step 10 completed after one managed infrastructure retry | $0.02095 |

Total training cost was approximately $0.06285.

## Held-out results

| Source | Overall success | Recall at five | Mean completion success | Unnecessary edits |
| --- | ---: | ---: | ---: | ---: |
| Step 40 SFT | 57.5% | 67.0% | 55.8% | 30.0% |
| Step 40 plus GRPO | 58.5% | 69.0% | 56.8% | 20.0% |
| Step 80 SFT | 61.5% | 71.0% | 59.6% | 20.0% |
| Step 80 plus GRPO | 61.5% | 71.5% | 58.7% | 15.0% |
| Step 100 SFT | 62.0% | 70.5% | 59.3% | 15.0% |
| Step 100 plus GRPO | 61.5% | 72.0% | 59.9% | 15.0% |

Paired held-out flips were:

- Step 40: 9 examples improved and 7 regressed, exact McNemar p = 0.8036.
- Step 80: 11 examples improved and 11 regressed, exact McNemar p = 1.0.
- Step 100: 8 examples improved and 9 regressed, exact McNemar p = 1.0.

## Decision

The short deterministic-reward GRPO pass did not establish a reliable overall
quality improvement. It improved restraint for the step 40 and step 80 starts,
but correction gains and regressions balanced out. Do not promote these trial
adapters over the SFT checkpoints.

The next justified RL experiment is the prepared LLM-judge GRPO lane, using a
selected new SFT checkpoint and the same source-blind held-out evaluation.

The three trial adapters were undeployed after evaluation. The original SFT
run was restored to step 80.

## Judge-backed pilot

The judge-backed lane was validated with a 25-step pilot before scaling it.

| Field | Value |
| --- | --- |
| Run | `flash-1784449568-455d5c42` |
| Warm start | `flash-1784446044-7bc2f567/step-50` |
| Training rows available | 5,000 |
| Training horizon | 25 steps, 8 unique prompts per step |
| Samples per prompt | 8 |
| Saved checkpoints | 5, 10, and 25 |
| Training cost | $0.0549 |
| Training wall time | 295.5 seconds |

The reward first applies strict deterministic gates. Exact accepted
suggestions receive 1.0. Invalid JSON and incorrect change decisions receive
only the deterministic partial reward. A valid, non-exact suggestion with the
correct change decision is scored by Freesolo-managed
`Qwen/Qwen3.6-35B-A3B` on correction quality, meaning preservation, tone
preservation, and minimality. The four judge dimensions have weights 0.40,
0.30, 0.15, and 0.15. An unacceptable verdict is capped at 0.49. The final
judge-backed reward is `min(0.99, 0.40 + 0.59 * judge_score)`.

The pilot completed without judge, schema, or deployment failure. Its recorded
mean reward rose from 0.7039 on the first logged update to 0.9144 at step 25,
with nonzero within-group reward standard deviation. This proves that the
training loop received a usable preference signal. It does not prove held-out
improvement. A planned 1,000-row sweep was stopped before producing a result so
the team could proceed directly to the consolidated run. The pilot adapter was
undeployed.

## Consolidated 5K judge GRPO run

The source SFT checkpoint was selected from complete 1,000-row evaluations
under the five-completion ranked-candidate protocol:

| SFT checkpoint | Overall success | Recall at five | Mean completion success | Unnecessary edits |
| --- | ---: | ---: | ---: | ---: |
| Step 50 | 71.5% | 80.0% | 69.4% | 5.0% |
| Step 100 | 75.9% | 81.7% | 73.3% | 7.0% |
| Step 150 | 76.9% | 82.8% | 75.5% | 8.0% |

Step 150 was the best fully evaluated checkpoint available at launch. The
consolidated run therefore uses `flash-1784446044-7bc2f567/step-150` as its
warm start.

| Field | Value |
| --- | --- |
| Run | `flash-1784451945-4745fb47` |
| Config | `training/flash/configs/grpo-v2-runtime-5k-judge-mega.toml` |
| Environment | `ariobarin/quip-v2-runtime-10w-5k-grpo-judge-20260719` |
| Training horizon | 625 steps, one pass over all 5,000 prompts |
| Samples per prompt | 8 |
| Total generated candidates | 40,000 |
| Saved checkpoints | 50, 100, 250, 400, and 625 |
| Estimated training cost | $1.02 |
| Estimated wall time | 1.19 hours |

This run consolidates the finalized correction prompt, strict suggestion JSON,
the runtime-weighted 5K dataset, the strongest measured SFT warm start, exact
and change-decision gates, and the 35B semantic judge into one training lane.
Checkpoint selection must still use held-out evaluation. Training reward alone
will not be used to promote the final adapter.
