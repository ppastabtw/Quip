# Quip specification

## Product statement

Quip is a local macOS composition layer and text decoder for compressed, misspelled, or phonetic English. Its defining behavior is speed. It improves for each user by learning their confirmed language patterns and by using relevant text from their open windows as temporary context.

## Locality contract

- All user-text inference runs on the user's Mac.
- The Qwen base model and Freesolo-trained adapter are stored and executed locally.
- Temporary drafts, selected text, prompts, and model outputs do not leave the Mac during product use.
- The application must remain functional without an internet connection after model installation.
- Freesolo is used for post-training and adapter export, not as the product's inference endpoint.
- Managed endpoints may be used only for training-time inspection or debugging and are not part of the judged product path.
- Freesolo training uses a prepared project dataset and produces the global Quip adapter.
- Per-user training runs on the user's Mac and produces a separate local adapter from that user's confirmed usage patterns.
- Open-window context is processed in memory for the current prediction and is not uploaded or retained by default.

## Core claim

The Freesolo-trained model should outperform base Qwen primarily by making fewer unnecessary edits while still decoding noisy text. The comparison must emphasize:

- unnecessary edit rate
- protected-token preservation
- successful shorthand and phonetic decoding
- latency

## Interaction model

Quip runs as a macOS menu-bar application.

### Intelligent mode

1. When Quip is enabled and a supported editable field has focus, intercept the first printable keystroke.
2. Preserve the destination application, editable element, and insertion point.
3. Route the user's writing burst into Quip's temporary composition box instead of inserting it into the destination.
4. Wait for a completed writing burst rather than running inference per character.
5. Trigger prediction at a text boundary such as punctuation, Return, or an idle pause.
6. Gather bounded, relevant context from accessible open windows.
7. Add the user's learned language patterns.
8. Ask the local model for `keep` or up to three minimal replacement candidates.
9. Show the exact draft as the first commit option alongside any candidates.
10. Commit nothing until the user confirms the exact draft or accepts a candidate.
11. Restore the destination and insert only the confirmed text.

Initial timing proposal: trigger after roughly 700 to 900 ms of inactivity. Initial draft-window proposal: at most 80 characters. Both values require testing.

Intelligent mode covers both:

- ordinary single-word mistakes such as `instaed`
- compressed or phonetic phrases such as `cnt cm tmrw`

Single-word corrections should use a stricter confidence threshold. Shorthand decoding and protected-text restraint remain the primary product story.

### Temporary composition box

The temporary composition box is the primary Quip interaction, not an incidental overlay.

- While Quip is capturing a writing burst, the destination application remains unchanged.
- When Quip is enabled, the temporary box opens automatically when typing begins in a supported editable field.
- The box shows the exact text typed by the user.
- The exact draft is always available as a first-class confirmation choice.
- Model candidates appear beside or below the exact draft as they become available.
- Confirming the exact draft inserts it unchanged.
- Accepting a candidate inserts that candidate instead.
- Cancelling discards the temporary draft and inserts nothing.
- Quip must preserve enough destination state to return focus and commit at the original insertion point.

A `keep` model response does not silently write into the destination. It means the exact draft is the recommended option and still requires user confirmation.

## Per-user learning

Quip starts with the same Freesolo-trained global adapter for every user. It then learns a separate local adapter for each macOS user profile.

### Learning signals

The personal training set is built from deliberate product interactions:

- confirmed replacement candidates
- dismissed suggestions, which become `keep` examples when the surrounding text remains unchanged
- user corrections made immediately after a Quip commit
- repeated personal abbreviations, names, vocabulary, and expansions

Quip must not treat every keystroke as training data. It stores compact input and outcome records only when an interaction supplies a useful label.

### Personal training loop

1. Append labeled interactions to a local per-user dataset.
2. Deduplicate and group repeated patterns.
3. Train or refresh a small per-user LoRA adapter locally when enough new examples accumulate.
4. Keep the global Freesolo adapter frozen.
5. Apply the per-user adapter during local inference.

Training runs as a periodic background job while Quip is idle, not after every correction. Before enough examples exist for useful training, Quip may inject a compact local pattern dictionary into the prompt so personalization improves immediately.

If the runtime cannot stack the global and user adapters, merge the global adapter into the base model once and load the user adapter over that merged model. Each user can pause learning, inspect the stored patterns, or reset their local adapter and training records.

## Intelligent window context

Open windows provide temporary context for ambiguous shorthand, names, projects, and domain vocabulary.

For each prediction, Quip may inspect accessible visible windows and collect:

- application name
- window title
- a bounded visible-text snippet

Quip ranks candidate windows locally by focus, recency, and relevance to the current text. Only the most relevant snippets enter the model context. The hackathon build uses Accessibility text only and does not use screenshots or OCR.

Secure text fields and excluded applications are never read. Window context is not persisted or added to personal training data unless a confirmed Quip interaction creates a compact labeled example. The menu-bar interface provides a visible switch for window context.

### Manual mode

The user can select existing text in a supported application and invoke Quip with a global keyboard shortcut. Quip then loads that selection into the same temporary composition box. Confirming a choice replaces the original selection. This is a secondary path for text that has already been inserted.

## Commit behavior

- Preserve the original destination element and insertion point before the composition box receives input.
- Use macOS Accessibility APIs to insert the confirmed draft or candidate when supported.
- Use simulated paste as a compatibility fallback.
- Preserve and restore the user's previous clipboard contents when using that fallback.
- Never insert or replace destination text without explicit user confirmation.

## Application scope

The hackathon build targets standard editable text fields across a deliberately tested compatibility set. It does not promise universal support.

Required demo targets:

- TextEdit
- Notes
- standard Chrome or Safari text inputs

Rich browser editors, Google Docs, terminals, PDFs, password fields, canvas editors, and unusual Electron controls are outside the reliability promise unless testing proves otherwise.

## Model behavior

Input consists of the bounded temporary draft or explicit selection, relevant open-window context, and a compact representation of learned user patterns.

The model runs in non-thinking mode for minimum latency and compatibility with guided decoding. Flash should constrain every response to a JSON schema from the first generated token.

The initial output contract is:

```json
{
  "action": "keep",
  "candidates": []
}
```

or:

```json
{
  "action": "replace",
  "candidates": ["minimal replacement"]
}
```

Contract rules:

- `action` is exactly `keep` or `replace`.
- `keep` requires an empty candidate list.
- `replace` requires one to three candidates.
- Candidates are ordered best first and generated directly by the model.
- Candidates replace the full bounded input span.

The application always renders the user's exact draft as a commit option, even though the model output contains candidates only when `action` is `replace`.

Protected content includes paths, filenames, names, commands, URLs, identifiers, version strings, and intentional slang. Examples include `usr/bin`, `q3_finl_v2.pdf`, and other text whose unusual form is meaningful.

## Freesolo training plan

This plan follows the workflow in the sponsor's post-training deck: build the environment, baseline on a held-out split, train with measurements between stages, and deploy only when the evaluation improves.

### Environment

Build one Flash environment containing:

- input records for shorthand, phonetic English, ordinary mistakes, clean prose, and protected text
- gold `keep` or replacement outputs for SFT
- a programmatic reward for GRPO
- separate training and held-out evaluation splits

Use that same environment to evaluate the base model, generate inspected rollouts, train adapters, and evaluate checkpoints.

### Training sequence

1. Baseline the selected Qwen3.5 checkpoint on the held-out evaluation set.
2. Run SFT on hundreds of clean examples to teach the exact output contract and core judgment.
3. Evaluate the SFT adapter against the untouched held-out split.
4. If time and results justify it, warm-start GRPO from the SFT adapter to sharpen keep-or-decode decisions.
5. Inspect high-reward traces for reward hacking and evaluate every useful checkpoint.
6. Ship the best held-out checkpoint, not automatically the final checkpoint.

GRPO fits this task because much of the desired behavior is programmatically scorable. OPD remains a fallback if a clearly stronger teacher can first be shown to outperform the student on the held-out task.

### Reward direction

The GRPO reward should combine verifiable components:

- schema validity and contract consistency
- correct `keep` decisions
- protected-token preservation
- match against acceptable gold decodings
- minimality of the proposed edit
- penalty for extra commentary, invention, or excessive rewriting

The held-out evaluation must not be identical to the optimized reward. Evaluation should separately report category-level errors so a rising reward cannot hide unnecessary edits.

### Data discipline

- Start with several hundred representative, carefully checked examples.
- Expand toward a few thousand only to cover meaningful edge cases.
- Deduplicate by normalized content and pattern, not only exact text.
- Keep all evaluation prompts and close variants out of training.
- Rerun important evaluations to establish the noise floor before claiming small gains.

### Training artifact and serving

Flash produces a LoRA adapter over the selected Qwen3.5 base. Export the adapter after training. The macOS application must run the base and adapter locally through a compatible inference runtime. Managed Flash deployment is useful for development checks, but it does not satisfy Quip's local-product claim.

The exported Freesolo adapter is Quip's global adapter. Each user has an additional local adapter trained on that user's labeled interactions. Both adapters and the personal training records remain on the Mac.

The Flash catalog currently presented in the sponsor deck includes Qwen3.5 checkpoints at 0.8B, 2B, 4B, and 9B parameters. Final checkpoint selection depends on the demo Mac's memory and measured latency.

## Demo shape

The live demo should show:

1. The base Qwen model over-editing protected or intentional text.
2. The Freesolo-trained model returning `keep` for that same text.
3. Both models receiving noisy shorthand or a typo.
4. The trained model proposing a minimal useful correction.
5. Relevant text from another open window resolving an otherwise ambiguous correction.
6. Two local user profiles producing different candidates from their learned usage patterns.
7. The destination application remaining unchanged while the user types into Quip's temporary box.
8. The user committing either the exact draft or a candidate at the preserved insertion point.

The evaluation comparison should run live from a deterministic corpus rather than being prerecorded.

The presentation should also show the Flash environment, training configuration, checkpoint evaluation, and resulting LoRA adapter so the post-training contribution is inspectable.

## Current scope boundary

The trained model and its evaluation are the central technical contribution. The macOS application needs a polished, credible cross-application demonstration, but broad editor compatibility is a stretch goal.

Work should split across four builders and four tracks:

1. Flash environment, dataset, training, and checkpoint comparison.
2. Local inference, per-user training, adapter composition, and model packaging.
3. macOS keyboard capture, Accessibility, destination insertion, and open-window context.
4. Tauri overlay, personal pattern storage, demo harness, and integration support.

Automatic routing into temporary composition, exact-draft confirmation, and candidate confirmation are required for the judged build. Selection-based replacement begins only after the primary composition path and local trained-model inference work end to end. Reserve a final phase for integration, compatibility testing, demo rehearsal, and fallback recording.

## Hackathon completion criteria

The judged build is complete when:

1. Quip captures a new writing burst into its temporary composition box in at least TextEdit and one browser without changing the destination.
2. Base Qwen and the Freesolo-trained adapter can both process the same local input.
3. The trained model returns schema-valid `keep` or replacement candidates.
4. The exact draft is always available for confirmation, including when the model returns `keep`.
5. Confirming the exact draft or a candidate inserts only that choice at the preserved destination.
6. A live comparison demonstrates shorthand decoding and protected-text restraint.
7. Accessible text from an open window changes an ambiguous prediction in a useful way.
8. A local user profile demonstrates a learned personal expansion without remote inference.

Automatic per-user retraining, selection-based replacement, additional application compatibility, and packaging polish are stretch goals. The judged build may use a pre-trained local user adapter produced from a recorded local interaction dataset.

## Target hardware

- Primary demo machine: Mac with M3 Pro and 18 GB unified memory.
- Compatibility and backup machine: MacBook Air with M4 and 16 GB unified memory.
- Initial local model target: Qwen3.5-2B at 4-bit quantization.
- Benchmark target: Qwen3.5-4B at 4-bit quantization on both machines before deciding whether its quality gain justifies additional latency and memory.

The primary demo should use the M3 Pro machine for its additional memory and stronger sustained performance. The M4 Air should prove that the scoped product still runs on a common fanless laptop.

## Implementation direction

Quip will use a Rust-first architecture.

### Application shell

Use Tauri 2 for the menu-bar application, settings surface, confirmation overlay, global shortcut, and clipboard integration. Keep the overlay frontend deliberately small. Native behavior and application state live in Rust.

The overlay may use HTML and CSS. This is an accepted implementation choice and does not change the Rust-first architecture boundary.

### macOS integration

Use Rust bindings over macOS Accessibility APIs to:

- request and verify Accessibility permission
- identify supported editable destinations before intercepting printable keyboard events
- inspect the focused application and editable element
- read selected or recent text
- enumerate accessible open windows and read bounded visible text
- preserve and restore the destination element and insertion point
- observe relevant text changes where supported
- insert confirmed text through Accessibility
- locate the selection or caret for overlay placement

The current leading crate is `axuielement`, which exposes `AXUIElement`, process trust, text markers, and `AXObserver` notifications. Drop to `objc2` ApplicationServices bindings only for missing API coverage.

### Local inference

The leading Rust inference candidate is `mistral.rs` with Metal acceleration. It currently advertises Qwen3.5 support, 4-bit quantization, LoRA weight merging, strict schema support, and a Rust SDK.

The first inference milestone is a compatibility spike that loads the exact Freesolo-exported adapter over the selected Qwen3.5 base and produces schema-constrained output on both target Macs. The next spike must prove that a separate per-user adapter can be composed with it. Do this before coupling inference into the application.

For the first demo implementation, Quip may run `mistral.rs` as a bundled local sidecar process and call its loopback API. This preserves a Rust implementation while isolating model startup and inference failures from the overlay. Linking the Rust SDK directly into the application is an optimization after the model path is proven.

### Architecture fallback

If the exported adapter does not work with `mistral.rs` on Metal, keep the macOS application in Rust and temporarily use a replaceable local inference sidecar that can load the artifact. Do not switch the product to remote inference merely to preserve an all-Rust claim.

Local per-user training may use a separate bundled sidecar if the Rust inference runtime cannot train LoRA adapters. The training sidecar receives only the local labeled interaction dataset and writes only the local user adapter.
