# Engineering Spec: Demo Gaps And Safe Demo Fallback

## Context

- **Current demo scope:** The judged path is TextEdit-first: type `cnt cm tmrw`, show candidates, select one, and turn it into `Can't come tomorrow.` [docs/hackathon-demo-scope-quip.md:14](hackathon-demo-scope-quip.md:14). The scope explicitly cuts universal macOS typing, Obsidian, Slack/Discord, rich browser editors, production packaging, and InputMethodKit from the main demo [docs/hackathon-demo-scope-quip.md:55](hackathon-demo-scope-quip.md:55).
- **Current implementation:** Real capture enters through `capture_active_destination` [src-tauri/src/main.rs:335](../src-tauri/src/main.rs:335). Scripted capture already enters through `inject_capture` [src-tauri/src/main.rs:478](../src-tauri/src/main.rs:478). The demo harness already injects a TextEdit-like capture [src/ui/demo.ts:832](../src/ui/demo.ts:832). The primary phrase exists in the fixture corpus [src-tauri/fixtures/demo_corpus.json:4](../src-tauri/fixtures/demo_corpus.json:4).
- **Problem:** The product loop exists, but the demo is still fragile: macOS Accessibility focus can point at menu/chrome instead of the text field, zero-candidate cases look like nothing happened, and live inference/model artifacts are not this workstream's responsibility.
- **Goal:** Fix only the gaps needed to make the TextEdit demo legible, and add an explicit `safe demo mode` that can always produce the remembered moment by bypassing real Accessibility while reusing Quip's existing candidate and commit flow.
- **Out of scope:** Do not make Obsidian, Slack, Discord, browsers, or universal live typing work here. Do not migrate to InputMethodKit. Do not fix live inference quality, Freesolo training, adapters, packaging, or installer behavior.

## Canonical Vocabulary

| Concern | Use this term | Avoid | Meaning |
| --- | --- | --- | --- |
| Primary judged flow | `TextEdit demo path` | `universal support` | We only promise TextEdit for the live demo. |
| Emergency scripted fallback | `safe demo mode` | `fake app`, `nuclear mode` | Explicit fallback that is honest in logs/UI. |
| One-click fallback action | `Run safe demo` | `Send TextEdit capture fixture` | Presenter-facing button/env path. |
| Existing scripted seam | `inject_capture` | `fallback engine` | Reuse the existing pipeline instead of duplicating candidate UI. |
| Real macOS capture path | `manual focused capture` | `safe demo` | Real Accessibility path, still best-effort. |

## Boundary Examples

### Safe Demo Capture

`safe demo mode` must create this `CaptureResult::Ready` shape for the primary case:

```json
{
  "status": "ready",
  "burst_id": "safe_demo_primary",
  "destination_id": "destination_textedit",
  "profile_id": "profile_default",
  "draft": "cnt cm tmrw",
  "trigger": "shortcut",
  "caret": { "x": 512, "y": 384, "width": 2, "height": 18 }
}
```

Expected first candidate in fixture mode:

```text
Can't come tomorrow.
```

Expected safe-mode debug event:

```json
{
  "event": "demo_safe_mode_started",
  "summary": "safe demo started: primary",
  "payload": {
    "case_id": "primary",
    "destination_id": "destination_textedit",
    "draft_chars": 11,
    "accessibility_bypassed": true
  }
}
```

### Real TextEdit Success

The real path is demo-ready only when logs show:

```text
capture_requested
capture_ready
prediction_started
prediction_result(candidate_count > 0)
bar_shown
candidate_selected
commit_succeeded
```

## Architecture

```text
real TextEdit path:
tray/menu action
  -> capture_active_destination
  -> accessibility capture
  -> run_capture_result
  -> run_burst_flow
  -> candidate bar
  -> select_candidate
  -> commit

safe demo fallback:
Run safe demo button / QUIP_DEMO_SAFE_MODE=1
  -> safe_demo_capture(case_id) (new)
  -> inject_capture / run_capture_result
  -> run_burst_flow
  -> candidate bar
  -> select_candidate
  -> virtual destination commit
```

## Detailed Design

### 1. Safe Demo Helper

Add one Rust helper near `inject_capture` in `src-tauri/src/main.rs` [src-tauri/src/main.rs:478](../src-tauri/src/main.rs:478):

```rust
fn safe_demo_capture(case_id: &str) -> Result<CaptureResult, String>
```

Supported cases:

| case_id | draft | profile_id | destination_id | expected candidate |
| --- | --- | --- | --- | --- |
| `primary` | `cnt cm tmrw` | `profile_default` | `destination_textedit` | `Can't come tomorrow.` |
| `typo` | `i went to the store instaed` | `profile_default` | `destination_textedit` | `I went to the store instead.` |
| `short` | `omw` | `profile_default` | `destination_textedit` | Fixture candidate from corpus |

Use the fixed caret from existing demo/selftest patterns: `x=512.0`, `y=384.0`, `width=2.0`, `height=18.0`. Use `destination_textedit` because virtual destinations already bypass real app commit fragility [src-tauri/src/commit/mod.rs:173](../src-tauri/src/commit/mod.rs:173).

Unknown `case_id` returns `Err("unknown safe demo case: <case_id>")`.

### 2. Tauri Command

Add this command in `src-tauri/src/main.rs`:

```rust
#[tauri::command]
async fn run_safe_demo(app: AppHandle, case_id: Option<String>)
```

Behavior:

1. Default `case_id` to `primary`.
2. Record `demo_safe_mode_started`.
3. Force fixture prediction for this run if possible without persisting settings. If persistence is simpler, set fixture mode and emit the existing settings-changed event.
4. Call the same path as `inject_capture`; do not render candidates directly in the UI.
5. On error, record `demo_safe_mode_failed` with `case_id` and `error`.

Register it in `tauri::generate_handler!` near existing commands [src-tauri/src/main.rs:1198](../src-tauri/src/main.rs:1198).

### 3. Environment Toggle

Replace or wrap the old `QUIP_DEMO_CAPTURE=1` startup hook [src-tauri/src/main.rs:1133](../src-tauri/src/main.rs:1133) with:

```bash
QUIP_DEMO_SAFE_MODE=1 QUIP_SHOW=demo npm run tauri -- dev
```

Behavior:

- Wait about 1500 ms after startup, matching the old hook.
- Run the `primary` safe demo case.
- Open the demo harness when safe mode is active, unless that creates Tauri window duplication.
- Emit `demo_safe_mode_started` and `capture_ready`.
- Do not call real Accessibility capture.

### 4. Demo Harness Button

Add a visible presenter button beside the current Composition Driver controls [src/ui/demo.html:49](../src/ui/demo.html:49):

```html
<button id="run_safe_demo">Run safe demo</button>
```

Add the IPC wrapper in `src/ui/ipc.ts` beside `injectCapture` [src/ui/ipc.ts:125](../src/ui/ipc.ts:125):

```ts
runSafeDemo: (caseId?: string) => invoke<void>("run_safe_demo", { caseId })
```

Wire it in `src/ui/demo.ts` near the existing fixture injection handler [src/ui/demo.ts:832](../src/ui/demo.ts:832):

- Before calling Rust, append timeline/debug text `demo_safe_mode_requested`.
- Call `api.runSafeDemo("primary")`.
- On success, set the last state to `safe demo: candidates requested`.
- On failure, show the error in the last state and record `demo_safe_mode_failed`.

The existing `Send TextEdit capture fixture` button may remain for debugging, but `Run safe demo` is the presenter-facing fallback.

### 5. Real TextEdit Gap Fixes

Keep this narrow. Add a one-shot retry inside `capture_active_destination` [src-tauri/src/main.rs:335](../src-tauri/src/main.rs:335) only when the first Accessibility diagnostic looks like a transient focus/menu miss:

- `focused.app_bundle_id == null`
- `focused.resolution_error == "not_text_role"`
- focused/chosen/system role is `AXMenuItem`

Behavior:

1. Record the first diagnostic as today.
2. If retryable, sleep 150 ms.
3. Record `capture_retry`.
4. Capture diagnostics again and proceed once.
5. Do not retry secure fields or clearly unsupported non-null app bundles.

This is only to smooth the tray/menu timing case. It is not browser/Obsidian support.

### 6. Phrase Guardrail

Only use phrases backed by `src-tauri/fixtures/demo_corpus.json`:

- `cnt cm tmrw` [src-tauri/fixtures/demo_corpus.json:4](../src-tauri/fixtures/demo_corpus.json:4)
- `i went to the store instaed` [src-tauri/fixtures/demo_corpus.json:17](../src-tauri/fixtures/demo_corpus.json:17)
- `omw` [src-tauri/fixtures/demo_corpus.json:47](../src-tauri/fixtures/demo_corpus.json:47)

Do not use `hi how are you` or ad hoc demo phrases, because zero candidates read as a broken app.

### 7. Observability

Use the existing debug sink; do not add a second log system.

Required new events:

- `demo_safe_mode_requested`
- `demo_safe_mode_started`
- `demo_safe_mode_failed`
- `capture_retry`

Required `capture_retry` payload:

```json
{
  "source": "manual_focused_capture",
  "retry_delay_ms": 150,
  "first_resolution_error": "not_text_role",
  "first_app_bundle_id": null
}
```

## Implementation Checklist

- [x] Add `safe_demo_capture(case_id)`: `src-tauri/src/main.rs`
- [x] Add `run_safe_demo` command: `src-tauri/src/main.rs`
- [x] Register `run_safe_demo`: `src-tauri/src/main.rs`
- [x] Add `QUIP_DEMO_SAFE_MODE=1` startup path: `src-tauri/src/main.rs`
- [x] Add one-shot focused-capture retry: `src-tauri/src/main.rs`
- [x] Emit safe-demo and retry debug events: `src-tauri/src/main.rs`
- [x] Add `runSafeDemo`: `src/ui/ipc.ts`
- [x] Add `Run safe demo` button: `src/ui/demo.html`
- [x] Wire button behavior and UI state: `src/ui/demo.ts`
- [x] Ensure demo phrases exist in fixture corpus: `src-tauri/fixtures/demo_corpus.json`
- [x] Add app validation skill: `.agents/skills/validate-quip-demo/SKILL.md`
- [x] Add validation script: `.agents/skills/validate-quip-demo/scripts/validate.sh`
- [x] Update demo scope doc after implementation: `docs/hackathon-demo-scope-quip.md`

## Execution Progress

- [x] Demo gap fixes: `complete`
- [x] Safe demo fallback: `complete`
- [x] Validation skill/script: `complete`
- [x] Manual QA: `validate-quip-demo passed`

## Testing & Validation Plan

Automated:

- Run `cargo fmt --check`.
- Run `cargo test`.
- Run `npm run build`.
- Run fixture selftest and assert output contains `SELFTEST PASS`.
- Add `.agents/skills/validate-quip-demo/scripts/validate.sh` so future agents can validate this app-side demo path without rediscovering commands.

Manual:

1. Start Quip in fixture mode.
2. Open a clean TextEdit document.
3. Type `cnt cm tmrw`.
4. Trigger `Manual focused capture`.
5. Verify candidates appear; select the first candidate.
6. Verify TextEdit changes to `Can't come tomorrow.` or logs show `commit_succeeded`.
7. Open the demo harness.
8. Click `Run safe demo`.
9. Verify candidates appear without relying on real Accessibility.
10. Select the first candidate.
11. Verify the harness shows a virtual commit to `destination_textedit`.
12. Launch with `QUIP_DEMO_SAFE_MODE=1 QUIP_SHOW=demo npm run tauri -- dev` and verify the safe demo starts automatically.

Regression risks:

- Retry could accidentally double-capture if implemented outside `capture_active_destination`.
- Safe demo could silently mask real capture failures if it runs in normal app mode.
- Persisting fixture mode could surprise live-inference testers.
- Validation scripts must not commit generated logs or personal text.

## Open Questions

- Should `Run safe demo` auto-select candidate 1? Recommendation: no. Keep explicit user selection so the demo still shows user control.
- Should safe mode open TextEdit? Recommendation: no. Opening GUI apps adds macOS automation risk; use the harness and virtual destination for fallback.

## References

- Demo scope: [docs/hackathon-demo-scope-quip.md:14](hackathon-demo-scope-quip.md:14)
- Current cuts: [docs/hackathon-demo-scope-quip.md:55](hackathon-demo-scope-quip.md:55)
- Demo fixture phrase: [src-tauri/fixtures/demo_corpus.json:4](../src-tauri/fixtures/demo_corpus.json:4)
- CaptureResult contract: [crates/quip-contracts/src/lib.rs:162](../crates/quip-contracts/src/lib.rs:162)
- Real capture command: [src-tauri/src/main.rs:335](../src-tauri/src/main.rs:335)
- Injected capture command: [src-tauri/src/main.rs:478](../src-tauri/src/main.rs:478)
- Startup demo hook: [src-tauri/src/main.rs:1133](../src-tauri/src/main.rs:1133)
- Command registration: [src-tauri/src/main.rs:1198](../src-tauri/src/main.rs:1198)
- Virtual destination commit bypass: [src-tauri/src/commit/mod.rs:173](../src-tauri/src/commit/mod.rs:173)
- Demo harness controls: [src/ui/demo.html:49](../src/ui/demo.html:49)
- Demo fixture wiring: [src/ui/demo.ts:832](../src/ui/demo.ts:832)
- IPC wrapper location: [src/ui/ipc.ts:125](../src/ui/ipc.ts:125)
