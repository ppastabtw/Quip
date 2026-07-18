---
name: validate-quip-sidecar
description: Build and validate Quip's local inference sidecar and phrase tester through their real process boundaries. Use after changing the fixture or live backend, health reporting, sidecar protocol, phrase-testing CLI, shared prediction contracts, or app-side inference client, and before claiming those behaviors work end to end.
---

# Validate Quip Sidecar

Run the repository's deterministic integration validator from the repository root:

```bash
.agents/skills/validate-quip-sidecar/scripts/validate.sh
```

The validator must:

1. Format-check the sidecar package and test the Rust workspace.
2. Build the actual `quip-inference-sidecar` executable.
3. Keep one sidecar process alive across health and prediction requests.
4. Verify ready health, schema-valid zero- and five-candidate results, request-ID echoing, and the explicit missing-adapter error.
5. Run the phrase tester and verify its base/global comparison is visibly labeled as fixture mode.
6. Print the observed responses without writing generated logs to the repository.

Treat unit tests alone as insufficient. A successful run ends with `Quip sidecar integration passed` after the process-level checks. On failure, report the failing command and response; do not claim completion.

After changing live inference behavior, also run:

```bash
.agents/skills/validate-quip-sidecar/scripts/validate-live.sh
```

The live validator must exercise Qwen through the actual loopback HTTP server
and the sidecar NDJSON boundary, then launch the real Tauri binary in live
self-test mode. It verifies live health, schema-valid base output, latency,
contract-valid zero-to-five candidate conversion, explicit unloaded-adapter results,
app-side child-process launch, candidate rendering state, and metrics.
Deterministic tests separately verify vote ranking, deduplication, five-candidate
capping, and exact-draft filtering. A successful run ends with
`Quip live inference integration passed`.
