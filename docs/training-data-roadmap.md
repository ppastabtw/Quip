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

The validated V2 dry-build candidate is documented separately in
`docs/training-data-v2-draft.md`. It preserves V1, keeps 90/10, adds one through
four severity-aware events, separates compressed and phonetic categories,
rejects common-token ambiguous drafts, isolates normalized surfaces across
splits, and evaluates the ranked five-completion decision. It has not been
trained or promoted.

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

The V2 work is isolated on branch `agent/quip-data-v2`, based on the published
V1 merge commit. V1 generated rows remain unchanged and reproduce their
committed hashes.

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
| DQ-005 | Add deterministic multi-event augmentation | revised build ready | Inspect samples and repeat representative 2B evidence |
| DQ-006 | Add spacing and dropped-vowel transformations | done in V2 draft | Validated compressed rows, operator counts, and inspected examples |
| DQ-007 | Filter ambiguous valid-word corrections | partial | Common-token probes are zero; grammatical and contextual ambiguity remains a known heuristic limit |
| DQ-008 | Add protected-text hard negatives | todo | Synthetic names, identifiers, paths, and capitalization cases preserved unchanged |
| DQ-009 | Research additional licensed source families | todo | Source comparison with license, fit, coverage, and recommendation |
| DQ-010 | Add compressed-phrase data | implemented in V2 draft | Provenance and category counts exist; protected unchanged controls still remain |
| DQ-011 | Add phonetic-spelling data | implemented in V2 draft | Deterministic rules and category counts exist; broader phonetic coverage remains |
| DQ-012 | Strengthen cross-split isolation | done in V2 draft | Validator rejects injected leakage; dry build has zero input, target, and family overlap |
| DQ-013 | Expand and version evaluation datasets | todo | Per-category denominators, confidence rationale, hashes, and protocol name |
| DQ-014 | Add five-completion product evaluation | done | Runtime and evaluator share ranking; 2B base and five checkpoints each produced 1,000 completions |
| DQ-015 | Reconcile the stale 80-character documentation | needs repair | Five-word behavior documented after safe worktree integration |
| DQ-016 | Preserve protocol-aware Discord reporting | needs external confirmation | Discord post confirmation after the final six-model report was routed with the metric-reset caveat |
| DQ-017 | Choose the first implementation slice | done | DQ-004 selected and completed without mutating the dataset |
| DQ-018 | Give the current dataset and evaluator an explicit protocol identity | done for V2 draft | `docs/training-data-v2-draft.md` records hashes, evaluator behavior, and exclusions |
| DQ-019 | Choose between the 2B product base, local 4B evaluation, and the managed 35B-A3B quality winner | needs product evidence | Mac quality, latency, memory, and adapter-loading observations |
| DQ-020 | Restrict pipeline iteration to 2B | done | User decision recorded; every other model size remains a historical benchmark entry only until protocol finalization |
| DQ-021 | Reject underdetermined heavy corruptions | in progress | Deterministic recoverability rule, regression fixtures, rebuilt audit, and inspected rejected examples |

Recommended next slice: publish the runtime-aligned, globally filtered dataset
environment and run one representative 2B SFT plus ranked-five checkpoint
sweep. Do not train any other model size.

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
| 2026-07-18 | Built and iterated the ignored V2 dry dataset. | `docs/training-data-v2-draft.md`; compiler report; inspected samples; repeated seed-42 build. | V2 has exact severity quotas, compressed and phonetic families, zero flagged common-token ambiguity cases, and zero protected cross-split overlap. V1 hashes remain byte-identical. |
| 2026-07-18 | Replaced single-completion evaluation with the product-ranked five-completion protocol. | Shared `rank_candidate_items` fixture; managed, benchmark, and checkpoint runners; evaluator tests. | Reports top-ranked success, candidate recall at five, and mean completion success separately. Previous V1 percentages are not directly comparable. |
| 2026-07-18 | Published an isolated V2 environment and ran the representative training preflight. | Environment `ariobarin/quip-v2-draft-20260718t2050`; dry-run `flash-1784418431-547761a4`; committed representative config. | Server validation passed. Estimated training cost is $0.07. No paid run was submitted pending explicit approval. |
| 2026-07-19 | Completed the first representative V2 2B training run and full checkpoint sweep. | Run `flash-1784436250-97876093`; base predictions; checkpoints 20 through 100; 200 examples and 1,000 completions per evaluation. | Step 80 led the JSON-reply protocol at 63.5% ranked success, 72.5% recall at five, and 15.0% unnecessary edits versus 35.5%, 44.5%, and 90.0% for base. The run charged $0.07. Runtime validation and revised-data comparison remain required before promotion. |
| 2026-07-19 | Rebasing onto `origin/main` initially looked like an intentional reply-contract migration. | Main commit `d054d4f`; scoring and prompt tests; adapter prediction artifacts. | Superseded after the spec, schema, datasets, and sidecar documentation were reconciled. Plain text was partial contract drift, not the authoritative protocol. |
| 2026-07-19 | Audited step 80 failures by category, severity, and candidate aggregation. | Preserved prediction JSONL and deterministic ranked-five scorer. | Of 31 compressed failures, 27 lacked the accepted target among all five completions. The audit exposed awkward source fragments, underdetermined heavy corruptions, one protected proper-name interruption, and two fragment-like unchanged interruptions. Repair data quality before another run. |
| 2026-07-19 | Limited iterative training to Qwen 2B only. | User amendment authority. | Keep every other model-size result in the historical benchmark, but do not train or tune those sizes until the pipeline is finalized. |
| 2026-07-19 | Replaced arbitrary MASSIVE windows with runtime-aligned chunks and broadened common-word rejection. | Seed-42 build hashes, 68 passing tests, quality report, and inspected evaluation samples. | The 2,000/200/200 build preserves exact severity and 90/10 quotas, follows five-word queue boundaries, rejects all-common English drafts without a corpus-membership loophole, and has zero protected split overlap. |
| 2026-07-19 | Tested slot-wide protected-value preservation as a possible hard-negative policy. | Failed deterministic build at test size one with 29 of 40 rows after all protected values were made immutable. | Rejected as overbroad. Keep DQ-008 open for a smaller explicit protected-text corpus instead of weakening the current quotas or hiding the failed experiment. |
| 2026-07-19 | Restored one strict JSON model completion contract across training and inference. | `docs/phase-0.schema.json`; scorer, prompts, Freesolo requests, prototype requests, Rust live sidecar parser, and contract tests. | The existing step 80 adapter is eligible for runtime validation. A new 2B run measures the revised dataset, not schema compatibility, and is accepted only if it beats the old adapter under the same evaluator. |
