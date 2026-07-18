# Quip global training data contract

Status: authoritative V0 data policy
Owner: Workstream 1
Last amended: 2026-07-18

## Authority

This document is the human-readable source of truth for the Quip V0 global
training dataset. Other planning documents link here and do not restate its
quotas.

The supporting authorities have distinct roles:

| Concern | Authority |
| --- | --- |
| Product and data decisions | This document |
| Executable row quotas and validation | `training/flash/dataset_compiler/contract.py` |
| Source URL, revision, license, and hashes | `training/flash/dataset/source_manifest.json` |
| Facts about one generated build | `training/flash/dataset/build_report.json` |

If implementation differs from this document, implementation is not ready to
publish or train.

## Source

Use MASSIVE 1.1 English as the sole seed dataset. Consume full natural
utterances from its `en-US` file. Source utterances may be any length.

Use the official MASSIVE train partition for Quip training, dev for checkpoint
evaluation, and test for final comparison. Keep source records in their official
partitions.

Reject an entire source utterance before window extraction when it contains
profanity, credentials, obvious sensitive personal data, or invalid text.

## Window model

Quip corrects the current typing burst, not the full document. Accepting or
dismissing a correction closes the burst and resets its window to zero. Longer
typing continues through contiguous windows with a maximum of five words.

Randomly sample source utterances with a fixed seed. Each sampled utterance may
contribute at most one randomly positioned contiguous window. Stop sampling as
soon as the executable quotas plus a small failure buffer are full. Do not
enumerate overlapping windows from the same source utterance.

Build equal quotas for these window sizes:

| Words in window | Share of each split |
| ---: | ---: |
| 1 | 20% |
| 2 | 20% |
| 3 | 20% |
| 4 | 20% |
| 5 | 20% |

Within every window size, keep 10% of rows unchanged and apply deterministic
typing-error augmentation to the remaining 90%.

| Target behavior | Share within each window size |
| --- | ---: |
| Return the clean window unchanged | 10% |
| Correct an augmented window to its clean source | 90% |

The clean source window is always the target. Augmentation provenance records
the deterministic seed and applied operation.

## Required validation

Before publishing a dataset:

1. Rebuild it deterministically from the pinned MASSIVE source.
2. Verify exact window-size and unchanged-row quotas.
3. Verify every row comes from its intended official source partition and no
   source record is reused across splits.
4. Verify profanity and sensitive-data rejection on sampled source text and
   final rows.
5. Verify JSON shape, provenance metadata, output schema, and dataset hashes.

Only the source, window, augmentation, and validation path defined above is
active for Quip V0 global data.
