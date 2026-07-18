# Rolling correction, doc 2: contract changes and the retrained model

Scope: everything blocked on either a shared-contract change or Person 1's
retrain. Prerequisite reading: `docs/rolling-correction-now.md` (doc 1),
which builds the word-level edit stream, stability gating, cancellation, and
caching on the current 5-word model with no contract changes. This doc
upgrades that machinery — it does not replace it.

What doc 2 unlocks over doc 1:

- **Confidence-gated hardening** (`votes`): doc 1 hardens on cross-pass
  agreement alone; the votes field restores the intra-pass signal.
- **Variable window length**: the 5-word cap becomes a runtime setting once
  the model is trained on 1–15 word spans.
- **Read-only committed prefix**: corrections become context-aware without
  ever touching finalized text.
- **Sentence consolidation**: one bar offering the whole corrected sentence
  at the period.

---

## Shared contract changes (one PR, Person 2 drafts, Person 4 reviews)

Touches `crates/quip-contracts/src/lib.rs`, `docs/phase-0.schema.json`,
`docs/phase-0-contracts.md`, `docs/fixtures/phase-0-examples.json`, and the
round-trip test. Land before Phase B below. Both changes are additive and
optional-field-shaped so old producers/consumers keep working.

### C1. Candidate confidence (`votes`)

`PredictionResult::Ok` gains a parallel array:

```rust
Ok {
    request_id: String,
    model_variant: ModelVariant,
    backend: Backend,
    candidates: Vec<String>,
    /// Same length as `candidates`: how many of the 5 raw samples resolved
    /// to this candidate after filtering. Omitted by producers that cannot
    /// vote (fixture backend emits all-1s).
    #[serde(skip_serializing_if = "Option::is_none")]
    votes: Option<Vec<u32>>,
    latency_ms: u64,
}
```

`validate()` additionally checks `votes.len() == candidates.len()` and
`votes[i] >= 1` when present. The data already exists in
`live.rs::normalize_model_outputs` (the vote map) and is currently discarded
during ranking.

### C2. Committed prefix

`PredictionRequest` gains:

```rust
/// Finalized text immediately before `draft`, bounded to the last ~20
/// words. Context only: the model must never rewrite it. Empty until the
/// retrained model lands (the current model was not trained on it).
#[serde(default, skip_serializing_if = "String::is_empty")]
pub committed_prefix: String,
```

Rendering is gated on the loaded model advertising support — a
`SidecarHealth`-reported model revision or an env flag
(`QUIP_MODEL_SUPPORTS_PREFIX=1`) — so the old model never sees a field it
was not trained on. The gate is also the rollback switch.

---

## Person 2

### F2-1. Emit votes (C1)

Carry the vote count through `normalize_model_outputs`'s ranked list into
`PredictionResult::Ok.votes`. Fixture backend emits
`Some(vec![1; candidates.len()])`. Update schema, fixtures, `validate()`,
and the round-trip test. Small; land with the contract PR.

### F2-2. Committed-prefix rendering (gated)

- `model_input` renders `{"committed_prefix": ..., "text": ...}` when the
  request carries a prefix **and** the capability gate is on.
- Update `system_prompt.txt` in lockstep with Person 1: the deployed prompt
  must be byte-identical to the training prompt, and it is `include_str!`-ed
  at compile time, so this is a coordinated commit.
- Hard post-check in code, not just training: if the suggestion textually
  contains a rewritten prefix (detectable because the suggestion should
  align to the tail only), strip or reject it.

### F2-3. Output budget and validators for longer tails

- Scale `max_tokens` with input length (`~2 * draft_words + 16`) instead of
  the fixed 64.
- Parameterize `is_implausibly_truncated` on draft length: for drafts ≥ 8
  words, reject only when the suggestion drops more than one word relative
  to the draft, rather than any shortfall — tune against Person 1's eval
  set.
- Re-run `is_model_scaffolding` tests against the new system prompt text,
  since that filter string-matches prompt lines.

### F2-4. Window-size performance sign-off

Re-run the doc 1 (N2-3) latency matrix at the runtime window chosen by
F1-4's quality curve, on both target machines, with prefix caching on.
Record prefill, decode tok/s, total latency, and peak memory — this is the
evidence for raising the default window.

---

## Person 4

### F4-1. Votes-based hardening

Tighten doc 1's hardening rule with the restored intra-pass signal: a slot
hardens when the caret is ≥ 2 words past it **and**
`consecutive_agreements >= 2` **and** top-candidate `votes >= 3` (of 5).
Thresholds are then re-tuned against F1-4's consistency curve instead of
the doc 1 heuristics.

### F4-2. Hardened-prefix request construction

- `begin_burst` fills `committed_prefix` (C2) with the last ≤ 20 hardened
  words from the session's edit-stream state, behind the same capability
  gate as F2-2.
- Raise `MAX_BURST_WORDS` from a constant to a setting
  (`AppSettings.window_words`, default 5 until Phase C, then the F1-4
  number, expected 8–10). Scale the 80-char backstop with it
  (`window_words * 16`).

### F4-3. Sentence consolidation pass

At sentence punctuation, fire one request over the whole sentence (the
retrained model handles sentence-length spans), present the result as a
single candidate bar offering the fully corrected sentence; accepting it
replaces the sentence and supersedes any pending word marks inside it.
Learning label: `replace` with `source: "sentence_pass"`.

---

## Person 1

### F1-1. Variable-length windows

- `dataset_compiler/contract.py`: `window_sizes` → `(1..=15)` with a
  **weighted allocation** replacing the current equal split (weights
  roughly proportional to real usage: heavy 1–5, tapering to 15; the
  even-divisibility check in `DatasetContract` becomes a weighted-quota
  check). `sources.py::sample_massive_windows` already parameterizes on
  `CONTRACT.window_sizes`; verify MASSIVE utterances are long enough to
  fill the 10–15 pools and add a second source if the pools starve.
- Add a small pool of sentence-length examples (16–25 words,
  sentence-aligned) tagged `category: "sentence_pass"` for F4-3.
- Update `scripts/validate_datasets.py` and `tests/` for the new contract.

### F1-2. Committed-prefix examples

- Example input becomes `{"committed_prefix": "<preceding words,
  verbatim>", "text": "<corrupted tail>"}`; output remains the corrected
  tail only. Populate the prefix from the source utterance's words before
  the sampled window; keep a healthy share of empty-prefix rows so the
  no-prefix case stays strong.
- Augmentation (`augmentation.py`) must never corrupt the prefix.
  **Adversarial rows required**: prefixes that themselves contain errors or
  shorthand the model must leave untouched — this is what actually teaches
  "read-only".
- Metadata records `prefix_words` alongside `window_size` for eval
  bucketing.

### F1-3. Prompt update

Extend `system_prompt.txt` with the prefix rule (wording owned by Person 1
since it must match training): the object may contain `committed_prefix`;
it is finalized earlier text provided only for context; the reply corrects
only `text` and never contains or rewrites the prefix. Coordinate the exact
bytes with F2-2.

### F1-4. Length and consistency evaluation

Two curves out of the existing harness (`benchmarking.py`, `scoring.py`,
`scripts/run_managed_eval.py`):

1. **Quality vs. window size**: bucket held-out results by `window_size`
   1–15; report correction accuracy, over-edit rate, and prefix-violation
   rate per bucket. The knee of this curve sets the runtime `window_words`
   (expected 8–10) — this number is the handoff to F4-2.
2. **Pass-to-pass consistency**: for a corpus of long corrupted texts,
   slide the runtime-sized window one word at a time, run each offset, and
   measure per-word agreement across overlapping passes at temperature 0.1.
   Deliver the agreement distribution to Person 4 to re-tune F4-1's
   thresholds with data instead of heuristics.

### F1-5. Retrain and handoff

Retrain via the existing `configs/sft*.toml` flow. Handoff mirrors the
Milestone 4 checklist in `docs/person-2-plan.md`: base model id and
revision, checkpoint revision, tokenizer and chat-template assumptions, the
byte-exact system prompt, held-out category results including both F1-4
curves, and an artifact checksum. The old model stays deployable — the C2
capability gate is the rollback switch.

---

## Sequencing

**Phase B — contract integration (no retrain needed; can start as soon as
doc 1's accumulator exists):**

- Contract PR (C1 + C2) lands; F2-1 emits votes; F4-1 switches hardening to
  real votes. `committed_prefix` stays empty and ungated-off.
- Checkpoint: doc 1's 25-word scripted session now hardens on votes;
  fixture mode still passes all shared fixtures.

**Phase C — model swap (blocked on Person 1's return):**

- F1-1 → F1-5 in order; F2-2 + F1-3 land as one coordinated commit; gate
  on; F4-2 sends the prefix and raises `window_words` to the F1-4 number;
  F4-3 sentence pass; F2-4 sign-off.
- Checkpoint: the scripted session plus a full-sentence rewrite offered at
  the period; prefix-violation rate ~0 on the held-out set, including
  adversarial prefixes; fixture mode and the old model remain one-step
  rollbacks (`gate off + window_words = 5` reproduces doc 1 behavior
  exactly).

## Definition of done

- `votes` flows producer → consumer and gates hardening; thresholds re-tuned
  from the F1-4 consistency curve.
- Retrained model: quality-vs-length and consistency curves recorded;
  runtime window raised per the curve with F2-4's performance evidence.
- Committed prefix demonstrably untouched on adversarial held-out prefixes,
  enforced both by training and by F2-2's code-level post-check.
- Sentence-consolidation bar works end to end and supersedes pending word
  marks.
- Rollback: capability gate off + `window_words = 5` reproduces doc 1
  behavior exactly.
