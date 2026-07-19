# Quip V2 base and frontier benchmark

Updated: 2026-07-19

## Outcome

The final Qwen3.5-2B adapter is the strongest product candidate in this
benchmark. It beats every Qwen base model on the complete 135-row comparison
while keeping managed latency near 1.7 seconds. Grok 4.5 is the preliminary
quality leader on the smaller shared frontier comparison, but it is much slower
and the sample is not large enough to replace the final adapter decision.

The locked test split was not used. These results come from the evaluation
split and remain selection evidence rather than final test evidence.

## Protocol

- Dataset policy: `massive_runtime_10w_augmentation_v2_5k`
- Evaluation protocol: `v2-runtime-10w-5k-ranked-five`
- Source evaluation rows: 1,000
- Source evaluation SHA-256: `2284458cc91e59700ad1b64636562bda3df26e657db8e90791594d519a264178`
- Evaluated slice: first 135 rows
- Evaluated slice SHA-256: `cb2f4e925d2f22813085fa9244f7d0b8dad65ab44ee3edea315902fd3d5386e1`
- Changed examples in the slice: 123
- Unchanged examples in the slice: 12
- Completions per example: 5
- Temperature: 0.7
- Ranking: the shared Quip runtime ranker
- Structured output: strict one-key `suggestion` object
- Thinking: disabled on all routes
- Locked test SHA-256: `df750f68b809aef2a14e5dba2d327ee3419995be388fb6e42540b41c2971db88`

The 135-row slice covers every window size from 1 through 10 and all four
dataset categories. Generated predictions and evaluation artifacts remain
local and are not committed.

## Model matrix

| Label | Transport | Provider and model |
| --- | --- | --- |
| Qwen3.5-0.8B base | Freesolo | `Qwen/Qwen3.5-0.8B` |
| Qwen3.5-2B base | Freesolo | `Qwen/Qwen3.5-2B` |
| Qwen3.5-4B base | Freesolo | `Qwen/Qwen3.5-4B` |
| Qwen3.5-9B base | Freesolo | `Qwen/Qwen3.5-9B` |
| Qwen3.6-35B-A3B base | Freesolo | `Qwen/Qwen3.6-35B-A3B` |
| Claude Fable 5 | Backboard | `anthropic/claude-fable-5` |
| GPT-5.6 Sol | Backboard | `openrouter/openai/gpt-5.6-sol` |
| Grok 4.5 | Backboard | `xai/grok-4.5` |
| GLM-5.2 | Backboard | `openrouter/z-ai/glm-5.2` |
| Quip V2 final step 250 | Freesolo | `flash-1784455014-8274f695` |

Kimi K3 was requested but excluded because the live JSON-capable catalog did
not expose an exact K3 entry. No alias or older K2 model was substituted.

## Complete Qwen and final-adapter comparison

All six Freesolo-served models completed all 135 rows, producing 675
completions each. They had no request errors and 100% schema validity.

| Model | Ranked success | Recall at five | Mean completion success | Unnecessary edits | Mean latency |
| --- | ---: | ---: | ---: | ---: | ---: |
| Quip V2 final step 250 | 80.7% | 88.9% | 81.2% | 8.3% | 1,686 ms |
| Qwen3.6-35B-A3B base | 64.4% | 70.4% | 59.0% | 66.7% | 1,694 ms |
| Qwen3.5-2B base | 40.0% | 51.8% | 35.4% | 75.0% | 1,652 ms |
| Qwen3.5-0.8B base | 31.9% | 44.4% | 28.7% | 66.7% | 1,359 ms |
| Qwen3.5-4B base | 30.4% | 51.1% | 30.1% | 83.3% | 1,680 ms |
| Qwen3.5-9B base | 25.2% | 35.6% | 22.7% | 83.3% | 2,617 ms |

The final adapter beats the strongest Qwen base by 16.3 percentage points in
ranked success and reduces unnecessary edits by 58.4 points. Raw model scale
does not produce monotonic quality on this task.

The final run completed 250 SFT steps for $0.149 and deployed immutable adapter
`flash-1784455014-8274f695@step-250.ac738095c8b864b213049df8819ee1a2908057c9`.

## Shared frontier comparison

Backboard accepted a contiguous prefix of 47 rows for every hosted model. The
table below recomputes every model on exactly that prefix, which contains 43
changed and 4 unchanged examples. Restraint percentages are especially
uncertain because only four rows test unchanged-input behavior.

| Model | Ranked success | Recall at five | Mean completion success | Unnecessary edits | Mean latency | P95 latency | Captured cost |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Grok 4.5 | 91.5% | 93.6% | 88.1% | 0.0% | 14,529 ms | 38,267 ms | $0.8400 |
| GLM-5.2 | 85.1% | 89.4% | 83.8% | 50.0% | 19,302 ms | 96,925 ms | $0.2409 |
| Claude Fable 5 | 83.0% | 87.2% | 82.6% | 0.0% | 13,580 ms | 18,729 ms | $2.0916 |
| Quip V2 final step 250 | 78.7% | 80.9% | 77.5% | 0.0% | 1,725 ms | 1,898 ms | n/a |
| GPT-5.6 Sol | 76.6% | 83.0% | 77.0% | 50.0% | 7,734 ms | 12,886 ms | $0.5238 |
| Qwen3.6-35B-A3B base | 59.6% | 68.1% | 55.7% | 50.0% | 1,753 ms | 1,946 ms | n/a |
| Qwen3.5-0.8B base | 42.6% | 48.9% | 31.5% | 50.0% | 1,393 ms | 1,513 ms | n/a |
| Qwen3.5-2B base | 38.3% | 53.2% | 37.9% | 50.0% | 1,616 ms | 1,892 ms | n/a |
| Qwen3.5-4B base | 27.7% | 51.1% | 31.1% | 50.0% | 1,704 ms | 1,952 ms | n/a |
| Qwen3.5-9B base | 21.3% | 29.8% | 20.0% | 50.0% | 2,571 ms | 3,374 ms | n/a |

Paired exact McNemar comparisons against the final adapter found:

| Frontier model | Improved rows | Regressed rows | Exact p |
| --- | ---: | ---: | ---: |
| Grok 4.5 | 6 | 0 | 0.0312 |
| GLM-5.2 | 5 | 2 | 0.4531 |
| Claude Fable 5 | 4 | 2 | 0.6875 |
| GPT-5.6 Sol | 6 | 7 | 1.0000 |

Grok is the preliminary quality leader. Its nominal paired p-value is below
0.05, but not below a four-comparison Bonferroni threshold of 0.0125. This
47-row result is directional evidence, not final model-selection proof.

## Hosted execution and cost boundary

The hosted runs produced contiguous successful prefixes of 62 rows for Claude,
90 for GPT, 60 for Grok, and 47 for GLM. Every later request returned Backboard
status `FAILED`. This is consistent with shared balance depletion after two
earlier WSL runners remained alive after their orchestration cells were
terminated. The duplicate processes were identified and stopped.

Terminal artifacts captured $5.224134 of hosted cost, and the smoke captured
$0.138697. Requests from the stopped duplicate processes were billed without
terminal artifacts, so the exact total account debit cannot be reconstructed
from local evidence. No paid retry was submitted.

## Failure inspection

Grok missed 4 of the shared 47 rows. Two outputs were plausible semantic or
grammar corrections that did not match the accepted synthetic target, one
misread a heavily corrupted music query, and one kept `spri teenty` instead of
recovering `april twenty`.

The final adapter missed 10 rows on the same prefix. Residual failures included
singularizing `revuess`, keeping `todo`, semantic substitutions such as
`weather in homestead` becoming `we are in hometown`, and incomplete recovery
of multi-error compressed phrases. These examples include both real model
errors and accepted-target ambiguity in the synthetic benchmark.

## Decision boundary

The final adapter is the best measured Quip product candidate because it
combines 80.7% success on the complete evaluation slice, low unnecessary edits,
and approximately 1.7-second managed latency. Grok merits a larger comparison
if hosted quality remains useful, but its preliminary advantage does not by
itself justify replacing the local adapter path.

Final promotion still requires the separately sealed test split and the local
macOS model-loading and latency checks. Managed-serving latency is not evidence
of local Metal performance.
