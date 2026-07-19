# Quip global training data V2 draft

Status: revised dry build ready for a 2B-only rerun

Owner: Workstream 1

Last reviewed: 2026-07-19

## Purpose

This document identifies the candidate V2 dataset and ranked five-completion
evaluation protocol. It does not replace the authoritative V0 policy in
`docs/training-data-contract.md`. Generated V2 rows remain in the ignored local
data cache until a representative training run is approved and evaluated.

## Dataset identity

- Policy: `massive_window_augmentation_v2_draft`
- Protocol: `v2-multi-error-ranked-five-draft`
- Seed: 42
- Source: MASSIVE 1.1 en-US, CC BY 4.0
- Rows: 2,000 train, 200 evaluation, and 200 test
- Changed ratio: 90%
- Unchanged ratio: 10%
- Window sizes: one through five clean target words, equally represented

Dry-build hashes:

| Split | SHA-256 |
| --- | --- |
| Train | `bd066bd10fe1ed2a2f0a898eb178df9c30a87bea0a4484f37ca0df99879f1e01` |
| Evaluation | `3346a885cdaa215e331121f61b36d523c106a731191111ae7d11195a14d988b3` |
| Test | `1c520d8efdf3e316aca3228770c3d5157e093ebf8d188137fb7c9edd91410177` |

## Severity policy

Changed training rows contain this exact event distribution:

| Events | Rows | Share of changed rows |
| ---: | ---: | ---: |
| 1 | 600 | 33.3% |
| 2 | 750 | 41.7% |
| 3 | 360 | 20.0% |
| 4 | 90 | 5.0% |

The mean is 1.967 events per changed row. One-word targets are capped at two
events, two-word targets at three, and longer targets at four. Every event
requires at least three clean alphabetic characters of supporting signal.
Later operations cannot revisit an earlier generated surface.

V2 uses substitution, deletion, insertion, transposition, repeat, spacing,
vowel deletion, and deterministic phonetic rewrite operations. Substitution
and insertion remain on alphabetic neighbors for alphabetic source keys.

## Families

The dry training split contains:

| Category | Rows |
| --- | ---: |
| `natural_keep` | 200 |
| `synthetic_typing_error` | 1,039 |
| `compressed_typing` | 638 |
| `phonetic_spelling` | 123 |

Rows are labeled `phonetic_spelling` when any phonetic rewrite is present,
then `compressed_typing` when spacing or vowel deletion is present. Remaining
changed rows use `synthetic_typing_error`.

## Ambiguity and isolation

V2 rejects a changed draft when every token is at or above English Zipf
frequency 3.0. The revised build no longer requires MASSIVE corpus membership
and rejects every all-common-token draft by English frequency alone. This
removes more valid-word and
all-common-token phrase collisions without pretending that frequency alone
proves grammaticality. Generated drafts with boundary or repeated whitespace
are also rejected.

Source windows now follow the product queue exactly: five-word chunks from the
start of each utterance, followed by the final one-through-four-word remainder.
The compiler no longer samples arbitrary interior windows that the runtime
would never produce.

The compiler and validator require train, evaluation, and test to be disjoint
across source families, normalized inputs, and normalized clean targets. The
dry audit reports zero overlap on every protected surface and zero conflicting
input mappings.

## Evaluation identity

The evaluation runner requests exactly five completions at temperature 0.7,
matching the product prototype. Offline evaluation shares the runtime's exact
filtering, exact-string deduplication, vote count, and first-completion
tie-break. It reports:

- vote-ranked top candidate success as `overall_success`;
- accepted candidate recall as `candidate_recall_at_5`;
- average individual completion success as `mean_completion_success`;
- candidate-bar unnecessary edit rate on unchanged examples;
- completion schema, change, and content rates.

Single-completion V1 percentages are not directly comparable to this protocol.

## Reproduction

Run through the documented Ubuntu WSL2 environment:

```text
python training/flash/scripts/build_datasets.py --offline --policy massive_window_augmentation_v2_draft --output-dir training/flash/.data-cache/builds/v2-dry
python training/flash/scripts/report_dataset_quality.py --dataset-dir training/flash/.data-cache/builds/v2-dry --protocol v2-multi-error-ranked-five-draft
python -m pytest training/flash/tests -q
```

The accepted dry build reports 68 passing tests. A second build with seed 42
must reproduce all three hashes above. Rebuilding V1 must continue to match the
three hashes in `docs/training-data-roadmap.md`.

## Known limits

- MASSIVE remains the sole clean source and is still a narrow domain for Quip.
- The vocabulary and frequency ambiguity rule is conservative, but it does not
  establish that a complete phrase is grammatical or contextually intended.
- Phonetic rewrites are a small deterministic rule set, not broad phonological
  coverage.
- Unchanged controls are generic MASSIVE text. Protected names, identifiers,
  paths, product terms, punctuation, and capitalization hard negatives remain
  unimplemented.
- Context and personalization data remain deferred until confirmed live team
  examples arrive.
- The first 2B run exposed underdetermined heavily corrupted rows and weak
  protected-text coverage. The dataset is not promoted.

## Iteration model boundary

Pipeline development is limited to Qwen 2B until the data and evaluation
protocol are finalized. Existing model-size V1 benchmark results remain
historical evidence, but no other model size receives V2 training or tuning
runs during pipeline iteration.

## First representative training result

The following result used the authoritative JSON model reply contract. The
same strict `suggestion` object is now enforced by training, evaluation, the
prototype, and live inference. This makes the adapter eligible for runtime
validation, but does not promote it automatically because its dataset still
has known quality limitations.

The candidate config is
`training/flash/configs/sft-v2-qwen-2b-representative.toml`. It uses
Qwen3.5-2B, 2,000 examples, LoRA rank 32, batch size 8, and 100 steps. The V2
environment is isolated as `ariobarin/quip-v2-draft-20260718t2050`.

Flash dry-run ID `flash-1784418431-547761a4` passed server-side schema and
environment validation. The no-submit estimate is $0.07, 6.1 billable training
minutes, and 0.23 hours total wall time on an RTX 4090.

Training run `flash-1784436250-97876093` completed all 100 steps after one
retriable provider attempt and charged $0.07. The successful attempt used an
RTX 4090 and reported 159.47 seconds of training time. Checkpoints 20, 40, 60,
80, and 100 were evaluated on all 200 evaluation examples with five
completions per example.

| Model | Checkpoint | Ranked success | Recall at five | Mean completion success | Unnecessary edits |
| --- | ---: | ---: | ---: | ---: | ---: |
| Base Qwen3.5-2B | none | 35.5% | 44.5% | 35.2% | 90.0% |
| V2 Qwen3.5-2B | 20 | 59.0% | 66.5% | 58.7% | 25.0% |
| V2 Qwen3.5-2B | 40 | 58.5% | 68.0% | 57.3% | 25.0% |
| V2 Qwen3.5-2B | 60 | 61.5% | 72.0% | 58.5% | 25.0% |
| V2 Qwen3.5-2B | 80 | 63.5% | 72.5% | 60.2% | 15.0% |
| V2 Qwen3.5-2B | 100 | 61.5% | 71.5% | 59.7% | 20.0% |

Step 80 was the provisional leader under the current reply contract, not a
promoted model. Its 31
compressed-category failures include 27 cases where none of five completions
matched the single accepted label. The audit found examples whose clean target
is an awkward source fragment or is no longer uniquely recoverable after
several mutations. Three of 20 unchanged examples also interrupted the
candidate bar, including a protected proper name and two fragment-like inputs.
The revised build aligns source windows with runtime chunking and strengthens
the common-word ambiguity rejection. First, runtime-validate the existing step
80 adapter under the restored contract. Then run one 2B-only comparison on the
revised dataset and score both models with the same five-completion evaluator.

## Runtime 10W 5K comparison

The next experiment is named `Quip V2 Runtime 10W 5K`. It uses one-through-ten
word runtime chunks and the revised ambiguity policy with 5,000 training rows,
1,000 evaluation rows, and 1,000 test rows. The Qwen3.5-2B recipe keeps seed
42, LoRA rank 32, LoRA alpha 64, batch size 8, and learning rate `1e-5`.
Training scales from 100 to 250 steps to retain the first run's
examples-per-step ratio, with checkpoints at 50, 100, 150, 200, and 250.

The dataset policy is `massive_runtime_10w_augmentation_v2_5k`, the evaluation
protocol is `v2-runtime-10w-5k-ranked-five`, and the Freesolo environment is
`ariobarin/quip-v2-runtime-10w-5k-20260719`.

The window distribution follows observed ten-word queue chunks rather than an
equal-size quota. The training quotas for one through ten words are 160, 320,
400, 560, 640, 680, 600, 440, 360, and 840. Evaluation and test each use 40,
80, 80, 120, 120, 120, 120, 80, 80, and 160. These counts preserve the exact
severity weights and reduce ambiguous one-word data while covering every
supported window size.
