# Quip IME candidate bar

Decided 2026-07-18. `docs/SPEC.md` is authoritative. This note preserves only the cross-workstream interaction boundary.

## Interaction

1. The user types directly in the destination. Quip never moves focus to a composition box.
2. Predictions run as the current burst grows. The existing bar remains visible while a newer prediction computes.
3. Each completion emits one full-text suggestion. Inference drops exact-input results, merges duplicate changed results, ranks them by vote count with earliest completion as the tie breaker, and exposes up to five candidates.
4. Zero candidates means skip. Nothing appears and the typed text remains unchanged.
5. While candidates are visible, keys `1` through `5` or a click select a candidate, Tab selects the highlighted candidate, and Esc dismisses the bar. Other typing passes through and refreshes the burst.
6. Selecting a candidate replaces the current burst range in place. The bar never takes keyboard focus.

## Workstream boundaries

- Workstream 1 owns model output, exact-input filtering, deduplication, ranking, and candidate limits.
- Workstream 3 owns passive Accessibility observation, burst markers, caret geometry, in-place replacement, secure-field exclusion, and candidate selection keys.
- Workstream 4 owns the non-focusable candidate bar and its visual states.
- Internal learning labels are separate from the inference result. Leaving text unchanged is not a model action.

## Shared-contract migration

The current Phase 0 schema, fixtures, Rust contract, TypeScript contract, inference adapter, and composition consumer still encode the earlier action field and smaller candidate cap. Their owners must migrate them as one integration change. Do not update only one producer or consumer.

The migration is complete when one shared fixture proves zero candidates, ranked deduplication, five candidates, selection keys `1` through `5`, dismissal without replacement, and explicit failure behavior.
