# Quip specification

## Product

Quip is a fast, local macOS composition layer for compressed, misspelled, or phonetic English. It learns each user's confirmed language patterns and uses relevant text from open windows as temporary context.

The Freesolo-trained model should beat base Qwen by decoding noisy text while making fewer unnecessary edits. The live comparison reports unnecessary edit rate, protected-token preservation, shorthand and phonetic decoding, and latency.

### Prototype data posture

- The hackathon validates behavior and technical feasibility; it does not claim a production-grade privacy architecture.
- Inference targets the Mac and remains available offline after model installation.
- The Windows model playground uses Freesolo managed serving only for training iteration and checkpoint inspection. This remote prototype path does not change the actual Quip product, which runs inference locally on the Mac.
- Freesolo trains both the global adapter and separate per-user adapters. User-confirmed interactions may become a substantial source of training data.
- Quip turns confirmed interactions into a deduplicated profile dataset, submits a profile run, then downloads the adapter for local inference.
- Prefer excluding ambient open-window context from profile training unless a confirmed labeled example needs it. This is a prototype default, not a blocking requirement.
- Never use credentials or obviously sensitive personal data in the hackathon datasets or demo.

## Experience

Quip runs as a menu-bar app and, when enabled, augments typing in place the way an input method does (the model is Sogou and the macOS Pinyin IME): the user types in their own textbox and a small candidate bar floats above the caret when Quip has a suggestion.

### Composition flow

1. Detect a supported editable field. Keystrokes pass through untouched; the destination receives the text as typed.
2. Observe the writing burst and the caret position passively through Accessibility.
3. Predict continuously while the burst grows, debounced just enough to avoid churn; punctuation and the draft-window cap fire immediately. Stale results are dropped, and the bar refreshes in place rather than flickering.
4. Add relevant open-window context and learned user patterns.
5. Show up to five deduplicated and ranked changed suggestions as numbered candidates in a small bar directly above the caret. The bar never takes keyboard focus, and it keeps its current candidates visible while the next predictions compute.
6. If every suggestion equals the input, the inference layer skips them and shows no candidate. The typed text stands and the user is not interrupted.
7. While candidates are visible, keys 1 through 5 or a click select one, Tab accepts the highlighted candidate, and Escape dismisses the bar. Any other key types through and the bar refreshes with the growing burst. Space stays an ordinary character because English needs it, so Tab plays the role Space has in the Pinyin IME.
8. A sentence boundary (whitespace after a terminator, or a newline) closes the composition session: visible candidates count as a stable dismissal and the next keystroke starts a fresh burst.

Keeping the typed text is always available by doing nothing; there is no separate exact-draft option because the draft is already committed keystroke-by-keystroke by the user themselves.

Initial values are a 150 ms live-prediction debounce and an 80-character draft window; both require testing against real inference times.

Quip covers both ordinary mistakes such as `instaed` and compressed phrases such as `cnt cm tmrw`, with a stricter confidence threshold for single-word corrections.

### Existing-text mode

A global shortcut can run a prediction over selected existing text. The same candidate bar appears above the selection; accepting a candidate replaces the selection, dismissing changes nothing. This is secondary to the live typing flow.

### Commit path

- Replace the burst range in place: prefer macOS Accessibility selection replacement over the tracked burst range.
- Fall back to simulated select-and-paste when required, preserving and restoring the previous clipboard.
- Never replace destination text without an explicit candidate selection. Dismissal or an unchanged suggestion changes nothing.

## Intelligence

### Model contract

The input contains the bounded draft or selection, relevant window snippets, and compact learned user patterns. The model runs in non-thinking mode. Inference runs exactly five completions, and each returns one full-input suggestion:

```json
{ "suggestion": "best full text" }
```

The inference layer compares the five completions with the input, drops exact matches, deduplicates changed suggestions, and ranks them by vote count with earliest completion as the tie breaker. It exposes zero through five candidates. The typed text is never repeated as a candidate because doing nothing already preserves it. The shared wire invariants live in `docs/phase-0-contracts.md`.

Protected tokens include names, paths, filenames, commands, URLs, identifiers, version strings, and intentional slang, including examples such as `usr/bin` and `q3_finl_v2.pdf`. Preservation remains orthogonal evaluation coverage, not a model action or a fixed training quota.

### Window context

For each prediction, Quip may collect the application name, window title, and a bounded accessible visible-text snippet from open windows. It ranks windows locally by focus, recency, and relevance, then passes only the most relevant snippets to the model.

The hackathon build uses Accessibility text, not screenshots or OCR. It never reads secure fields or excluded applications. Context is not persisted or added to personal training unless a confirmed interaction produces a compact labeled example. A menu-bar switch disables window context.

### Per-user learning

Every user starts with the frozen global Freesolo adapter and receives a separate LoRA adapter trained through Freesolo and exported back to the Mac. The personal dataset records only useful labels from:

- confirmed candidates
- dismissed suggestions when the surrounding text remains unchanged, which may become identity-target examples
- corrections made immediately after a Quip commit
- repeated personal abbreviations, names, vocabulary, and expansions

Quip does not treat every keystroke as a training label. It collects useful confirmed interactions, deduplicates repeated patterns, and can build a substantial per-user dataset for Freesolo. Prefer excluding ambient window text unless it is needed for a confirmed labeled example. Before enough examples exist for training, a compact local pattern dictionary provides immediate personalization.

If the runtime cannot stack adapters, Quip merges the global adapter into the base once and loads one user adapter over it. Users can pause learning, inspect stored patterns, or reset their local adapter and records.

## Training

### Global Freesolo adapter

The initial corpus is correction-heavy and includes identity targets for slang, names, abbreviations, filenames, commands, URLs, versions, and ambiguous fragments. Identity targets use the same suggestion contract and are not a separate action. Executable row quotas live in `training/flash/dataset_compiler/contract.py`; the checked-in JSONL files are smoke fixtures, not the completed corpus.

Most correction rows are generated deterministically from correct US QWERTY text. Operators include adjacent substitution, deletion, nearest-key insertion, adjacent transposition, repeat, and spacing changes. Each generated row records its seed and operator provenance. A small optional LLM or teacher lane covers semantic shorthand and phonetic forms that mechanical augmentation cannot produce. Clean phrase and pair sources remain under research; every source must be pinned, licensed, attributed, filtered, and split by source family before use.

Baseline Qwen3.5, run SFT, and use development results to choose steps, checkpoint cadence, and whether OPD or warm-started GRPO is worth trying. Evaluate the selected checkpoint once on the locked test split, inspect failures, and export the strongest credible checkpoint rather than automatically choosing the last one.

### Evaluation

Keep iterative development evaluation separate from the locked final comparison. Evaluation is more identity-heavy than training so unnecessary edits are visible, but Quip does not claim to know the exact natural identity prior. Report correction accuracy separately from false-correction rate, plus category results, schema validity, protected-token preservation, and latency. Deduplicate normalized patterns, separate source families across splits, and exclude evaluation prompts and close variants from training. Any later optimized reward remains distinct from this evaluation.

### Artifacts

Export the chosen global adapter or checkpoint immediately to a team-owned Hugging Face repository with `flash export --adapter-id <run-id> --repository <owner>/<repo>`. Undeployed, inactive run artifacts may be garbage-collected after about seven days. Managed deployment is optional for inspection and is not part of Quip's local product path.

Each user's separate adapter is trained through a private Freesolo run, exported to the Mac, and applied by the local runtime alongside the global adapter. The Flash catalog includes Qwen3.5 checkpoints at 0.8B, 2B, 4B, and 9B parameters.

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
- A profile-training client packages compact confirmed examples, submits a private Freesolo run, and downloads the exported per-user adapter. It never provides remote inference.

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

1. Flash environment, global and per-user training, evaluation, and checkpoint comparison.
2. Local inference, adapter composition, and packaging.
3. Keyboard capture, Accessibility, destination commits, and window context.
4. Tauri box, personal pattern storage, demo harness, and integration.

Reserve a final phase for compatibility testing, rehearsal, and fallback recording.

### Judged build and live demo

The build is complete when it can:

1. Observe a writing burst typed directly into TextEdit and one browser input, and place the candidate bar at the caret without stealing focus or altering the typed text.
2. Run base Qwen and the global Freesolo adapter locally on the same input with schema-valid output.
3. Show nothing when the suggestion equals the input, and replace the burst in place only on an explicit candidate selection; dismissal changes nothing.
4. Show base Qwen over-editing protected text while the trained model preserves it.
5. Compare base and trained outputs on noisy shorthand or a typo, with the trained model producing the minimal useful correction.
6. Use accessible text from an open window to resolve an ambiguous prediction.
7. Show two local user profiles producing different candidates from learned patterns.

The comparison runs live from a deterministic corpus rather than a recording. The presentation also shows the Flash environment, training configuration, checkpoint evaluation, and exported adapter.

Automatic per-user retraining, selection-based replacement, broader application compatibility, and packaging polish are stretch goals. The judged build may use two profile adapters trained through Freesolo from prepared compact interaction datasets.
