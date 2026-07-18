# Phase 0 boundaries

`docs/SPEC.md` governs product behavior. This document records the v0 boundary implemented by the shared schema and app consumers.

The goal is to let workstreams integrate without deciding their internal designs early. A boundary is accepted only after one producer and one consumer validate the same fixture.

## Shared now

Only these values cross workstream boundaries:

- `prediction_request` and `prediction_result` between orchestration and inference
- `capture_result` between Accessibility and the composition UI
- `sidecar_health` between inference and the health UI

The current executable shapes live in `docs/phase-0.schema.json`. Examples live in `docs/fixtures/phase-0-examples.json`. Both implement this candidate-based boundary.

## Kept internal for now

- Accessibility element handles, insertion markers, and destination restoration state
- UI candidate and transition state
- the model completion shape, sampling count, ordering, and deduplication
- personal-example storage
- model and adapter paths
- idle timing, draft limits, and other tuning values

The Accessibility layer exposes an opaque `destination_id`. The UI returns that identifier when committing or cancelling, without interpreting the stored destination state.

## Invariants

- A request carries bounded draft text, bounded window snippets, and compact personal patterns.
- `model_variant` identifies `base`, `global`, or `global_plus_personal`. `backend` independently identifies `fixture` or `live`.
- Each internal model completion contains one full-input `suggestion`.
- Inference removes exact-draft suggestions, deduplicates changed suggestions, and returns up to five ranked candidates.
- A successful result may have zero candidates. Zero means skip and shows no suggestion bar.
- A successful result has no action field. Each candidate is a full-input replacement.
- Ranking uses duplicate vote count first and earliest completion as the tie breaker.
- Failures use an explicit error result. They do not invent candidates.
- The typed burst stays in the destination as typed (IME model). Doing nothing always keeps it; the UI shows model candidates only, and the model never returns the draft as a candidate.
- A `ready` capture carries the caret rectangle in screen coordinates so the suggestion bar can be placed above it.
- Committing a candidate replaces the just-typed burst range in the destination in place. A dismissal or zero-candidate result changes nothing.
- Secure and unsupported fields produce an unavailable capture result and never reach inference.
- Internal Accessibility state and personal records stay local to the Mac.

## Minimum fixtures

The fixtures cover:

- base and global results for the same shorthand input
- base and global results for protected text
- context-assisted replacement
- personal-pattern replacement
- zero-candidate skip and a five-candidate result
- a missing-adapter error
- ready and unavailable capture results
- ready and degraded sidecar health

Fixtures demonstrate the protocol. They are not training data or proof of model quality.

## Change rule

During v0, the affected producer and consumer may change a boundary when a fixture or integration test shows a mismatch. The integration owner resolves compatibility questions, while `docs/SPEC.md` remains authoritative for product behavior.

Boundary changes must update the schema, fixtures, Rust contract, TypeScript contract, inference adapter, and composition consumer together. The training prototype and Rust sidecar use the same vote-count ranking and earliest-completion tie breaker.
