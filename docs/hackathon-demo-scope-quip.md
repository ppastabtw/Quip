# Hackathon Demo Scope: Quip

Source: `docs/SPEC.md`, `docs/technical-plan.md`, and integration QA through July 18, 2026.

## Current State

- **Remaining time:** short; optimize for demo reliability over product completeness.
- **Demo format:** assumed live demo plus Devpost/slides; exact rubric and sponsor criteria are unverified.
- **Runnable state:** local macOS app, not a hosted product.
- **Working or partly working:** TextEdit Accessibility capture/commit, candidate bar, deterministic fixture candidates, safe demo mode, context toggle, local logs, demo harness, local inference path/health work.
- **Fragile:** browser rich editors, Obsidian, Slack/Discord, universal live typing, caret geometry, Accessibility focus resolution, model latency, adapter availability.
- **Broken as a promise:** "live typing suggestions anywhere on macOS." Accessibility cannot support that reliably in the time left.

## Demo Spine

Problem state -> trigger action -> visible transformation -> proof of result -> why it matters:

```text
Someone types compressed or messy English in TextEdit
  -> Quip observes the typed burst and shows ranked candidates near the caret
  -> the presenter selects one candidate
  -> the text is replaced in place, only after explicit selection
  -> the demo proves local, context-aware English composition with safe user control
```

This is the only primary demo flow. Selection-transform mode is a backup/proof of broader utility, not the main story.

## Remembered Moment

**remembered moment:** The presenter types `cnt cm tmrw`, Quip floats suggestions, and one key turns it into `Can't come tomorrow.` directly in the editor.

**what changes on screen:** `cnt cm tmrw` becomes `Can't come tomorrow.` in TextEdit after an explicit candidate choice; dismissal would leave the original text unchanged.

**why it scores:** It makes the product visible immediately: compressed text -> useful candidate -> safe in-place commit. Rubric fit is unverified, but this maps to product usefulness, local inference, and model-quality storytelling.

## Feature Triage

### Must Work

- TextEdit live demo path: capture typed draft, show candidate bar, select candidate, replace text in place.
- One curated compressed phrase: `cnt cm tmrw -> Can't come tomorrow.`
- One ordinary typo/minimal-edit example, either in the live TextEdit flow or the deterministic comparison harness.
- Candidate dismissal/unchanged behavior: the user can keep typed text by doing nothing or dismissing.
- Local logs visible enough for debugging before the demo.
- Demo harness comparison for base vs trained/global-plus-personal behavior, especially if live model latency or adapters are shaky.
- Safe demo mode: a clearly labeled `Run safe demo` fallback that bypasses real Accessibility while reusing Quip's candidate and commit flow.

### Fake Safely

- Use deterministic fixture candidates if live inference is too slow or model artifacts are missing.
- Use `Run safe demo` if Accessibility focus lands on menu chrome or the TextEdit field cannot be captured during the live slot.
- Use seeded profile examples to show two users producing different candidates.
- Use prepared context snippets or a known-good TextEdit context setup for the context example.
- Use a cached/recorded model comparison if the live adapter path fails during rehearsal.
- Use selection-transform mode as a "broader workflow prototype" if it lands, but be honest that it is prototype scaffolding.

### Cut Now

- Universal live typing across macOS.
- Obsidian live typing support.
- Slack/Discord live typing support.
- Google Docs, Notion, rich browser editors, terminals, PDFs, password fields, canvas editors.
- Production packaging, notarization, installer polish.
- Automatic per-user retraining during the demo.
- Full Sogou/Pinyin-level IME smoothness.
- InputMethodKit integration on the main demo path. Keep the IMK worktree as a parallel spike only.

### Only Mention

- True macOS input method architecture via InputMethodKit as the next step for broad live typing.
- Browser/Obsidian/Slack/Discord context adapters.
- Production privacy hardening.
- Full local model packaging with robust adapter management.
- Longer-term Freesolo per-user adapter refresh from confirmed interactions.

## Judge-Proof Setup

- Use the primary Mac with Accessibility permission already granted for ChatGPT/Codex, terminal, editor, and the Quip app process as needed.
- Start from a clean TextEdit document with large enough font to see the before/after.
- Start Quip in fixture or known-good mode unless live inference has passed rehearsal twice in a row.
- Keep `QUIP_DEMO_SAFE_MODE=1 QUIP_SHOW=demo npm run tauri -- dev` ready as the explicit presenter fallback.
- Confirm the tray/menu settings before demo: enabled on, window context set to the mode required for the chosen example, active profile set.
- Preload exact demo phrases:
  - `cnt cm tmrw`
  - one typo example from the deterministic corpus
  - one profile-specific example
  - one context-specific example
- Keep `.workspace/quip-debug/events.jsonl` or the demo log tail available off-screen for fast diagnosis, but do not make logs part of the main visible story.
- Prepare one backup screen recording showing the TextEdit flow and one screenshot/recording of the comparison harness.
- Before rehearsal freeze, run `.agents/skills/validate-quip-demo/scripts/validate.sh` and confirm it ends with `Quip demo validation passed`.
- Honest sentence for mocked/seeded parts: "For the live hackathon demo, this path can run from deterministic local fixtures when model artifacts are unavailable; the same request/response contract is used by the local inference worker."

## Build Order

1. **Demo spine runnable:** make the TextEdit path deterministic and boring: launch app, type phrase, show candidates, select, commit, dismiss.
2. **Seed demo data:** lock final phrases and fixture/model outputs; remove phrases that produce zero candidates.
3. **Visible polish only on demo path:** show clear candidate states, no silent hiding, readable candidate bar, obvious selected candidate.
4. **Context/profile proof:** use the demo harness or a known-good TextEdit setup to show context and two-profile behavior.
5. **Backup flow:** record the primary TextEdit flow and comparison harness before continuing to risky work.
6. **Optional broader utility:** add or polish selection-transform mode only if the primary flow is already rehearsed.
7. **Rehearse out loud:** run the exact flow start-to-finish at least three times with no code changes between rehearsal and demo freeze.
8. **Final checks:** verify no model binaries, adapters, personal data, secrets, or generated logs are committed.

## Backup Demo

- If TextEdit live capture fails: click `Run safe demo`, then play the backup recording or show the deterministic comparison harness live.
- If live inference fails: switch to fixture mode and say this uses the same local prediction contract.
- If context capture fails: use the prepared context fixture in the harness.
- If candidate commit fails: show candidate generation and use the recording for the in-place commit moment.
- If browser/Obsidian questions come up: say the hackathon build targets TextEdit/standard inputs; true broad live typing moves to the parallel InputMethodKit spike.

## Rehearsal Bullets

- "Quip helps people type compressed or messy English and choose a safe correction without auto-overwriting them."
- "The typed text stays unless the user explicitly selects a candidate."
- "The model contract returns full-text suggestions, then Quip deduplicates and shows only changed candidates."
- "The hackathon build proves the core local composition loop; broad macOS app support is the next native input-method layer."
- "Freesolo is used for global and per-user adapter training; local inference is the product direction."

## Open Risks

- TextEdit flow may still be sensitive to focus/shortcut timing.
- Candidate bar may feel less smooth than a real IME.
- Live model artifacts or adapter composition may not be ready in time.
- Context capture is currently basic Accessibility visible text, not app-specific context harnessing.
- Judging rubric and sponsor requirements are unverified.

## Next Build Block Check

After the next implementation block, update this file with:

- Which **must work** items are now working.
- Which **must work** items are still broken.
- Any newly risky dependency that should be faked, cut, or moved to backup.
