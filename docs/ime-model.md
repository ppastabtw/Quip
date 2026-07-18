# Quip IME integration boundary

Decided 2026-07-18. `docs/SPEC.md` owns product behavior and `docs/phase-0-contracts.md` owns the shared wire contract. This note only assigns the cross-workstream implementation boundary.

## Workstream boundaries

- Workstream 1 owns the single-suggestion model contract, training, and evaluation.
- Workstream 2 owns exactly five inference completions, exact-input filtering, deduplication, ranking, and the candidate-only result.
- Workstream 3 owns passive Accessibility observation, burst markers, caret geometry, in-place replacement, secure-field exclusion, and candidate selection keys.
- Workstream 4 owns the non-focusable candidate bar and its visual states.
- Internal learning labels are separate from the inference result. Leaving text unchanged is not a model action.
