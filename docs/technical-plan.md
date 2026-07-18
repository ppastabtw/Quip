# Technical plan: Quip

Source: `docs/SPEC.md`

## Tech stack

- **desktop shell**: Tauri 2 for the menu bar app, settings UI, composition box, global shortcut, and clipboard integration.
- **system layer**: Rust for macOS integration, application state, orchestration, sidecar control, and packaging-sensitive code.
- **macOS Accessibility**: `axuielement` for Accessibility trust, focused elements, text markers, and `AXObserver`; `objc2` ApplicationServices bindings only where `axuielement` lacks coverage.
- **composition UI**: HTML and CSS rendered through Tauri for the temporary writing box, exact-draft option, candidate list, settings, and demo controls.
- **local inference**: `mistral.rs` with Metal as the leading runtime for Qwen3.5, 4-bit quantization, LoRA merging, strict schemas, and Rust-facing integration.
- **model family**: start with Qwen3.5-2B at 4-bit; benchmark Qwen3.5-4B only after 2B establishes a working latency and quality baseline.
- **global training**: Flash `EnvironmentSingleTurn` for dataset construction, SFT, optional GRPO, checkpoint evaluation, inspected rollouts, and global LoRA export.
- **personalization**: Freesolo-trained per-user LoRA adapter plus a compact local pattern dictionary before enough examples exist for training.
- **storage**: local files or an embedded local store for settings, personal examples, learned pattern dictionary, adapter metadata, and profile-specific state; no remote database in the judged build.
- **observability**: local structured logs, demo-visible latency metrics, schema-validity counters, and evaluation reports; upload only compact confirmed examples selected for a private Freesolo training run.

## Key decisions and rejected alternatives

**compose before commit**: Quip captures the writing burst in its own temporary box and commits only after explicit confirmation. Direct edits to destination fields were rejected because they make cancellation, protected-token preservation, and trust harder to demonstrate.

**local inference with Freesolo training**: The base model, exported adapters, prompts, drafts, ambient context, and primary personal record store stay on the Mac. Global and per-user adapter training run through Freesolo, using prepared global data or compact confirmed profile examples. Remote inference was rejected.

**Accessibility text over screenshots or OCR**: Window context comes from bounded accessible text snippets ranked locally by focus, recency, and relevance. Screenshots and OCR were rejected for the hackathon build because they increase privacy risk, implementation complexity, and demo fragility.

**one prediction per burst**: Prediction runs after punctuation, Return, or a short idle pause instead of on every character. Per-keystroke inference was rejected because it increases latency pressure and makes local model comparison noisier.

**guided JSON output contract**: The model emits either `keep` with no candidates or `replace` with one to three full-input candidates. Free-form responses were rejected because commentary, partial edits, and schema drift would complicate commit safety and evaluation.

**exact draft is always a commit option**: The application adds the unchanged draft as the first option even when the model recommends `keep`. Auto-committing `keep` was rejected because it bypasses the confirmation contract.

**mistral.rs sidecar first, direct SDK later**: Start with a bundled local loopback sidecar for inference so model lifecycle failures are isolated from the Tauri app. Direct Rust SDK integration remains a later optimization once adapter loading and schema decoding are proven.

**adapter composition with merge fallback**: The preferred runtime loads the Qwen base, frozen global Freesolo adapter, and separate user adapter together. If stacking fails, merge the global adapter into the base once and load a single user adapter over it.

**private Freesolo profile runs**: Per-user LoRA training uses private Freesolo runs so the sponsor technology is part of both global improvement and personalization. Only compact confirmed examples enter a profile run, and the exported adapter returns to local inference.

**narrow judged app scope**: The demo targets TextEdit, Notes, and one standard browser input. Rich editors, terminals, PDFs, secure fields, canvas editors, unusual Electron controls, and universal macOS support are explicitly out of scope.

**parallel workstreams with narrow contracts**: The four builders should work in parallel around the inference, capture, and health boundaries. Accessibility restoration, UI state, storage, tuning, and other internal details stay with their workstream until integration proves they must be shared.

## Architecture overview

Quip runs as a Tauri menu-bar app with a Rust core. When enabled, the Accessibility layer detects supported editable destinations, records the active app, element, selection, insertion point, and nearby visible text, then routes the user's writing burst into Quip's temporary composition box. The orchestration layer waits for punctuation, Return, or an idle trigger, builds a bounded model input from the draft or selection, relevant window snippets, and compact user patterns, then calls the local inference sidecar for the base Qwen and global Freesolo adapter comparison.

The UI always presents the exact draft plus any schema-valid candidates. On confirmation, the commit layer restores the original destination and inserts or replaces text through Accessibility where possible, falling back to simulated paste while preserving the previous clipboard. The learning layer records only compact labeled interactions that are useful for personalization, deduplicates repeated patterns, updates the local pattern dictionary immediately, and eventually refreshes the per-user adapter while idle.

The training path is separate from the inference path. Flash owns global and per-user adapter training, checkpoint inspection, evaluation, and export. SFT learns the JSON contract from gold `output` values because Flash rejects `structured_outputs` for SFT; GRPO constrains sampled rollouts with `train.structured_outputs`; local inference enforces the same schema. Profile runs receive only compact confirmed labeled examples, never live inference traffic or ambient window context by default.

## Module and folder structure

The repository has no application source yet. A likely implementation structure is:

```text
src-tauri/
  src/
    accessibility/      # focused element detection, text markers, observers, window text, secure-field exclusion
    composition/        # burst capture, idle trigger, draft bounds, candidate state, exact-draft handling
    commit/             # destination restore, accessibility insertion, selection replacement, paste fallback
    inference/          # sidecar client, schema validation, base-vs-trained comparison, latency metrics
    learning/           # local examples, pattern dictionary, profile state, adapter refresh orchestration
    settings/           # context toggle, learning pause/reset, profile selection, demo controls
  sidecars/
    inference/          # local model runtime wrapper around mistral.rs or fallback runtime
src/
  ui/                   # Tauri webview composition box, settings, demo harness
training/
  flash/                # global and profile datasets, reward/eval scripts, checkpoint comparison, adapter export notes
artifacts/
  models/               # ignored local model/adapter paths and metadata, not committed binaries
docs/
  SPEC.md               # product/design source of truth
  OPEN_QUESTIONS.md     # build-time experiments
```

## Risk areas

**mistral.rs adapter loading on Metal**: Failure looks like the app can run base Qwen but cannot load the exported Freesolo adapter or strict JSON decoding reliably. Roll back to a replaceable local sidecar with the same request/response contract. De-risk by proving adapter loading, quantization, schema decoding, and latency before wiring inference into the UI.

**global plus user adapter composition**: Failure looks like personalization cannot run alongside the global Freesolo behavior, or user adapters overwrite the trained restraint. Roll back to a base model with the global adapter merged once and one user adapter loaded over it. De-risk with a small adapter-composition test harness using two intentionally different user profiles.

**managed per-user training**: Failure looks like a small profile dataset overfits, weakens restraint, or produces an adapter that cannot compose with the global adapter. Roll back to the compact pattern dictionary plus two prepared Freesolo-trained profile adapters for the judged build. De-risk with held-out profile examples and explicit global-plus-user composition tests.

**accessibility destination preservation**: Failure looks like Quip loses the insertion point, commits into the wrong field, modifies text on cancel, or fails across TextEdit and browser inputs. Roll back to the narrowest known-good app/input pair and simulated paste fallback with clipboard restore. De-risk by building the destination capture/restore spike before the full Tauri UI.

**window context quality and privacy**: Failure looks like accessible snippets are empty, irrelevant, too large, or accidentally include secure/excluded fields. Roll back to disabling context by default while keeping the menu-bar toggle and deterministic examples. De-risk by auditing snippets from TextEdit, Notes, Chrome, and Safari with bounded length, source labels, and secure-field checks.

**latency on target hardware**: Failure looks like the 700 to 900 ms idle trigger feels broken because local inference returns too slowly or memory pressure destabilizes the demo. Roll back to Qwen3.5-2B at 4-bit, shorter prompts, smaller context, or deterministic corpus comparison. De-risk by measuring base and trained-model latency on both target Macs before expanding model size or context.

**evaluation credibility**: Failure looks like the trained model improvement appears cherry-picked or the held-out split leaks into training. Roll back to fewer but inspected examples with clear category reporting. De-risk by keeping held-out prompts distinct, deduplicating normalized patterns, rerunning important evaluations, and showing category-level error rates.

**demo integration**: Failure looks like all subsystems work separately but the live flow cannot complete in the allotted pitch time. Roll back to a deterministic corpus comparison plus a fallback recording. De-risk with a final integration phase, scripted examples from real model outputs, rehearsal, and compatibility testing.

**parallel integration drift**: Failure looks like four builders completing local pieces whose boundary data does not line up. Roll back to deterministic fixtures for inference, capture, and health. De-risk by validating the shared fixtures before independent work starts.

## Assumptions

- `docs/SPEC.md` is the design source of truth in place of `plans/design.md`.
- The technical plan artifact should live at `docs/technical-plan.md`, not `plans/technical-plan.md`.
- The judged build optimizes for a live Hack the 6ix demo over general-purpose macOS compatibility.
- The team has four builders available and should work in parallel across model training, local inference, Accessibility, and Tauri/demo integration.
- The primary demo machine is an M3 Pro with 18 GB unified memory, with an M4 MacBook Air with 16 GB as the compatibility target.
- The app can require Accessibility permissions for the judged build.
- Model binaries, exported adapters, personal records, and local profile data should stay out of Git.
- The official demo duration is not confirmed; plan for a three-minute core pitch until the schedule is verified.

## Agenda

### Phase 0: shared contracts and fixtures

1. Define provisional v0 schemas only for prediction exchange, capture result, and sidecar health.
2. Add paired base and trained fixtures plus capture, health, and missing-adapter cases.
3. Validate the fixtures with one producer and one consumer before workstreams diverge.
4. Keep workstream internals and tuning values out of the shared contract until integration requires them.

### Workstream 1: Flash training and evaluation

1. Scaffold the single-turn, non-reasoning Flash environment with `EnvironmentSingleTurn`, prompt construction, and `RewardResult` scoring.
2. Package `dataset/train.jsonl` and `dataset/eval.jsonl` with top-level `input`, optional `output`, and optional `metadata` keys only.
3. Baseline Qwen3.5 on the held-out split and report category-level errors, unnecessary edit rate, protected-token preservation, shorthand and phonetic decoding, schema validity, and latency.
4. Run SFT with JSON gold outputs and positive `train.max_examples`, then evaluate the adapter on the untouched split.
5. Warm-start GRPO with `init_from_adapter` only if SFT improves the task; omit LoRA rank and alpha, constrain rollouts with the Quip JSON schema, and use explicit `max_steps` and `save_at_steps`.
6. Run Flash dry-run and cost-estimation checks before training submission.
7. Export the chosen adapter immediately to a team-owned Hugging Face repository and document checkpoint, rank, quantization target, held-out metrics, and inspected failure cases.
8. Add a private per-user Freesolo run path from compact confirmed examples, then export and evaluate two distinct profile adapters.

### Workstream 2: local inference, adapters, and packaging

1. Build the local inference sidecar around `mistral.rs` with the shared request/response contract and deterministic fixture mode.
2. Prove base Qwen loading, 4-bit Metal inference, guided JSON decoding, schema validation, and latency reporting before consuming app events.
3. Load the global Freesolo adapter exported by the training workstream; if it fails, swap in a replaceable local runtime behind the same sidecar contract.
4. Test global plus per-user adapter composition with two intentionally different user profiles; if stacking fails, merge the global adapter into the base and load one user adapter.
5. Benchmark Qwen3.5-2B on the M3 Pro and M4 Air; test Qwen3.5-4B only if quality requires it and latency remains interactive.
6. Load Freesolo-trained per-user adapters from two prepared profiles and verify that each changes only its intended patterns.
7. Package model and adapter paths as local artifacts with health checks, missing-artifact errors, and no committed model binaries.

### Workstream 3: Accessibility capture, commit, and context

1. Implement Accessibility permission detection, focused editable element recognition, secure-field exclusion, and supported-app gating.
2. Capture destination application, element, selection, insertion point, and text-marker state before redirecting input.
3. Prototype writing-burst interception in TextEdit and one browser input while leaving the destination unchanged.
4. Implement idle, punctuation, and Return triggers with initial 700 to 900 ms idle timing and an 80-character draft window.
5. Restore the destination and commit confirmed text through Accessibility insertion or selection replacement.
6. Add simulated paste fallback that preserves and restores the previous clipboard.
7. Collect bounded accessible window text from supported apps, rank snippets locally, and expose only bounded context records to the orchestration layer.
8. Validate cancellation, exact-draft commit, candidate commit, wrong-field prevention, secure-field avoidance, and context toggle behavior across TextEdit, Notes, and the chosen browser.

### Workstream 4: Tauri composition UI, learning, and demo harness

1. Build the Tauri menu-bar shell with enabled state, context toggle, learning pause/reset, profile selection, settings access, and existing-text shortcut.
2. Implement the composition box with stable candidate layout, exact draft first, up to three model candidates, loading state, unavailable state, and cancel behavior.
3. Wire fixture-backed candidate rendering before the live inference sidecar is ready.
4. Store compact local labeled examples from confirmed candidates, stable dismissed suggestions, post-commit corrections, and repeated personal patterns, then package only selected examples for Freesolo profile training.
5. Build the local pattern dictionary for immediate personalization before adapter training has enough examples.
6. Add demo comparison screens for base Qwen versus trained-model output, protected-token preservation, shorthand decoding, context resolution, personalization, and latency.
7. Add local structured logs, sidecar health display, schema-validity counters, model/adapter presence checks, and a deterministic corpus fallback mode.

### Integration checkpoints

1. Connect Workstream 3 destination snapshots and burst events to Workstream 4 composition state using the shared fixtures from Phase 0.
2. Connect Workstream 4 prompt construction to Workstream 2 fixture mode, then to live sidecar inference once the adapter-loading proof passes.
3. Connect Workstream 1 exported adapter artifacts to Workstream 2 packaging and verify the demo corpus uses real model outputs, not hand-written candidates.
4. Connect Workstream 4 selected profile examples to Workstream 1 Freesolo profile training, then connect the exported adapters to Workstream 2 and verify two local profiles produce different candidates.
5. Run an end-to-end TextEdit flow: capture, predict, exact-draft option, candidate option, cancel, restore, commit, comparison, and local example capture.
6. Run an end-to-end browser flow with bounded window context and protected-token examples.

### Final hardening

1. Select final demo examples from real model outputs covering noisy shorthand, ordinary typo correction, protected-token preservation, ambiguous context, and two personalized profiles.
2. Verify operational readiness for the live build: local logs, sidecar health checks, model/adapter presence checks, latency reporting, deterministic corpus fallback, and rollback to fixture mode.
3. Test compatibility on the M3 Pro primary machine and M4 Air backup machine, recording quality, latency, memory behavior, and app-specific limitations.
4. Rehearse the pitch around the live comparison, Flash environment, training configuration, checkpoint evaluation, exported adapter, and local privacy contract.
5. Prepare a fallback recording and deterministic corpus flow while keeping the primary pitch live.
6. Update `docs/OPEN_QUESTIONS.md` as risks are proven, rejected, or moved into fallback status.
7. Package the repo and demo materials for Devpost with public code, evaluation summary, training configuration, exported adapter explanation, and a concise build/process writeup.
