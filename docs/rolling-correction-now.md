# Rolling correction, doc 1: ship now — no contract changes, no retrain

Scope: everything Persons 2 and 4 can build today against the current 5-word
model, without touching `crates/quip-contracts`, `docs/phase-0.schema.json`,
or the shared fixtures. The retrain-dependent work and all shared-contract
changes live in `docs/rolling-correction-full.md` (doc 2, blocked on
Person 1).

## Problem being solved here

The five-word burst window is simultaneously the model's input span, the
commit unit, and the presentation unit. Doc 1 breaks the second and third
couplings using only inference-side machinery: results become a **word-level
edit stream** with stability gating (the streaming-dictation pattern), so a
long error run produces corrections trickling out word by word instead of
sequential five-word bars. The model window itself stays at 5 words — that
coupling is broken in doc 2.

Everything below stays inside module boundaries each person already owns.
`Snapshot` (in `src-tauri/src/composition/mod.rs`) is Person 4's own type,
not a shared contract, and may change freely. Cancellation is an internal
operation between orchestration and the sidecar client, deliberately kept
out of the Phase 0 schema.

---

## Person 4

Owns: `src/ui/demo.ts`, `src-tauri/src/composition/mod.rs`, orchestration in
`src-tauri/src/main.rs`, learning labels.

### N4-1. Word-boundary cadence as default

- Make `pipeline_word` the default strategy in `demo.ts`: fire on completed
  words; sentence punctuation and fifth-word completion stay immediate;
  ~500 ms pause fallback mid-word; 80-char backstop unchanged.
- Keep the strategy dropdown as the A/B harness for the rest of this doc.
- Keep single-flight + `dirty` coalescing exactly as is; it composes with
  cancellation (N4-4) rather than being replaced by it.

Exit: with live inference, typing a sentence fires roughly one request per
completed word, none mid-word except via the fallback timer.

### N4-2. Word-level diff of results

New module (suggested: `src-tauri/src/composition/edits.rs`). Pure logic, no
model dependency — build and test first.

- `fn diff_words(draft: &str, candidate: &str) -> Vec<WordEdit>` using a
  word-level LCS/Myers alignment, where `WordEdit { draft_range:
  Range<usize>, replacement: String }` and `draft_range` indexes into the
  fired draft. Handle 1→N expansions (`"tmrw"` → `"tomorrow"`, `"cnt"` →
  `"can't"`) and N→1 merges; a wholly-unaligned result degrades to one
  whole-draft edit.
- Unit tests over the existing fixture corpus pairs (draft, candidate):
  every fixture exchange must produce a sensible edit list.

### N4-3. Edit accumulator with stability gating

The core of this doc. Session-scoped state (one per composition session,
reset by `endSession`), living in the engine so learning labels attach where
they already do.

- Track words by absolute position in the destination text (map each fired
  burst's `firedStart` offset to word slots). Per slot:

  ```rust
  struct WordSlot {
      original: String,
      hypothesis: Option<String>,   // current best correction
      consecutive_agreements: u32,  // passes in a row proposing `hypothesis`
      hardened: bool,
  }
  ```

- On each `apply_result`: run `diff_words` on the top candidate and update
  the overlapping slots. Same hypothesis as the previous pass → increment
  `consecutive_agreements`; different → reset to 1 with the new hypothesis;
  no edit proposed → decay toward "no correction".
- **Hardening rule without a confidence field** (doc 2's `votes` upgrade
  tightens this later): a slot hardens when the caret is ≥ 2 words past it
  **and** `consecutive_agreements >= 3`. Because the window slides one word
  at a time under N4-1's cadence, each word is seen up to five times, so
  three agreeing passes is available for every word without added cost.
  As a weak intra-pass proxy, a result whose candidate list collapsed to a
  single distinct candidate (the 5 samples deduplicated to 1) may count as
  two agreements — the dedup already implies unanimity.
- Hardened slots surface to the UI and are excluded from re-prediction: the
  slot boundary, not the raw window slide, now defines "final".
- `Snapshot::Suggesting` (or a new `Snapshot::Editing` phase) carries
  `edits: Vec<{ range, original, replacement, stable: bool }>` so the
  webview renders per-word marks instead of one bar per burst.
- Preserved invariants: stale results still dropped by `burst_id`;
  zero-candidate results decay hypotheses; dismissal semantics per N4-5.
- Instrument the demo stats line with per-word agreement counts — this is
  the data that tunes the thresholds until doc 2's measured curves exist.

Exit: an engine test scripts a 25-word all-errors session and asserts that
corrections harden progressively behind the caret, never in visible 5-word
groups.

### N4-4. Client-side cancellation

- When a word boundary fires while a request is in flight and the draft has
  moved past the fired window, call the internal `cancel(request_id)`
  command (see N2-2) instead of only setting `dirty`, then fire the fresh
  request as soon as the engine confirms the slot is free.
- Keep `dirty` as the fallback for backends that cannot cancel (fixture).

### N4-5. Presentation and learning labels

- **Inline marks** for hardened edits: subtle underline/highlight on the
  corrected span in the caret-anchored overlay; hover/arrow-key focus shows
  original vs. replacement; Tab applies the nearest, Escape reverts.
- Auto-apply stays behind a setting (`auto_apply_threshold`), default off
  for the judged demo. The interrupting candidate bar is reserved for
  low-stability multi-candidate results.
- Learning labels: applied edit → `replace` with `source: "hardened_edit"`;
  explicit revert → `keep` with `source: "revert"`; marks ignored at
  session end → no label. Pattern extraction
  (`learning::extract_patterns`) runs per-edit, which yields cleaner
  shorthand→expansion pairs than the current whole-burst diff.

---

## Person 2

Owns: `src-tauri/sidecars/inference/`, `src-tauri/src/inference/`, serving
configuration, benchmarks.

### N2-1. Streaming completions

Prerequisite for real cancellation. Switch the completion call in `live.rs`
to `"stream": true` and assemble deltas. Verify the `n: 5` + streaming
interaction: if the server rejects `n > 1` with streaming, fall back to 5
parallel `n:1` requests over 5 sockets (which also makes cancellation
instant) — benchmark both shapes and keep the faster one.

### N2-2. Request cancellation (internal operation)

- `SidecarClient` keeps a handle to the in-flight request's `TcpStream`
  (e.g. `Arc<Mutex<Option<(String /*request_id*/, TcpStream)>>>` via
  `try_clone`). `cancel(request_id)` calls `shutdown(Shutdown::Both)` when
  the id matches; the blocked `predict` returns an I/O error mapped to a
  non-retryable internal `ErrorInfo { code: "cancelled" }` that
  orchestration swallows silently. No shared-schema change: fixture mode
  ignores cancel.
- **Verify the server actually stops decoding**: llama.cpp-family servers
  (including LM Studio) abort generation when a streaming client
  disconnects. Fire a long generation, cancel, and confirm via server
  logs/GPU utilization that decode stops. Record the finding either way;
  if the server ignores disconnects, cancellation is client-side only and
  the rest of the doc still stands (it just saves less).

### N2-3. Prefix caching

- Keep prompt ordering stable-first (system prompt, then personal patterns,
  then the volatile draft last). `model_input` already ends with the draft;
  verify personal patterns render before it once they are non-empty.
- Enable/verify the server's prompt cache and measure warm per-request
  latency at window size 5 with and without cache, on both target machines.
  Also measure at 10 and 15 words (oversized inputs are fine for a latency
  benchmark even if quality is off-distribution) — these numbers feed
  doc 2's window decision.
- Confirm cache reuse does not change outputs: re-run the phrase-tester
  corpus cached vs. uncached; results must match at temperature 0.1.

### N2-4. Per-word cost reduction (optional, measure first)

Under N4-1 the request rate roughly triples versus `pause_150`. If N2-3's
measurements show the sidecar cannot keep up, drop `COMPLETION_COUNT` from
5 to 3 for mid-burst passes (stability now comes from cross-pass agreement,
so intra-pass sampling matters less) and keep 5 only for punctuation-
triggered passes. Decide from measurements, not upfront.

---

## Sequencing and exit

Order of landing (each independently shippable):

1. N4-2 (diff module, pure logic) and N2-1/N2-3 in parallel.
2. N4-1 (cadence default) + N2-2 + N4-4 (cancellation end to end).
3. N4-3 (accumulator) + N4-5 (presentation).

**Exit demo:** live 25-word scripted session on the current 5-word model —
corrections stream out per word behind the caret, no chunk boundaries
visible; time-to-first-hardened-correction and cancellation savings
measured and recorded in the demo stats line.

Doc 1 changes no shared contract and no training artifact, so rollback is
switching the default strategy back to `pause_150` and hiding the marks
overlay.
