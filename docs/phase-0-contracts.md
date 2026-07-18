# Phase 0 boundaries

`docs/SPEC.md` governs product behavior. `docs/phase-0.schema.json` defines the provisional v0 wire format. If they conflict, the specification wins.

The goal is to let workstreams integrate without deciding their internal designs early. A boundary is accepted only after one producer and one consumer validate the same fixture.

## Shared now

Only these values cross workstream boundaries:

- `prediction_request` and `prediction_result` between orchestration and inference
- `capture_result` between Accessibility and the composition UI
- `sidecar_health` between inference and the health UI

The executable shapes live in `docs/phase-0.schema.json`. Examples live in `docs/fixtures/phase-0-examples.json`.

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
- Inference removes exact-draft suggestions, returns `keep` when no changed suggestion remains, and otherwise returns `replace` with at most three deduplicated changed suggestions.
- A successful `keep` result has no candidates.
- A successful `replace` result has one to three full-input replacements, ordered best first.
- Failures use an explicit error result. They do not invent an action or candidates.
- The application adds the exact draft option independently. A matching model suggestion is not exposed as a candidate.
- Secure and unsupported fields produce an unavailable capture result and never reach inference.
- Internal Accessibility state and personal records stay local to the Mac.

## Minimum fixtures

The fixtures cover:

- base and global results for the same shorthand input
- base and global results for protected text
- context-assisted replacement
- personal-pattern replacement
- a missing-adapter error
- ready and unavailable capture results
- ready and degraded sidecar health

Fixtures demonstrate the protocol. They are not training data or proof of model quality.

## Change rule

During v0, the affected producer and consumer may change a boundary when a fixture or integration test shows a mismatch. The integration owner resolves compatibility questions, while `docs/SPEC.md` remains authoritative for product behavior.
