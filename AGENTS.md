# Quip repository guide for agents

This file is the working map for the repository. Read it before changing code.
It explains what Quip is, which documents are authoritative, how the current
implementation fits together, what is still a prototype, and how to validate a
change through the real boundaries it affects.

## Authority and decision order

When sources disagree, use this order:

1. `docs/SPEC.md` is the product and experience source of truth.
2. `docs/technical-plan.md` is the implementation plan and risk register.
3. `docs/phase-0-contracts.md` states the shared runtime invariants.
4. `docs/phase-0.schema.json` is the executable wire shape, with examples in
   `docs/fixtures/phase-0-examples.json`.
5. `docs/training-data-contract.md` is the global dataset policy. Its executable
   quotas are in `training/flash/dataset_compiler/contract.py`.
6. The current code describes what is implemented today. Plans describe the
   intended destination, not proof that a feature works.

`docs/ime-model.md`, `docs/benchmarking.md`, and workstream notes are supporting
references. Workstream plans such as `docs/person-2-plan.md` may be stale or
experimental; never let them override the sources above or the current shared
contract. Freesolo integration decisions must also agree with the official
documentation at `https://freesolo.co/docs`.

## Product in one minute

Quip is a local-first macOS composition layer for misspelled, compressed, or
phonetic English. It behaves like an English IME:

- The user types directly into an existing textbox. Quip observes; it does not
  redirect or pre-commit input.
- A bounded writing burst, relevant accessible window text, and compact personal
  patterns form a prediction request.
- Local inference runs exactly five completions. Each completion emits one
  object: `{ "suggestion": "best full text" }`.
- The inference layer removes exact-input results, deduplicates changed strings,
  and ranks them by vote count, using earliest completion as the tie-breaker.
- Zero changed candidates means skip: keep the typed text and show no bar.
- One to five changed candidates appear in a non-focusable bar above the caret.
  Number keys or a click select, arrows move the highlight, Tab accepts the
  highlighted candidate, and Escape dismisses. Space remains normal input.
- Text changes only after explicit selection. Accessibility replacement is
  preferred; simulated paste is the fallback and must restore the clipboard.
- Confirmed interactions and stable dismissals can become local learning labels.
  Repeated patterns provide immediate personalization; Freesolo-trained global
  and per-user LoRA adapters are the longer-term path.

The product inference path is local on the Mac. Freesolo performs training and
may provide managed serving for Windows-side model experiments, but managed
serving is not the Quip app runtime.

## Current reality: implemented versus planned

Do not infer completion from file names or from the product specification.

| Area | Implemented now | Still incomplete or only simulated |
| --- | --- | --- |
| Tauri shell | Tray-only Tauri 2 app, settings window, demo window, non-focusable suggestion bar, state events, structured local logs | Existing-text global shortcut and production packaging polish |
| Composition | `Idle -> Predicting -> Suggesting`, stale-result dropping, zero-candidate skip, selection, dismissal labels, fixture latency replay, standalone InputMethodKit burst capture and native candidate commit | Broader text-client compatibility and production input-method packaging polish |
| Fixture inference | Shared fixtures, richer deterministic demo corpus, health/failure paths, base/global/personal comparisons | Fixtures are not model-quality evidence |
| Live inference | Persistent Rust child sidecar, loopback-only `mistral.rs` HTTP client, local Qwen3.5 Base inference, five-completion aggregation, guided JSON, health and latency | Global adapter, user adapter, adapter composition, packaged sidecar restart supervision |
| Accessibility | Permission check, supported-app gating, secure-field rejection, focused editable capture, opaque destination registry | `AXObserver`, live burst markers, real caret geometry, continuous key handling, selected existing-text mode |
| Commit | AX selected-text write, focus/range restore, paste fallback with plain-text clipboard restore | Reliable replacement of the preceding typed burst; capture currently stores the selection range, not a continuously tracked burst range |
| Window context | Always-on bounded context for supported, non-secure destinations; one 240-character Accessibility snippet from the focused window for manual and native IME captures; editable controls are excluded, with a TextEdit document fallback and a Notes path that excludes the caret line | Multi-window collection, recency/relevance ranking, and broader client validation |
| Learning | Per-profile JSON patterns, append-only JSONL labels, seeded demo profiles, thresholded pattern injection, reset/inspect UI | Profile dataset packaging, Freesolo profile submission, adapter download/load, post-commit correction labels |
| Training | Reproducible MASSIVE compiler, deterministic QWERTY augmentation, checked-in train/eval/test splits, reward/scoring code, Flash environment, SFT/OPD/GRPO configs, managed benchmark tools | A checked-in claim that a selected trained adapter wins and loads locally |
| Validation | Rust tests, Python tests, deterministic process validator, live Qwen/sidecar/Tauri validator, and real TextEdit/Notes/Chrome native-context validator | Broader browser/client compatibility and real external commit are not fully automated |

Important consequences:

- `capture_focused_destination` remains a callable one-shot Accessibility spike.
  Ordinary live typing instead uses the separately installed Quip Native
  InputMethodKit source and its loopback bridge to the Tauri engine.
- Captured caret coordinates use a fallback rectangle at `(0, 0)`; real AX
  caret geometry is not wired.
- `collect_context_snippets` traverses the focused supported window for visible
  static Accessibility text, excluding editors and secure fields. TextEdit
  reads its document editor as a fallback; Notes reads its active editor and
  removes the newline-bounded caret line. It returns at most one 240-character
  snippet and is not yet a multi-window relevance ranker.
- `replace_burst` logs a failed real commit but still returns the selected text
  so the demo textarea can update. A passing fixture self-test therefore does
  not prove an external TextEdit commit.
- Keyboard navigation is implemented by the demo textarea in the harness and
  by the active InputMethodKit controller in real text clients. The
  non-focusable suggestion window never receives keys and no global event tap
  is required for the native-input-source path.
- The native input source is not installed by a normal Rust or npm build.
  `scripts/install-input-method.sh` changes `/Library/Input Methods`, requires
  macOS administrator approval, and the user must enable Quip Native in
  Keyboard Settings.
- Live mode accepts `base` only. `global` and `global_plus_personal` return
  `adapter_not_loaded` until real adapters are integrated.
- Fixture personalization is local token substitution from the pattern
  dictionary. It is not a user LoRA.

## End-to-end architecture

The app path is:

```text
Quip Native input source in a destination textbox, or demo textarea
  -> loopback native IME bridge when using a real destination
  -> CaptureResult (ready or unavailable)
  -> Tauri command layer in src-tauri/src/main.rs
  -> composition::Engine::begin_burst
  -> PredictionRequest
       -> app FixtureBackend, or
       -> persistent sidecar child -> fixture/live sidecar backend
  -> PredictionResult
  -> Engine::apply_result (drops stale results)
  -> Snapshot event
  -> non-focusable candidate bar
  -> explicit selection
  -> AX commit or paste fallback
  -> local learning label and pattern update
```

The offline training path is separate:

```text
pinned MASSIVE 1.1 en-US archive
  -> source filtering and one seeded window per source utterance
  -> deterministic QWERTY augmentation
  -> train/eval/test JSONL plus provenance report
  -> QuipEnvironment and shared scoring contract
  -> Freesolo SFT (optional OPD/GRPO follow-up)
  -> held-out managed evaluation and inspected checkpoint selection
  -> exported adapter
  -> local sidecar loading (not implemented yet)
```

## Repository map

### Root and shared contracts

- `Cargo.toml`: Rust workspace for `quip-contracts`, the Tauri app, and the
  inference sidecar.
- `package.json`: Vite/Tauri frontend commands. `npm run build` type-checks and
  builds all three webview entry pages.
- `.cargo/config.toml`: adds the Swift compatibility-library search path needed
  by the AX bridge on Apple Silicon machines that have Command Line Tools but
  not full Xcode. Keep this target-specific linker workaround intact.
- `.gitattributes`: normalizes text to LF and explicitly pins generated dataset
  JSON/JSONL line endings so hashes remain reproducible across Windows and Mac.
- `.gitignore`: excludes builds, Python environments/caches, local source
  caches, secrets, models/adapters, logs, and generated evaluations. It also
  ignores Markdown outside the explicitly allowed project documentation paths.
- `.env.example`: placeholder names for Freesolo and Backboard credentials;
  actual `.env` content is local and ignored.
- `crates/quip-contracts/`: serde types shared by the Tauri app and sidecar.
  `PredictionResult::validate()` enforces candidate count, uniqueness, and
  non-empty strings. The round-trip test reads the shared fixture file.
- `docs/phase-0.schema.json`: authoritative JSON Schema for prediction,
  capture, and health values.
- `docs/fixtures/phase-0-examples.json`: handshake fixtures. These are protocol
  examples, not training data and not evidence of trained-model behavior.
- `artifacts/models/`: ignored location for local model files; only `.gitkeep`
  belongs in Git.

### Tauri/Rust application: `src-tauri/src/`

- `main.rs`: application entry point and integration owner. It initializes the
  data directory and logging, owns the mutex-wrapped `Engine`, builds the tray,
  exposes Tauri commands, emits state/settings/metrics events, positions the
  suggestion bar, runs comparisons, and contains fixture/live self-tests.
- `composition/mod.rs`: authoritative app state machine. It constructs bounded
  requests, chooses the configured backend, contract-validates metrics, rejects
  stale results, handles highlight/selection/dismissal, and records learning.
- `inference/mod.rs`: app-side deterministic backend and demo corpus. It merges
  shared fixtures with `src-tauri/fixtures/demo_corpus.json`, simulates Base
  over-editing, performs fixture personal-pattern substitution, and tracks
  request/error/schema/latency metrics.
- `inference/sidecar_client.rs`: lazy persistent child-process client. It sends
  one NDJSON command per line over stdin/stdout, resets the child after a
  transport failure, and resolves the binary from `QUIP_INFERENCE_SIDECAR`, a
  sibling executable, or `target/debug`.
- `ime_bridge.rs`: loopback-only NDJSON bridge from the standalone Swift
  InputMethodKit source. It turns stable native captures into the existing
  composition flow and returns navigation, dismissal, and explicit commit
  messages to the verified active text client.
- `accessibility/mod.rs`: one-shot macOS AX capture and destination registry.
  Supported bundle IDs are TextEdit, Notes, Vivaldi, Chrome, and Safari. Secure,
  unsupported, untrusted, and non-editable destinations return unavailable
  reasons rather than reaching inference.
- `commit/mod.rs`: explicit-confirmation commit path. It tries AX selected-text
  insertion first, then restores focus/range and uses `osascript` Command-V with
  `arboard` clipboard preservation. Only plain-text clipboard snapshots are
  supported by the fallback.
- `learning/mod.rs`: local profile store under `<data-dir>/profiles`. It seeds
  `profile_a`, `profile_b`, and `profile_default`; writes `patterns.json` and
  append-only `examples.jsonl`; sends patterns after count 2; and limits a
  request to the strongest 8 patterns.

### Native macOS input source: `native/quip-ime/`

- `controller.swift`: pass-through InputMethodKit controller. It tracks an
  80-UTF-16-unit burst, verifies selection/caret state, handles Tab, number
  keys, arrows, and Escape, and applies only explicit commits.
- `engine_bridge.swift`: reconnecting loopback client for the Tauri bridge at
  `127.0.0.1:48731`.
- `server.swift`, `client_shim.*`, and `Info.plist`: standalone input-method
  process, Objective-C text-client calls, and macOS input-source metadata.
- `scripts/build-native-input-method.sh`: builds a staged bundle without
  changing system state.
- `scripts/install-input-method.sh`: signs, backs up, installs, and registers
  the bundle; it requires administrator approval and manual enablement.
- `settings/mod.rs`: persisted `settings.json` with enabled, context, learning,
  profile, backend, and model variant fields. Environment overrides affect
  backend and variant for the process without rewriting the file.

### Tauri webviews: `src/ui/`

- `contracts.ts`: TypeScript mirror of the shared JSON schema. Keep it aligned
  with `quip-contracts` and the schema; it is not independently authoritative.
- `ipc.ts`: typed Tauri commands and event listeners. App-only `Snapshot`,
  metrics, settings, comparison, and commit types also live here.
- `suggestions.html` / `suggestions.ts`: pure renderer for candidates or an
  error chip. It ignores `predicting` snapshots so existing candidates do not
  flicker while a refresh runs. Clicks select candidates.
- `settings.html` / `settings.ts`: settings controls plus pattern inspection and
  profile reset.
- `demo.html` / `demo.ts`: in-app development harness. It simulates a textbox,
  tests multiple inference cadence strategies, maintains a trailing five-word
  / 80-character burst window, supplies caret geometry for the demo window,
  handles candidate keys, displays health/metrics, injects capture fixtures,
  and runs deterministic corpus comparisons.
- `style.css`: styles all three pages, including the dark transparent IME bar.
- `vite.config.ts`: treats the three HTML files as separate Vite entry points
  served on port 1420 in development.
- `src-tauri/tauri.conf.json`: declares the hidden `suggestions`, `settings`, and
  `demo` windows. The suggestion window is transparent, always-on-top, and
  explicitly non-focusable.

### Inference sidecar: `src-tauri/sidecars/inference/`

- `src/protocol.rs`: long-lived NDJSON server with `health` and `predict`
  operations. Malformed commands return a protocol error and do not kill the
  process.
- `src/fixture.rs`: exact shared-fixture replay. It semantically matches all
  request fields except the request ID, then rewrites results with the caller's
  ID. This is distinct from the richer app-side `FixtureBackend`.
- `src/live.rs`: loopback-only HTTP client for an externally started
  `mistral.rs` OpenAI-compatible server. It uses the shared system prompt,
  non-thinking mode, JSON-schema output, temperature 0.1, and five completions.
  It retries incomplete batches up to three attempts, filters exact input and
  obvious scaffolding/truncation, then vote-ranks results.
- `src/main.rs`: starts fixture mode by default or live mode with `--live`.
- `src/bin/quip-phrase-tester.rs`: interactive or one-shot Base/Global CLI.
  Fixture mode only knows shared prerecorded phrases; live mode runs Base and
  reports the expected unloaded Global adapter.
- `scripts/run-live-phrase-tester.sh`: starts/reuses local Qwen and runs the
  live CLI.
- `scripts/run-live-app.sh`: starts/reuses local Qwen, builds the sidecar, and
  launches Tauri with process-only `live` / `base` overrides.
- `README.md`: operator-facing sidecar protocol and launch notes.

The sidecar does not embed `mistral.rs`. The shell scripts start a loopback
server (default `127.0.0.1:1234`), and the Rust sidecar speaks HTTP to it. The
sidecar itself then speaks NDJSON to the app.

### Freesolo training: `training/flash/`

- `system_prompt.txt`: one prompt shared by training, managed evaluation, the
  Windows playground, benchmarking, and local live inference. Prompt changes
  are cross-runtime contract changes.
- `scoring.py`: strict one-key output parser and reward. A response succeeds
  only when it is schema-valid, makes the correct keep/change decision, and its
  normalized suggestion is one of `metadata.accepted_suggestions`.
- `environment.py`: `QuipEnvironment`, a Freesolo single-turn environment that
  yields the selected split and delegates reward to `score_completion`.
- `augmentation.py`: deterministic US-QWERTY mutations: substitution, deletion,
  insertion, transposition, repeat, and spacing. Defaults are relative lab
  weights, not claims about population error frequency.
- `dataset_compiler/contract.py`: executable V0 quotas, row parsing,
  confidentiality/content filters, provenance validation, split leakage checks,
  reward checks, and build-report hash checks.
- `dataset_compiler/sources.py`: downloads/caches the pinned MASSIVE archive,
  checks archive/member hashes, parses en-US records, rejects unsuitable source
  utterances, and samples at most one window per source record.
- `dataset_compiler/compiler.py`: builds deterministic splits and the report.
  Current quotas are 2,000 train / 200 eval / 200 test; each split is evenly
  divided across one-to-five-word windows, with 10% unchanged and 90% carrying
  one deterministic augmentation event within every window size.
- `dataset/`: checked-in compiled JSONL, source identity, attribution, and build
  facts. MASSIVE train maps to Quip train, dev to eval, and test to locked test.
- `configs/`: Flash run configurations. `sft-smoke.toml` is the short plumbing
  check; `sft-v0-base.toml` and `sft.toml` are SFT lanes; `opd.toml` is a
  fallback; `rl.toml` is warm-start GRPO only after SFT beats Base.
- `scripts/validate_datasets.py`: full sourced-corpus validation by default;
  `--smoke` accepts smaller integration corpora.
- `scripts/build_datasets.py`: deterministic rebuild or `--verify-only` check.
  Rebuilding intentionally rewrites the checked-in splits and report.
- `scripts/run_managed_eval.py`: runs a Base model or deployed adapter through
  Freesolo serving and writes prediction JSONL outside the source dataset.
- `scripts/evaluate_predictions.py`: reports schema validity, change accuracy,
  decode success, unnecessary edit rate, overall success, latency, and category
  results.
- `benchmarks/models.toml`, `benchmarking.py`, `scripts/run_benchmark.py`, and
  `benchmark_dashboard.py`: common held-out model matrix, guarded transports,
  cost/latency capture, scoring, Markdown summary, and interactive HTML report.
  Generated benchmark outputs belong under ignored `artifacts/eval/`.
- `prototype/`: localhost-only Windows/WSL developer playground. Its
  augmentation tab is fully local; its model tab calls Freesolo managed serving
  for five completions. It is not part of the macOS app and never sends the
  Flash credential to the browser.
- `tests/`: Python unit/contract coverage for augmentation, datasets, scoring,
  environment behavior, evaluation, benchmarking, and the prototype server.
- `uv.lock`: pinned Python resolution. Flash itself must run inside Ubuntu WSL2
  on Windows because its dependency path imports POSIX-only `fcntl`.

### Repository validation skills

- `.agents/skills/validate-quip-sidecar/`: deterministic and live process-level
  sidecar validators.
- `.agents/skills/validate-quip-context/`: real TextEdit/Notes/Chrome Accessibility
  context capture through the native IME bridge.
- `.agents/skills/run-freesolo-flash-wsl/`: exact Windows/WSL setup,
  authentication, dry-run, training, evaluation, and export workflow.

When a task matches one of these skills, read its `SKILL.md` completely and use
it. Do not replace the skill's process checks with unit tests.

## Shared runtime contract and invariants

Only three boundary families are shared across workstreams:

- `PredictionRequest` / `PredictionResult`: orchestration to inference.
- `CaptureResult`: Accessibility to composition/UI.
- `SidecarHealth`: inference to health UI.

Keep these invariants intact:

- `model_variant` (`base`, `global`, `global_plus_personal`) identifies model
  intent. `backend` (`fixture`, `live`) independently identifies the source of
  a successful answer.
- Requests contain a bounded full draft, context snippets, and compact personal
  patterns. Internal element handles, adapter paths, and raw completions do not
  cross the boundary.
- A model completion contains exactly one full-input suggestion. The result has
  no `action` field and candidates are never partial edits.
- Exactly five internal completions become zero to five unique changed
  candidates. Exact draft results are removed. Vote count wins; earliest
  completion breaks a tie.
- A successful empty candidate list is normal skip behavior, not an error.
- An error is explicit and never invents candidates.
- The typed text is already present and must not be repeated as a keep option.
- Capture `destination_id` is opaque outside Accessibility/commit code.
- Secure or unsupported captures never reach inference.
- Only explicit candidate selection may replace destination text.

When changing a shared contract, update all of these in one change:

1. `docs/phase-0-contracts.md` for meaning.
2. `docs/phase-0.schema.json` for shape.
3. `docs/fixtures/phase-0-examples.json` for representative values.
4. `crates/quip-contracts/src/lib.rs` and its round-trip tests.
5. `src/ui/contracts.ts` and any app IPC/state mirrors.
6. Both fixture producers/consumers and sidecar process tests.
7. The `validate-quip-sidecar` process validator.

## Local state, environment variables, and artifacts

The default Tauri app data directory contains:

```text
settings.json
logs/quip.log.<date>
profiles/<profile-id>/patterns.json
profiles/<profile-id>/examples.jsonl
```

Useful development variables:

- `QUIP_DATA_DIR`: isolate settings, profiles, and logs in a chosen directory.
- `QUIP_SHOW=demo,settings`: show normally hidden windows at startup.
- `QUIP_DEMO_CAPTURE=1`: inject one fixture capture after startup.
- `QUIP_SELFTEST=1`: run the fixture app self-test and exit.
- `QUIP_SELFTEST_LIVE=1`: run the live app/sidecar/Qwen self-test and exit.
- `QUIP_BACKEND_MODE=fixture|live`: process-only backend override.
- `QUIP_MODEL_VARIANT=base|global|global_plus_personal`: process-only variant
  override.
- `QUIP_INFERENCE_SIDECAR`: explicit sidecar executable path.
- `QUIP_MODEL_ADDR`: loopback model-server address; defaults to
  `127.0.0.1:1234` and rejects non-loopback addresses.
- `QUIP_BASE_MODEL_ID`: model used by launch scripts; defaults to
  `Qwen/Qwen3.5-2B`.
- `QUIP_BASE_MODEL_QUANT`: launch-script UQFF quantization level; defaults to 4.
- `MISTRALRS_BIN`: explicit local `mistral.rs` executable for launch scripts.

Never commit model binaries, adapters, secrets, personal records, generated
predictions, evaluation artifacts, or logs. Keep models under ignored
`artifacts/models/`, adapters under `artifacts/adapters/`, evaluations under
`artifacts/eval/`, and temporary runtime state outside the repository. The
root `.gitignore` ignores Markdown by default except selected documentation
locations; put maintained project documentation under `docs/` or explicitly
adjust ignore rules.

## Common development commands

Run commands from the repository root unless a command changes directory.

Install/build the webview and check the Rust workspace:

```bash
npm ci
npm run build
cargo fmt --all -- --check
cargo test --workspace
```

Launch the default fixture-backed app. It is tray-only, so use the tray menu or
show development windows explicitly:

```bash
npm run tauri dev
QUIP_SHOW=demo,settings npm run tauri dev
```

Run the fixture sidecar or phrase tester without installing a model:

```bash
cargo run -p quip-inference-sidecar
cargo run --quiet -p quip-inference-sidecar --bin quip-phrase-tester -- "cnt cm tmrw"
```

Run the real deterministic sidecar integration:

```bash
.agents/skills/validate-quip-sidecar/scripts/validate.sh
```

Run local live Base inference on macOS after `mistral.rs` is installed:

```bash
src-tauri/sidecars/inference/scripts/run-live-phrase-tester.sh "see you tomorow"
src-tauri/sidecars/inference/scripts/run-live-app.sh
```

Build the native input source, or install it when system changes are intended:

```bash
npm run build:input-method
npm run install:input-method
```

After changing live inference behavior, run the full live validator required by
the repository skill:

```bash
.agents/skills/validate-quip-sidecar/scripts/validate-live.sh
```

After changing native keyboard or Accessibility context ingestion, run:

```bash
.agents/skills/validate-quip-context/scripts/validate.sh
```

With the Python environment installed, check training code from
`training/flash`:

```bash
cd training/flash
python -m pytest
python scripts/validate_datasets.py
python scripts/build_datasets.py --verify-only
python scripts/run_benchmark.py --dry-run
```

`run_benchmark.py --dry-run` can validate the Freesolo catalog and sends no
inference requests. Any managed evaluation, training, deployment, or export
requires the WSL skill on Windows, authentication checks, non-sensitive data,
and the approval rules below.

## Validation matrix

Unit tests are necessary but are not completion evidence for behavior that
crosses a real process, application, model, or OS boundary.

| Change | Minimum evidence before claiming completion |
| --- | --- |
| TypeScript/webview only | `npm run build`; inspect the affected Tauri window when visual behavior changed |
| Rust state/settings/learning only | `cargo fmt --all -- --check` and `cargo test --workspace`; run the app self-test if the composition flow changed |
| Shared prediction contract, fixture backend, sidecar protocol/client, health, phrase tester | Use `$validate-quip-sidecar`; success ends with `Quip sidecar integration passed` |
| Live backend, model prompt, normalization, sidecar lifecycle, live app conversion | Use both deterministic and live sidecar validation; live success ends with `Quip live inference integration passed` |
| Native InputMethodKit capture/commit | Build the standalone bundle, run the live sidecar validator's native-bridge round trip, then test the installed source in at least TextEdit and the selected browser before claiming OS-client compatibility |
| Accessibility existing-text/context | Use `$validate-quip-context`; success ends with `Quip native context integration passed` after real TextEdit, Notes, and Chrome captures |
| Training augmentation/scoring/environment | Run the targeted Python tests and the full Python suite |
| Dataset policy/compiler/source | Rebuild deterministically when intended, validate exact quotas/provenance/hashes, and inspect the diff in all split/report files |
| Freesolo environment/config/training/evaluation/export | Use `$run-freesolo-flash-wsl`; report versions, identity without secrets, dry-run/cost, run/checkpoint IDs, held-out metrics, inspected failures, and export state |

For any behavior change without an applicable real integration skill, create
the skill before claiming completion. Report the visible outcome and relevant
logs, not merely the command's exit code.

## Safe change playbooks

### Composition or candidate behavior

- Preserve the IME principle: typed text is already committed and doing nothing
  keeps it.
- Keep the engine authoritative; webviews render snapshots and call commands.
- Never hold the engine mutex across model I/O or a simulated latency sleep.
- Keep stale-result rejection by burst ID.
- Do not hide the current bar merely because the next request is predicting.
- Explicit selection can learn a `replace`; only a stable dismissal of visible
  real candidates can learn `keep`.
- Verify zero candidates, errors, selection, dismissal, supersession, and
  sentence boundaries separately.

### Inference or prompt behavior

- Keep `training/flash/system_prompt.txt` shared rather than copying prompts.
- Keep loopback binding. Local debug events intentionally log drafts, context, and output for inspection; never commit or share those logs.
- Preserve non-thinking mode and strict JSON schema.
- Request exactly five completions and require a complete batch.
- Aggregate outside the model: exact-input filter, safety filters,
  deduplication, votes, earliest-index tie-break, maximum five.
- Do not make Global silently behave like Base when its adapter is missing.
- Fixture fallback must remain available after live changes.

### Accessibility or commit behavior

- Check process trust before reading destinations.
- Reject secure fields, unsupported apps, and non-editable controls before
  inference.
- Keep native AX objects and ranges inside the Accessibility module; expose only
  opaque IDs and contract values.
- Capture and later restore the exact app, element, focus, selection, and burst
  range. Never assume the currently focused field is still the destination.
- Commit only on explicit selection. Dismiss/cancel/zero candidates must not
  write text.
- Clipboard fallback must preserve and restore the prior clipboard even on
  paste failure; expand clipboard support deliberately rather than discarding
  non-text content.

### Learning or profile behavior

- Keep records profile-scoped and local.
- Do not treat every keystroke as a label.
- Avoid ambient context in training records unless a confirmed labeled example
  genuinely needs it.
- Keep request patterns bounded and thresholded.
- Reset must remove examples and learned patterns; demo reseeding is intentional
  for the current judged build.
- Adapter training is remote Freesolo training, but adapter inference remains
  local. Do not add remote app inference as a shortcut.

### Training data or evaluation

- Never restate or silently change data quotas outside
  `docs/training-data-contract.md` and the executable contract.
- Preserve MASSIVE official partition identity and family separation.
- Never train on eval/test inputs or targets. Use eval for iteration and the
  locked test split only for selected final comparisons.
- Keep clean source as the target and record deterministic augmentation
  provenance.
- Report correction success separately from unnecessary edit rate, plus schema
  validity and latency. An optimized reward is not the held-out evaluation.
- Select a checkpoint from measured held-out results and inspected failures;
  never automatically choose the last checkpoint.
- Export the selected adapter promptly to a team-owned repository and verify its
  immutable identity/checksum before local integration.

## Privacy, cost, and Git guardrails

- This is a hackathon prototype, not a production privacy guarantee. Describe it
  accurately.
- Never use credentials, secrets, or obviously sensitive personal data in
  fixtures, datasets, prompts, examples, screenshots, or demo text.
- Never print, pipe, store, or commit a Freesolo or Backboard key. `.env` is
  ignored; `.env.example` contains placeholders only.
- On Windows, perform Freesolo operations through Ubuntu WSL2 using
  `$run-freesolo-flash-wsl`; do not patch the installed package to bypass
  `fcntl` failures.
- Exercise Freesolo purposefully, including practical limits, but run a dry-run
  and cost estimate and obtain approval before any paid training submission.
- Do not commit model binaries, adapters, generated logs, evaluation responses,
  local profile state, or caches.
- Preserve unrelated work in a dirty worktree. Several workstreams share these
  modules; inspect the diff before editing and never discard another person's
  changes.

## Definition of done

A change is done only when:

1. It follows the authority order and preserves the shared invariants.
2. Code, docs, fixtures, and mirrored types affected by the change agree.
3. The smallest relevant tests pass.
4. The real boundary validator for the changed behavior passes and its visible
   result/logs were inspected.
5. Known prototype limitations are reported honestly; a fixture or headless
   self-test is not presented as proof of an unexercised macOS/model boundary.
6. No secrets, personal data, model artifacts, adapters, logs, or generated
   evaluation output entered the commit.
