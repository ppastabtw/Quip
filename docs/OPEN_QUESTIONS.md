# Quip implementation checks

The product decisions are complete. These remaining unknowns require build-time experiments:

1. Verify that `mistral.rs` loads the exact exported Freesolo adapter on Metal; use another local sidecar if it fails.
2. Verify global and per-user adapter composition; fall back to a merged global model plus one user adapter if necessary.
3. Define the minimum profile-example threshold and verify a private Freesolo per-user run can produce an adapter that composes with the global adapter.
4. Verify passive Accessibility observation can preserve burst markers and replace only the intended range as the destination keeps receiving input.
5. Confirm that Accessibility exposes enough bounded text from the demo applications for useful window context.
6. Benchmark Qwen3.5-2B first and try 4B only if quality requires it and latency remains interactive.
7. Grow the global training set only until the held-out comparison shows a clear improvement.
8. Confirm the official demo duration; assume three minutes until then.
9. Select final base, context, and personalized examples from real model outputs.
10. Export the selected adapter or checkpoint to the team Hugging Face repository and verify the downloaded artifact before managed retention expires.
