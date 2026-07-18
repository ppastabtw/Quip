# Quip training data quality roadmap

Status: live execution roadmap, not dataset policy

Owner: Workstream 1

Last reviewed: 2026-07-18

## Purpose

Quip is not intended to be only a spell checker. Its useful model behavior is
conservative correction of ordinary mistakes plus recovery of compressed,
phonetic, and personalized English while preserving intentional text.

This document turns the current data-quality review into small, independently
testable changes. It tracks hypotheses, decisions, evidence, and remaining
work. The active dataset policy remains `docs/training-data-contract.md` until
an item here is implemented, validated, and deliberately promoted into that
contract.

## Current evidence baseline

- The current corpus contains 2,000 training rows, 200 evaluation rows, and 200
  test rows from MASSIVE 1.1 English.
- Each window size from one through five words has an equal quota.
- Within each window size, 90% of rows contain one deterministic QWERTY
  mutation and 10% remain unchanged.
- The current categories are `qwerty_typo` and `natural_keep`.
- All six model-size runs finished 100 training steps and evaluated 11
  checkpoints each. The detailed evidence is in
  `docs/model-size-scaling-results.md`.
- Qwen3.6-35B-A3B step 60 is the managed-serving quality leader on the current
  synthetic benchmark at 87.5% success. Qwen3.5-4B step 70 is the strongest
  compact dense result at 84.0%.
- The existing Qwen3.5-2B V0 Base remains
  `flash-1784405174-22c9eb92/step-50`. The size sweep does not replace that
  product decision. Product may later retain 2B, evaluate 4B locally, or
  investigate the 35B-A3B footprint.
- Current split hashes are train
  `b4531ca8b8fd74c801a5ae406b9d694155b2d96c5ce875d2315d7b590f02c960`,
  evaluation
  `74f6fae94adf4f7ced38d99f8362f300b421681dab3a5144a008109f5b4c0af1`,
  and test
  `ac4f6dfd691e6d673bf2c8956d909fe6fb58024dc9646fdd7d82db67de8b1561`.
- These percentages measure a narrow synthetic correction task. They are not
  estimates of end-to-end Quip product quality.

## Decisions already made

1. Keep the 90% changed and 10% unchanged training ratio for the current
   iteration. Record it as a future calibration question once real interaction
   telemetry exists.
2. Defer training on window context and personal patterns until the team
   supplies confirmed live examples. Do not invent a large context-training
   scheme before that data arrives.
3. Continue using MASSIVE as the current clean-text seed while investigating
   additional sources and synthetic phrase families incrementally.
4. Prefer one validated increase in complexity at a time. Use compiler checks,
   dataset inspection, and a small representative model before another full
   model-size sweep.
5. Treat lower future headline success as expected when the benchmark becomes
   harder and more realistic. Compare models within a versioned protocol and
   do not present scores from different protocol versions as directly
   comparable.
6. The hackathon benchmark may evolve. Preserve each evaluated dataset and
   protocol identity so results remain auditable, without blocking progress on
   the idea of a permanently locked test set.

## Work sequence

### Phase 0: preserve and diagnose the current baseline

Do not change generated rows in this phase.

- Preserve the current dataset hashes, selected checkpoints, costs, metrics,
  inspected failures, and excluded artifacts.
- Add a repeatable diagnostic report for mutation counts, mutation severity,
  window sizes, ambiguous valid-word inputs, punctuation, capitalization,
  source domains, and cross-split surface overlap.
- Record the current benchmark as protocol `v1-single-qwerty`.

Exit evidence:

- One command reproduces the diagnostic report from checked-in JSONL.
- The report matches the current build report and does not mutate datasets.

### Phase 1: support multiple realistic error events

Expand augmentation without adding new semantic source families yet.

- Support one through several mutation events per changed row.
- Make the event-count distribution configurable and deterministic. Start with
  a bounded distribution centered on a small number of events, then choose its
  exact shape from inspected output rather than assuming a normal distribution
  is automatically realistic.
- Add missing-space, extra-space, dropped-vowel, repeated-character,
  transposition, insertion, deletion, and neighboring-key behavior.
- Prevent later operations from silently undoing earlier operations.
- Track requested events, applied events, effective character edit distance,
  and operation sequence in provenance.
- Keep the existing single-mutation path available as a comparison and
  rollback surface.

Exit evidence:

- Determinism tests cover every operator and mixed sequences.
- Dataset validation proves the configured severity distribution.
- Manual inspection accepts a stratified sample from every severity level.
- A small representative-model run beats or usefully complements the current
  baseline on a matching multi-error development set before any full sweep.

### Phase 2: remove ambiguous and unsafe labels

Reduce examples that teach confident edits where the draft may already be
valid.

- Detect augmented one-word inputs that are common valid English words.
- Detect changed phrases whose corrupted input is also a plausible clean
  phrase.
- Reject or separately label examples such as `on` to `in`, `he` to `her`, and
  `sing` to `song` when context does not disambiguate them.
- Do not use one lexical-frequency threshold as proof of correctness. Combine
  dictionary or frequency checks with source context and targeted inspection.
- Add hard unchanged examples containing names, abbreviations, paths,
  filenames, identifiers, product terms, punctuation, and capitalization.
- Preserve privacy filtering by using synthetic protected strings rather than
  real credentials or personal data.

Exit evidence:

- The diagnostic report shows the ambiguous-input rejection count and sampled
  reasons.
- Protected-text examples survive unchanged through dataset validation and a
  representative model evaluation.
- No newly accepted row relies on an unreviewed real secret or personal value.

### Phase 3: add Quip-specific correction families

Add one family at a time with separate provenance and metrics.

Candidate families:

1. Compressed phrases such as dropped vowels and common chat shorthand.
2. Phonetic spellings and sound-alike constructions.
3. Multi-token abbreviations and number substitutions.
4. Informal chat, email, Notes, and browser-form writing.
5. Minimal punctuation and capitalization repair where the intended change is
   unambiguous.

Candidate acquisition routes:

- Deterministic transforms over licensed clean text.
- Small, manually reviewed synthetic templates that resemble the supported
  product surfaces.
- Public correction or typo corpora after license, provenance, and task-fit
  review.
- Confirmed team interactions once collection and labeling are available.

Each family needs its own unchanged controls and adversarial cases. It must not
be merged into one generic `typo` category.

Exit evidence for each family:

- Source and license are recorded.
- Generation rules are deterministic or the exact generated artifact is
  pinned.
- At least one accepted and one rejected example are documented for every
  transform.
- Category-specific metrics show whether the family helped without hiding
  regressions in ordinary text.

### Phase 4: strengthen split isolation and benchmark size

- Make normalized inputs globally disjoint across train, evaluation, and test.
- Detect exact clean-target overlap across splits and either prevent it or
  explicitly justify unavoidable short targets.
- Keep source records, templates, and augmentation families together when
  splitting so close variants cannot cross partitions.
- Expand evaluation and test counts enough to report useful category and
  unchanged-text rates. Size them from desired confidence and category coverage,
  not merely because augmentation can generate arbitrary volume.
- Version each dataset build and preserve its report, hashes, and protocol
  name.

Exit evidence:

- The validator fails on injected cross-split input, target, source-family, and
  template-family leakage.
- Every reported metric includes its denominator and dataset protocol version.

### Phase 5: evaluate the actual five-completion decision

Retain the single-completion score as a cheap diagnostic, then add product-level
evaluation matching the candidate bar.

- Generate exactly five completions using the same prompt, temperature, schema,
  filtering, deduplication, and ranking rules as the runtime.
- Report top-1 success, accepted-candidate recall within five, ranked candidate
  success, and bar-level unnecessary-interruption rate.
- Report pass-at-5 only with a precise definition. Also evaluate the actual
  vote-ranked result rather than averaging five binary scores that the UI never
  uses.
- Preserve per-completion outputs so disagreement and candidate diversity can
  be inspected.
- Compare single-completion and five-completion results before deciding which
  metric controls checkpoint selection.

Exit evidence:

- A shared fixture produces the same filtering and ranking in offline
  evaluation and the inference sidecar.
- The report distinguishes model correction failure from aggregation failure.

### Phase 6: integrate confirmed context and personalization data

This phase waits for labeled team interactions.

- Define the minimum confirmed example schema for draft, optional bounded
  context, optional personal pattern, selected or dismissed candidate, and
  resulting text.
- Exclude ambient window text unless it is necessary for the confirmed label.
- Create paired examples where the same draft has different correct behavior
  with and without context or under two profiles.
- Keep profile training separate from the global evaluation set.

Exit evidence:

- Data owners approve the schema and privacy boundary.
- Paired context and profile tests demonstrate the intended behavioral change.

## Documentation reconciliation

The current checkout is behind `origin/main`, and completed training work is
still uncommitted. Do not merge or rewrite that work as part of this roadmap.

The following conflict has a likely direction but is not fully resolved across
the real product surfaces:

- `docs/SPEC.md` still mentions an 80-character draft window from commit
  `324c63c`.
- The newer training contract added a five-word window in commit `5dcbc61`.
- The still newer demo commit `4d5a419` implements queued five-word batches in
  `src/ui/demo.ts`.
- The Accessibility path on current `origin/main` still bounds drafts to 80
  characters in `src-tauri/src/accessibility/mod.rs`.

Current history indicates that five-word queued batches are the intended
direction, but demo behavior is not proof that the real Accessibility path uses
that boundary. After the worktree is safely integrated, obtain one observed app
result from the owning workstream. Then align the implementation and smallest
authoritative product document, removing the losing path, stale comments, and
duplicated planning text together.

## Reporting rules

- Never compare percentages across protocol versions without labeling both
  versions.
- Lead with category denominators and bar-level safety behavior, not one
  aggregate score.
- Preserve failed and rejected hypotheses in the ledger.
- Record actual artifacts and commands as evidence. A completed training run is
  not by itself proof of product improvement.
- Update the ledger after every material experiment, implementation, or user
  decision.

## Live ledger

| ID | Item | Current status | Evidence required to close |
| --- | --- | --- | --- |
| DQ-001 | Preserve current model-size and dataset baseline | done | `docs/model-size-scaling-results.md` and current build hashes |
| DQ-002 | Keep the 90% changed and 10% unchanged ratio for now | done | Decision recorded; reopen only with interaction telemetry or a user decision |
| DQ-003 | Defer context and personalization training | needs external data | Confirmed team interaction examples and approved schema |
| DQ-004 | Add a non-mutating dataset diagnostic report | done | `training/flash/scripts/report_dataset_quality.py`; current build matches; 52 tests pass |
| DQ-005 | Add deterministic multi-event augmentation | todo | Operator tests, severity quotas, inspected samples, and representative-model evidence |
| DQ-006 | Add spacing and dropped-vowel transformations | todo | Validated category rows and targeted metrics |
| DQ-007 | Filter ambiguous valid-word corrections | todo | Rejection diagnostics, adversarial tests, and inspected samples |
| DQ-008 | Add protected-text hard negatives | todo | Synthetic names, identifiers, paths, and capitalization cases preserved unchanged |
| DQ-009 | Research additional licensed source families | todo | Source comparison with license, fit, coverage, and recommendation |
| DQ-010 | Add compressed-phrase data | todo | Provenance, controls, and category-specific evaluation |
| DQ-011 | Add phonetic-spelling data | todo | Provenance, controls, and category-specific evaluation |
| DQ-012 | Strengthen cross-split isolation | todo | Validator anti-leakage tests across all identity surfaces |
| DQ-013 | Expand and version evaluation datasets | todo | Per-category denominators, confidence rationale, hashes, and protocol name |
| DQ-014 | Add five-completion product evaluation | todo | Shared aggregation fixture and offline versus sidecar agreement |
| DQ-015 | Reconcile the stale 80-character documentation | needs repair | Five-word behavior documented after safe worktree integration |
| DQ-016 | Preserve protocol-aware Discord reporting | needs external confirmation | Discord post confirmation after the final six-model report was routed with the metric-reset caveat |
| DQ-017 | Choose the first implementation slice | done | DQ-004 selected and completed without mutating the dataset |
| DQ-018 | Give the current dataset and evaluator an explicit protocol identity | todo | Baseline manifest records accepted artifacts, hashes, evaluator behavior, and excluded partial results |
| DQ-019 | Choose between the 2B product base, local 4B evaluation, and the managed 35B-A3B quality winner | needs product evidence | Mac quality, latency, memory, and adapter-loading observations |

Recommended next slice: design DQ-005 and DQ-007 together, then implement only
the deterministic event-count machinery and ambiguity rejection probes needed
for one dry dataset build. Do not authorize a paid training run until the new
distribution and rejected examples are inspected.

## Execution log

Append material actions here. Preserve failed experiments and superseded
decisions rather than rewriting history.

| Date | Action | Evidence | Result and next decision |
| --- | --- | --- | --- |
| 2026-07-18 | Reviewed the current corpus, compiler, evaluator, and product boundary. | Checked-in JSONL, build report, source manifest, code, and evaluation artifacts. | The V0 pipeline is reproducible and learns its narrow task, but central compressed and phonetic behavior is not represented. |
| 2026-07-18 | Completed the six-model, 100-step Freesolo size sweep. | `docs/model-size-scaling-results.md` and accepted evaluation artifacts. | 35B-A3B leads managed quality, 4B is the compact candidate, and the product remains on 2B pending a separate decision. |
| 2026-07-18 | Recorded user amendments for the next data iteration. | Conversation authority. | Keep 90/10, defer context training, add complexity incrementally, improve ambiguity and split quality, expand evaluation, and test five-completion behavior. |
| 2026-07-18 | Fetched current `origin/main` and inspected commits `324c63c`, `5dcbc61`, and `4d5a419`. | Git history plus current demo, Accessibility, specification, and data-contract surfaces. | Five-word behavior is newer but is implemented only in the demo path. Real app boundary reconciliation remains DQ-015. |
| 2026-07-18 | Routed verified model results and the expected metric-reset explanation through the dedicated Discord communication thread. | Codex thread `019f76d6-4a06-7df0-8629-ac24bc8b69b0`. | Delivery confirmation remains external and will not be actively polled. |
| 2026-07-18 | Reconciled conflicting 35B latency values before team reporting. | Accepted `-final` predictions average 751.23 ms; excluded non-final predictions average 795.78 ms. | Sent 751.23 ms correction to the Discord communication thread and retained the non-final artifact as invalid evidence. |
| 2026-07-18 | Established this roadmap before changing training behavior. | `docs/training-data-roadmap.md`. | Recommended next proof is the non-mutating DQ-004 diagnostic report. |
| 2026-07-18 | Implemented the deterministic non-mutating DQ-004 report. | `training/flash/dataset_quality.py`; `training/flash/scripts/report_dataset_quality.py`; `training/flash/tests/test_dataset_quality.py`. | Current build hashes match. The report finds 36 training ambiguity candidates, no spacing operations, no capitalized targets, two train/eval input overlaps, two train/test overlaps, and one conflicting mapping. Dataset validation passes and the full suite reports 52 passed. |
