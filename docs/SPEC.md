# Quip specification

## Product

Quip is a fast, local macOS composition layer for compressed, misspelled, or phonetic English. It learns each user's confirmed language patterns and uses relevant text from open windows as temporary context.

The Freesolo-trained model should beat base Qwen by decoding noisy text while making fewer unnecessary edits. The live comparison reports unnecessary edit rate, protected-token preservation, shorthand and phonetic decoding, and latency.

### Locality contract

- All inference runs on the Mac and remains available offline after model installation.
- The Qwen base, global Freesolo adapter, per-user adapters, and personal training records stay local.
- Temporary drafts, selections, prompts, context, and outputs never go to a remote inference service.
- Freesolo uses a prepared project dataset to train and export the global adapter. Managed endpoints are limited to training inspection and debugging.
- Per-user training runs locally from confirmed usage patterns.
- Open-window context is processed in memory and is not uploaded or retained by default.

## Experience

Quip runs as a menu-bar app and, when enabled, augments typing in place the way an input method does (the model is Sogou and the macOS Pinyin IME): the user types in their own textbox and a small candidate bar floats above the caret when Quip has a suggestion.

### Composition flow

1. Detect a supported editable field. Keystrokes pass through untouched; the destination receives the text as typed.
2. Observe the writing burst and the caret position passively through Accessibility.
3. After punctuation, Return, or an idle pause, run one prediction for the burst rather than one per character.
4. Add relevant open-window context and learned user patterns.
5. On a `replace` result, show up to three numbered candidates in a small bar directly above the caret. The bar never takes keyboard focus.
6. On a `keep` result, show nothing. The typed text stands and the user is not interrupted.
7. Selecting a candidate (number key or click) replaces the just-typed burst in the destination in place. Pressing Escape or simply continuing to type dismisses the bar and changes nothing.

Keeping the typed text is always available by doing nothing; there is no separate exact-draft option because the draft is already committed keystroke-by-keystroke by the user themselves.

Initial values are a 400 ms idle trigger and an 80-character draft window. The pause is kept short because model inference latency stacks on top of it before the bar can appear; both values require testing against real inference times.

Quip covers both ordinary mistakes such as `instaed` and compressed phrases such as `cnt cm tmrw`, with a stricter confidence threshold for single-word corrections.

### Existing-text mode

A global shortcut can run a prediction over selected existing text. The same candidate bar appears above the selection; accepting a candidate replaces the selection, dismissing changes nothing. This is secondary to the live typing flow.

### Commit path

- Replace the burst range in place: prefer macOS Accessibility selection replacement over the tracked burst range.
- Fall back to simulated select-and-paste when required, preserving and restoring the previous clipboard.
- Never replace destination text without an explicit candidate selection. Dismissal and `keep` change nothing.

## Intelligence

### Model contract

The input contains the bounded draft or selection, relevant window snippets, and compact learned user patterns. The model runs in non-thinking mode. SFT learns the JSON contract from gold outputs because Flash rejects `structured_outputs` for SFT; GRPO constrains sampled rollouts with `train.structured_outputs`, and the local runtime enforces the same schema at inference.

Valid outputs are:

```json
{ "action": "keep", "candidates": [] }
```

```json
{ "action": "replace", "candidates": ["best candidate"] }
```

`action` is exactly `keep` or `replace`. `keep` has no candidates; `replace` has one to three, ordered best first. Each candidate replaces the full bounded input. The model never returns the typed draft as a candidate; keeping the typed text is the do-nothing default in the UI.

Protected content includes paths, filenames, names, commands, URLs, identifiers, version strings, and intentional slang, including examples such as `usr/bin` and `q3_finl_v2.pdf`.

### Window context

For each prediction, Quip may collect the application name, window title, and a bounded accessible visible-text snippet from open windows. It ranks windows locally by focus, recency, and relevance, then passes only the most relevant snippets to the model.

The hackathon build uses Accessibility text, not screenshots or OCR. It never reads secure fields or excluded applications. Context is not persisted or added to personal training unless a confirmed interaction produces a compact labeled example. A menu-bar switch disables window context.

### Per-user learning

Every user starts with the frozen global Freesolo adapter and receives a separate local LoRA adapter. The personal dataset records only useful labels from:

- confirmed candidates
- dismissed suggestions when the surrounding text remains unchanged, which become `keep` examples
- corrections made immediately after a Quip commit
- repeated personal abbreviations, names, vocabulary, and expansions

Quip does not store every keystroke. It appends compact labeled interactions, deduplicates repeated patterns, and periodically refreshes the user adapter while idle. Before enough examples exist for training, a compact local pattern dictionary provides immediate personalization.

If the runtime cannot stack adapters, Quip merges the global adapter into the base once and loads one user adapter over it. Users can pause learning, inspect stored patterns, or reset their local adapter and records.

## Training

### Global Freesolo adapter

Scaffold one single-turn, non-reasoning Flash environment. Its `EnvironmentSingleTurn` builds prompt messages and returns a `RewardResult` from `score_response`. Package separate `dataset/train.jsonl` and `dataset/eval.jsonl` files and select them through `environment.params.split`.

Every dataset row uses the exact keys `input`, optional `output`, and optional `metadata`; Flash drops other top-level keys. SFT learns from JSON gold values in `output`. GRPO samples from `input`, while gold references and scorer-only fields belong in `output` or `metadata`.

Training proceeds as follows:

1. Baseline the selected Qwen3.5 checkpoint on the held-out split.
2. Run SFT with a positive `train.max_examples` on hundreds of clean JSON-output examples to teach judgment and the output contract.
3. Evaluate the SFT adapter on the untouched split.
4. If useful, warm-start GRPO with `init_from_adapter`, omit LoRA rank and alpha, and constrain rollouts with the Quip JSON schema.
5. Use `max_steps` and `save_at_steps` for required checkpoint boundaries. Run `flash train config.toml --dry-run` and `--cost` before submission.
6. Inspect high-reward traces, evaluate useful checkpoints, and ship the best held-out checkpoint rather than automatically choosing the last one.

OPD is a fallback only if a stronger teacher first beats the student on the held-out task.

### Reward and evaluation

The GRPO reward combines schema validity, contract consistency, correct `keep` decisions, protected-token preservation, acceptable gold decodings, minimal edits, and penalties for commentary, invention, or excessive rewriting.

The held-out evaluation remains distinct from the optimized reward and reports category-level errors. Start with several hundred checked examples, expand toward a few thousand only for real coverage gaps, deduplicate by normalized pattern, exclude evaluation prompts and close variants from training, and rerun important evaluations to establish the noise floor.

### Artifacts

Export the chosen global adapter or checkpoint immediately to a team-owned Hugging Face repository with `flash export --adapter-id <run-id> --repository <owner>/<repo>`. Undeployed, inactive run artifacts may be garbage-collected after about seven days. Managed deployment is optional for inspection and is not part of Quip's local product path.

Each user's separate adapter is trained locally, and both adapters are applied by the local runtime. The Flash catalog includes Qwen3.5 checkpoints at 0.8B, 2B, 4B, and 9B parameters.

## Application

### Supported scope

The judged build targets TextEdit, Notes, and standard Chrome or Safari inputs. It does not promise rich browser editors, Google Docs, terminals, PDFs, password fields, canvas editors, unusual Electron controls, or universal macOS compatibility.

### Architecture

- Use Rust for system integration, state, and inference orchestration.
- Use Tauri 2 for the menu bar, settings, the HTML and CSS caret-anchored candidate bar, global shortcut, and clipboard integration.
- Use `axuielement` for process trust, focused elements, text markers, and `AXObserver`; use `objc2` ApplicationServices bindings only for missing coverage.
- Use Accessibility to recognize editable destinations, capture and restore destination state, read selections and bounded window text, observe changes, place the box, and commit confirmed text.
- Use `mistral.rs` with Metal as the leading local inference runtime because it supports Qwen3.5, 4-bit quantization, LoRA merging, strict schemas, and a Rust SDK. Start with a bundled loopback sidecar to isolate model lifecycle failures; direct SDK integration is a later optimization.
- If `mistral.rs` cannot load the exported adapter, use another replaceable local sidecar rather than remote inference.
- A separate bundled local training sidecar may produce per-user LoRA adapters if the Rust inference runtime cannot train them. It receives only the local labeled dataset and writes only the user adapter.

Prove global adapter loading and per-user adapter composition before coupling inference into the Tauri application.

### Hardware and model targets

- Primary demo: M3 Pro with 18 GB unified memory.
- Backup and compatibility target: M4 MacBook Air with 16 GB.
- Start with Qwen3.5-2B at 4-bit quantization.
- Benchmark Qwen3.5-4B at 4-bit on both Macs and adopt it only if its quality gain justifies the latency and memory.
- Start at LoRA rank 32, within Flash's rank caps of 128 for 2B and 64 for 4B.

Prefer the M3 Pro for its additional memory and sustained performance. Use the fanless M4 Air to prove compatibility on a common laptop.

## Delivery

### Parallel work

The trained model and its evaluation remain the central technical contribution. Four builders split into:

1. Flash environment, dataset, training, and checkpoint comparison.
2. Local inference, per-user training, adapter composition, and packaging.
3. Keyboard capture, Accessibility, destination commits, and window context.
4. Tauri box, personal pattern storage, demo harness, and integration.

Reserve a final phase for compatibility testing, rehearsal, and fallback recording.

### Judged build and live demo

The build is complete when it can:

1. Observe a writing burst typed directly into TextEdit and one browser input, and place the candidate bar at the caret without stealing focus or altering the typed text.
2. Run base Qwen and the global Freesolo adapter locally on the same input with schema-valid output.
3. Show nothing for `keep`, and replace the burst in place only on an explicit candidate selection; dismissal changes nothing.
4. Show base Qwen over-editing protected text while the trained model keeps it.
5. Compare base and trained outputs on noisy shorthand or a typo, with the trained model producing the minimal useful correction.
6. Use accessible text from an open window to resolve an ambiguous prediction.
7. Show two local user profiles producing different candidates from learned patterns.

The comparison runs live from a deterministic corpus rather than a recording. The presentation also shows the Flash environment, training configuration, checkpoint evaluation, and exported adapter.

Automatic per-user retraining, selection-based replacement, broader application compatibility, and packaging polish are stretch goals. The judged build may use a pre-trained local user adapter produced from a recorded local interaction dataset.
