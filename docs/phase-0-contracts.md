# Phase 0 boundaries

`docs/SPEC.md` governs product behavior. This document records the provisional v0 wire boundary.

The goal is to let workstreams integrate without deciding their internal designs early. A boundary is accepted only after one producer and one consumer validate the same fixture.

## Shared now

Only these values cross workstream boundaries:

- `prediction_request` and `prediction_result` between orchestration and inference
- `capture_result` between Accessibility and the composition UI
- `sidecar_health` between inference and the health UI

The executable shapes live in `docs/phase-0.schema.json`. Examples live in `docs/fixtures/phase-0-examples.json`.

## Kept internal for now

- Accessibility element handles and burst-range markers
- UI candidate and transition state
- raw model transport and aggregation implementation. Each raw completion still validates against `$defs.model_completion`.
- personal-example storage
- model and adapter paths
- idle timing, draft limits, and other tuning values

The Accessibility layer exposes an opaque `destination_id`. The UI returns that identifier when committing without interpreting the stored destination state.

## Invariants

- A request carries bounded draft text, bounded window snippets, and compact personal patterns.
- `model_variant` identifies `base`, `global`, or `global_plus_personal`. `backend` independently identifies `fixture` or `live`.
- Each internal model completion is exactly one JSON object matching `$defs.model_completion`. Plain text, extra fields, empty suggestions, and commentary are invalid.
- Inference runs exactly five completions, removes exact-draft suggestions, deduplicates changed suggestions, and returns zero through five ranked candidates.
- A successful result may have zero candidates. Zero means skip and shows no suggestion bar.
- A successful result has no action field. Each candidate is a full-input replacement.
- Ranking uses duplicate vote count first and earliest completion as the tie breaker.
- A successful result may carry `votes`, parallel to `candidates`: how many raw samples resolved to each candidate (each at least 1). Producers that cannot vote omit it (the fixture backend emits all 1s); consumers treat a missing field as "no confidence signal". Correction hardening in the edit accumulator is gated on this signal.
- A `ready` capture may carry `word_offset`: the 0-based session word index of the draft's first word, counted from the last composition-session boundary. Producers that do not track session word positions omit it; bursts without it never update per-word correction state.
- Failures use an explicit error result. They do not invent candidates.
- The typed burst stays in the destination as typed (IME model). Doing nothing always keeps it; the UI shows model candidates only, and the model never returns the draft as a candidate.
- A `ready` capture carries the caret rectangle in screen coordinates so the suggestion bar can be placed above it.
- Committing a candidate replaces the just-typed burst range in the destination in place. A dismissal or zero-candidate result changes nothing.
- Secure and unsupported fields produce an unavailable capture result and never reach inference.
- Internal Accessibility state and personal records stay local to the Mac.

## Minimum fixtures

The fixtures cover:

- base and global results for the same shorthand input
- zero-candidate and five-candidate successful results
- base and global results for protected text
- context-assisted replacement
- personal-pattern replacement
- a missing-adapter error
- ready and unavailable capture results
- ready and degraded sidecar health

Fixtures demonstrate the protocol. They are not training data or proof of model quality.

## Change rule

During v0, the affected producer and consumer may change a boundary when a fixture or integration test shows a mismatch. The integration owner resolves compatibility questions, while `docs/SPEC.md` remains authoritative for product behavior.
