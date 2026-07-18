# Quip implementation unknowns

The product spec is ready for implementation. No further user decision is currently required. These items must be answered through short build-time experiments.

## Blocking questions

None.

## Build-time checks

1. Verify that `mistral.rs` loads the exact Freesolo adapter on Metal. Use a different local sidecar if it fails.
2. Verify that the runtime can compose the global adapter with a separate per-user adapter. Fall back to a merged global model plus one user adapter if necessary.
3. Choose and benchmark a local LoRA training path for per-user adapters on both target Macs.
4. Verify that Quip can preserve a destination insertion point while its temporary composition box receives keyboard input, then restore and commit reliably.
5. Verify that Accessibility exposes enough bounded visible text from the required demo applications to make window context useful.
6. Benchmark Qwen3.5-2B first. Try 4B only if the quality gap matters and latency remains interactive.
7. Grow the initial training set only until the held-out comparison shows a clear improvement.
8. Confirm the official demo duration. Assume a three-minute pitch until then.
9. Select the final base, context, and personalized examples after seeing real model outputs.

## Decision log

- Quip will be tightly scoped for a hackathon demo.
- The primary interaction is a temporary composition box that receives text before the destination application does.
- The exact typed draft and any model candidates require explicit confirmation before insertion.
- Selection-based replacement is a secondary interaction for existing text.
- The intended experience is an always-running local app with burst-based intelligent triggering.
- Inference will not run for every character.
- All insertion and replacement actions require explicit confirmation.
- Accessibility is the primary replacement path, with clipboard-based copy and paste as a fallback.
- The clipboard fallback is acceptable if previous clipboard contents are restored.
- Both single-word mistakes and multiword shorthand are in scope.
- Single-word mistakes receive a stricter confidence threshold.
- Universal compatibility across every Mac application is not required.
- Flash will train a LoRA adapter over a Qwen3.5 base.
- The model will use non-thinking mode and guided JSON output.
- The model will directly emit an ordered list of zero to three candidates.
- Training starts with SFT and may continue with GRPO warm-started from the SFT adapter.
- The base model and every useful checkpoint will use an untouched held-out evaluation.
- Training data begins with hundreds of clean examples, with deduplication and decontamination required.
- The production claim requires local inference. A managed Flash endpoint is only a development aid.
- Product use sends no user text to Freesolo or any other remote inference service.
- Freesolo is used to train and export the adapter. The base model and adapter run locally on the Mac.
- The primary demo machine has an M3 Pro and 18 GB unified memory.
- The backup compatibility machine is an M4 MacBook Air with 16 GB unified memory.
- Start local inference testing with Qwen3.5-2B at 4-bit quantization and benchmark Qwen3.5-4B.
- The application uses a Rust-first architecture.
- A minimal HTML and CSS Tauri overlay is acceptable. System integration, state, and inference orchestration remain in Rust.
- The initial inference integration may use a bundled local `mistral.rs` sidecar to isolate model lifecycle concerns.
- Four people will build Quip.
- Automatic temporary composition, local inference, confirmation, and destination insertion are required before selection-based replacement begins.
- A final phase is reserved for integration, compatibility testing, and demo rehearsal.
- Each macOS user receives a separate locally trained adapter based on labeled Quip interactions.
- Per-user training data and adapter weights remain on the Mac.
- Quip may use bounded accessible text from relevant open windows as temporary local context.
- Open-window context is not uploaded or retained by default.
