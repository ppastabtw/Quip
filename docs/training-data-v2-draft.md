# Quip global training data V2 draft

Status: validated dry build, representative training awaiting approval

Owner: Workstream 1

Last reviewed: 2026-07-18

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
| Train | `93cab0f3cc7f85ee53773a24292c386f237acd9eb2c32dd5c3169c2939862385` |
| Evaluation | `082a8d79a98f8d7f81bb5c4ca8c6808b3d9bcd304c4f2fd38995a2324038c2f1` |
| Test | `b7324c2004d73efbf9e61006dd8540d62574f0653a16f23a091a42c0e08d09fc` |

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
| `synthetic_typing_error` | 1,026 |
| `compressed_typing` | 665 |
| `phonetic_spelling` | 109 |

Rows are labeled `phonetic_spelling` when any phonetic rewrite is present,
then `compressed_typing` when spacing or vowel deletion is present. Remaining
changed rows use `synthetic_typing_error`.

## Ambiguity and isolation

V2 rejects a changed draft when every token is both present in the clean
MASSIVE vocabulary and at or above English Zipf frequency 3.0. This removes
common valid-word and all-common-token phrase collisions without pretending
that frequency alone proves grammaticality. Generated drafts with boundary or
repeated whitespace are also rejected.

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

The accepted dry build reports 66 passing tests. A second build with seed 42
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
- No V2 model has been trained or evaluated. The dataset is not promoted.

## Representative training gate

The candidate config is
`training/flash/configs/sft-v2-qwen-2b-representative.toml`. It uses
Qwen3.5-2B, 2,000 examples, LoRA rank 32, batch size 8, and 100 steps. The V2
environment is isolated as `ariobarin/quip-v2-draft-20260718t2050`.

Flash dry-run ID `flash-1784418431-547761a4` passed server-side schema and
environment validation. The no-submit estimate is $0.07, 6.1 billable training
minutes, and 0.23 hours total wall time on an RTX 4090. No training submission
has been authorized or made.
