# Quip UI pivot: IME-style candidate bar

Decided 2026-07-18. Quip dropped the separate composition box in favor of the
interaction model of Sogou and the macOS Pinyin input method. `docs/SPEC.md`
is updated and authoritative; this doc summarizes the pivot, what Workstream 4
already implemented, and what Workstream 3 must do differently.

## Interaction model

1. The user types in their own textbox (TextEdit, Notes, a browser input).
   Keystrokes pass through untouched — the destination always receives the
   text as typed.
2. On a trigger (punctuation, Return, ~400 ms idle, 80-char window), Quip runs
   one prediction on the burst. The idle pause is deliberately short because
   inference latency stacks on top of it.
3. `replace` result → a small non-focusable bar floats directly above the
   caret with up to three numbered candidates. `keep` result → **nothing
   appears**; the user is never interrupted.
4. While the bar is visible: `1`–`3` (or click) replaces the just-typed burst
   in place; `Esc` or simply continuing to type dismisses it and changes
   nothing.
5. The bar never takes keyboard focus. Focus stays in the destination the
   whole time.

Consequences of the model:

- There is no exact-draft option anymore. Keeping the typed text is the
  do-nothing default because the text is already in the destination.
- Commit semantics changed from "insert into a restored field" to "replace
  the burst range in place."
- Learning labels: selected candidate → `replace` example; dismissal of
  visible candidates (Esc or typing on) → `keep` example; a `keep` result
  records nothing (no user signal).

## Contract changes (Phase 0, v0)

- `capture_result.ready` gained a **required `caret` field**: a rect in
  logical screen coordinates (origin top-left) anchoring the bar.
  Updated in `docs/phase-0.schema.json`, `docs/fixtures/phase-0-examples.json`,
  `crates/quip-contracts` (Rust), and `src/ui/contracts.ts` (TS).
- `prediction_request` / `prediction_result` / `sidecar_health` are
  **unchanged** — Workstreams 1 and 2 are unaffected.
- Invariants reworded in `docs/phase-0-contracts.md`: the typed burst stays in
  the destination as typed; the UI shows model candidates only; committing a
  candidate replaces the burst range; dismissal and `keep` change nothing.

## Implemented (Workstream 4, branch `w4-composition-ui`)

- **Engine** (`src-tauri/src/composition/`): `Idle → Predicting → Suggesting`;
  keep → no bar; errors → explicit error chip with no candidates; stale
  results dropped after dismissal; typing over suggestions counts as a stable
  dismissal.
- **Candidate bar** (`suggestions` window + `src/ui/suggestions.*`):
  frameless, transparent, `focusable: false`, auto-sized, positioned above
  the caret by the Rust side on every snapshot.
- **Playground** (demo window): a textarea simulating any macOS textbox —
  trigger detection, caret geometry to screen coordinates, digit selection,
  Esc/type-through dismissal, and in-place replacement — so the full IME feel
  is demoable before Workstream 3 lands.
- **Unchanged**: learning store and pattern dictionary, settings + tray,
  corpus comparison screens, health panel, metrics, structured logs.
- **Validated**: 25 unit tests; headless selftest (`QUIP_SELFTEST=1`) drives
  capture → suggest → select/dismiss/keep/failure through the real app,
  7 checks passing; visual check via `QUIP_DEMO_CAPTURE=1` confirmed the bar
  renders at caret coordinates without stealing focus.

## Required changes to Workstream 3

The pivot deletes W3's hardest problems (keystroke rerouting, focus restore)
and adds one scoped responsibility (selection keys).

1. **Passive observation instead of interception.** Do not swallow or reroute
   typing. Watch the focused editable element via `AXObserver`
   (value-changed / selection-changed), maintain the burst buffer, and keep a
   text marker for the burst start.
2. **Emit the extended capture.** On trigger, send `capture_result.ready`
   with the draft plus the `caret` rect in logical screen coordinates.
   Validate against the shared fixtures.
3. **Commit = in-place replacement.** Select the burst range (burst-start
   marker → current caret) and replace it with the selected candidate via
   Accessibility; fallback is select-range + simulated paste with clipboard
   preserve/restore. There is no destination-restore step: focus never moved.
4. **Selection keys while the bar is visible.** A CGEventTap must swallow
   only `1`–`3` and `Esc` when suggestions are showing (observe
   `composition://state` or call `get_composition_state`) and forward them as
   `select_candidate` / `dismiss_suggestions`. Every other key passes through
   and doubles as a dismissal signal.
5. **Unchanged:** secure-field exclusion (`unavailable` captures), supported
   app gating, bounded window-context collection.

## Open items

- `docs/technical-plan.md` still describes the old compose-box model in its
  W3/W4 sections; SPEC.md wins where they conflict.
- Bar polish before the judged demo: multi-display coordinate clamping,
  appear/dismiss animation, light-mode variant, real app icon.
- Existing-text mode (global shortcut over a selection) is specced for the
  same bar but not yet implemented.
