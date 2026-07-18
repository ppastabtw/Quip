# Quip specification

## Product statement

Quip is a local macOS text decoder for compressed, misspelled, or phonetic English. Its defining behavior is restraint. It keeps valid or intentionally unusual text unchanged and proposes minimal replacements only when the model is confident that decoding is useful.

## Locality contract

- All user-text inference runs on the user's Mac.
- The Qwen base model and Freesolo-trained adapter are stored and executed locally.
- Selected text, recent text, prompts, and model outputs do not leave the Mac during product use.
- The application must remain functional without an internet connection after model installation.
- Freesolo is used for post-training and adapter export, not as the product's inference endpoint.
- Managed endpoints may be used only for training-time inspection or debugging and are not part of the judged product path.
- Training data is a prepared project dataset, not text collected from Quip users.

## Core claim

The Freesolo-trained model should outperform base Qwen primarily by making fewer unnecessary edits while still decoding noisy text. The comparison must emphasize:

- unnecessary edit rate
- protected-token preservation
- successful shorthand and phonetic decoding
- latency

## Interaction model

Quip runs as a macOS menu-bar application.

### Intelligent mode

1. Observe typing locally.
2. Wait for a completed writing burst rather than running inference per character.
3. Trigger at a text boundary such as a space, punctuation, Return, or an idle pause.
4. Read only a capped recent-text window.
5. Ask the model for `keep` or up to three minimal replacement candidates.
6. Remain silent for `keep`.
7. Require explicit confirmation before replacing text.

Initial timing proposal: trigger after roughly 700 to 900 ms of inactivity. Initial context proposal: at most 80 recent characters. Both values require testing.

Intelligent mode covers both:

- ordinary single-word mistakes such as `instaed`
- compressed or phonetic phrases such as `cnt cm tmrw`

Single-word corrections should use a stricter confidence threshold. Shorthand decoding and protected-text restraint remain the primary product story.

### Manual mode

The user can select text in any supported application and invoke Quip with a global keyboard shortcut. This is the reliable fallback and the first implementation milestone.

## Replacement behavior

- Use macOS Accessibility APIs to read and replace selected or recent text when supported.
- Use simulated copy and paste as a compatibility fallback.
- Preserve and restore the user's previous clipboard contents when using that fallback.
- Never replace text without explicit user confirmation.

## Application scope

The hackathon build targets standard editable text fields across a deliberately tested compatibility set. It does not promise universal support.

Required demo targets:

- TextEdit
- Notes
- standard Chrome or Safari text inputs

Rich browser editors, Google Docs, terminals, PDFs, password fields, canvas editors, and unusual Electron controls are outside the reliability promise unless testing proves otherwise.

## Model behavior

Input consists of a bounded recent-text span or explicit selection.

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

The Flash catalog currently presented in the sponsor deck includes Qwen3.5 checkpoints at 0.8B, 2B, 4B, and 9B parameters. Final checkpoint selection depends on the demo Mac's memory and measured latency.

## Demo shape

The live demo should show:

1. The base Qwen model over-editing protected or intentional text.
2. The Freesolo-trained model returning `keep` for that same text.
3. Both models receiving noisy shorthand or a typo.
4. The trained model proposing a minimal useful correction.
5. The user confirming the candidate and Quip replacing it in another application.

The evaluation comparison should run live from a deterministic corpus rather than being prerecorded.

The presentation should also show the Flash environment, training configuration, checkpoint evaluation, and resulting LoRA adapter so the post-training contribution is inspectable.

## Current scope boundary

The trained model and its evaluation are the central technical contribution. The macOS application needs a polished, credible cross-application demonstration, but broad editor compatibility is a stretch goal.

With four builders and 30 hours, work should split into four tracks:

1. Flash environment, dataset, training, and checkpoint comparison.
2. Local Rust inference compatibility and model packaging.
3. macOS Accessibility, global shortcut, and replacement behavior.
4. Tauri overlay, demo harness, presentation, and integration support.

Manual selection and confirmation are required for the judged build. Intelligent background triggering begins only after the manual path and local trained-model inference work end to end. Reserve the final six hours for integration, compatibility testing, demo rehearsal, and fallback recording.

## Hackathon completion criteria

The judged build is complete when:

1. A global shortcut captures selected text in at least TextEdit and one browser.
2. Base Qwen and the Freesolo-trained adapter can both process the same local input.
3. The trained model returns schema-valid `keep` or replacement candidates.
4. Quip remains silent for `keep` and shows a confirmation overlay for replacements.
5. Confirming a candidate replaces the original text in place.
6. A live comparison demonstrates shorthand decoding and protected-text restraint.

Intelligent background triggering, additional application compatibility, and packaging polish are stretch goals.

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
- inspect the focused application and editable element
- read selected or recent text
- observe relevant text changes where supported
- replace text through Accessibility
- locate the selection or caret for overlay placement

The current leading crate is `axuielement`, which exposes `AXUIElement`, process trust, text markers, and `AXObserver` notifications. Drop to `objc2` ApplicationServices bindings only for missing API coverage.

### Local inference

The leading Rust inference candidate is `mistral.rs` with Metal acceleration. It currently advertises Qwen3.5 support, 4-bit quantization, LoRA weight merging, strict schema support, and a Rust SDK.

The first inference milestone is a compatibility spike that loads the exact Freesolo-exported adapter over the selected Qwen3.5 base and produces schema-constrained output on both target Macs. Do this before coupling inference into the application.

For the first demo implementation, Quip may run `mistral.rs` as a bundled local sidecar process and call its loopback API. This preserves a Rust implementation while isolating model startup and inference failures from the overlay. Linking the Rust SDK directly into the application is an optimization after the model path is proven.

### Architecture fallback

If the exported adapter does not work with `mistral.rs` on Metal, keep the macOS application in Rust and temporarily use a replaceable local inference sidecar that can load the artifact. Do not switch the product to remote inference merely to preserve an all-Rust claim.
