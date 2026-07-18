# Inference sidecar (Workstream 2)

The sidecar has a deterministic fixture backend and an opt-in live backend for
Qwen3.5-2B through a loopback-only `mistral.rs` server. Fixture mode remains the
demo-safe fallback and requires no model installation.

The sidecar speaks the Phase 0 shapes (`crates/quip-contracts`,
`docs/phase-0.schema.json`): it answers `prediction_request` with
`prediction_result` and reports `sidecar_health`. The app-side client lives in
`src-tauri/src/inference/`. Model binaries and adapters are local artifacts
under `artifacts/models/` and are never committed.

## Development protocol

The sidecar reads one newline-delimited JSON command at a time from standard
input and writes one Phase 0 result to standard output. It stays alive for
multiple commands and does not log drafts or generated text.

```json
{"operation":"health"}
{"operation":"health","case_id":"adapter_degraded"}
{"operation":"predict","request":{"request_id":"fresh-id","profile_id":"profile_default","model_variant":"base","draft":"cnt cm tmrw","context_snippets":[],"personal_patterns":[]}}
```

Run the repository validation skill after sidecar or contract changes:

```text
.agents/skills/validate-quip-sidecar/scripts/validate.sh
```

## Phrase tester

Compare the fixture base and global variants interactively:

```bash
cargo run --quiet -p quip-inference-sidecar --bin quip-phrase-tester
```

Or test one phrase directly:

```bash
cargo run --quiet -p quip-inference-sidecar --bin quip-phrase-tester -- "cnt cm tmrw"
```

The tester identifies itself as fixture mode because it does not load Qwen.
It lists the prerecorded phrases that currently have results and reports an
unavailable result for other phrases.

## Live phrase inference

After installing `mistral.rs`, run one command from the repository root. It
starts the loopback-only 4-bit Qwen server when needed and then opens the same
interactive tester in live mode:

```bash
src-tauri/sidecars/inference/scripts/run-live-phrase-tester.sh
```

Pass a phrase to perform one inference and exit:

```bash
src-tauri/sidecars/inference/scripts/run-live-phrase-tester.sh "cnt cm tmr"
```

The base Qwen result includes measured end-to-end latency. Until a Freesolo
adapter is installed, the global variant returns `adapter_not_loaded` rather
than duplicating the base result.

Each model completion is constrained to the same single-suggestion JSON schema
used by Freesolo training:

```json
{"suggestion":"best full text"}
```

The Rust inference layer requests exactly five completions concurrently, removes
exact-draft suggestions, deduplicates changed text, ranks duplicate votes with
earliest-completion tie-breaking, and returns zero to five candidates. Zero
candidates means skip and no suggestion bar.

## Run the full app with live inference

The team can launch the pushed Tauri app, the Rust sidecar, and local Base Qwen
with one command:

```bash
src-tauri/sidecars/inference/scripts/run-live-app.sh
```

This development launcher builds the sidecar, starts the loopback model server
when necessary, and forces `live` / `base` for that app session without
overwriting persisted settings. The app keeps one NDJSON sidecar process alive
for health and prediction requests. Global variants remain unavailable until
the Freesolo adapter is exported and installed.
