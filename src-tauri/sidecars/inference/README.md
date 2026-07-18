# Inference sidecar (Workstream 2)

Landing spot for the local inference sidecar built around `mistral.rs` with
Metal: base Qwen3.5 loading, 4-bit quantization, the global Freesolo adapter,
per-user adapter composition, guided JSON decoding, and latency reporting.

The sidecar speaks the Phase 0 shapes (`crates/quip-contracts`,
`docs/phase-0.schema.json`): it answers `prediction_request` with
`prediction_result` and reports `sidecar_health`. The app-side client lives in
`src-tauri/src/inference/`. Model binaries and adapters are local artifacts
under `artifacts/models/` and are never committed.
