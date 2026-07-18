//! Workstream 4 client / Workstream 2 boundary: prediction backend.
//!
//! Builds `quip_contracts::PredictionRequest` values (bounded draft, ranked
//! context snippets, personal patterns) and validates every
//! `PredictionResult`. Two backends share one trait: a deterministic fixture
//! backend keyed off the Phase 0 fixtures and demo corpus, and the loopback
//! client for Workstream 2's local inference sidecar
//! (`src-tauri/sidecars/inference/`). Also surfaces
//! `quip_contracts::SidecarHealth` for the health UI.
