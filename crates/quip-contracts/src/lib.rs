//! Typed mirrors of the Phase 0 wire contracts.
//!
//! `docs/phase-0.schema.json` defines the provisional v0 shapes and
//! `docs/phase-0-contracts.md` defines the boundary rules; both are
//! authoritative over this crate. These types must round-trip
//! `docs/fixtures/phase-0-examples.json` without losing or inventing fields
//! (see `tests/fixture_roundtrip.rs`).
//!
//! Only three values cross workstream boundaries:
//! - [`PredictionRequest`] / [`PredictionResult`] between orchestration and inference
//! - [`CaptureResult`] between Accessibility and the composition UI
//! - [`SidecarHealth`] between inference and the health UI

use serde::{Deserialize, Serialize};

/// Which model answered or should answer a prediction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelVariant {
    Base,
    Global,
    GlobalPlusPersonal,
}

/// Where a successful result came from, independent of the model variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Fixture,
    Live,
}

/// Explicit failure payload. Failures never invent candidates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

/// One bounded window-text snippet passed as prediction context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextSnippet {
    pub app_name: String,
    pub window_title: String,
    pub visible_text: String,
}

/// One compact learned pattern from the local dictionary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonalPattern {
    pub shorthand: String,
    pub expansion: String,
}

/// Request from orchestration to inference for one writing burst.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PredictionRequest {
    pub request_id: String,
    pub profile_id: String,
    pub model_variant: ModelVariant,
    pub draft: String,
    pub context_snippets: Vec<ContextSnippet>,
    pub personal_patterns: Vec<PersonalPattern>,
}

/// Result of one prediction. A successful result carries zero through five
/// unique full-input replacements ordered best first. Zero candidates means
/// the five raw generations all resolved to unchanged input after filtering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PredictionResult {
    Ok {
        request_id: String,
        model_variant: ModelVariant,
        backend: Backend,
        candidates: Vec<String>,
        /// Same length as `candidates` when present: how many of the raw
        /// samples resolved to each candidate after filtering and dedup.
        /// Omitted by producers that cannot vote (the fixture backend emits
        /// all 1s). Consumers treat `None` as "no confidence signal".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        votes: Option<Vec<u32>>,
        // The schema allows any non-negative number; producers emit whole
        // milliseconds so fixtures round-trip as integers.
        latency_ms: u64,
    },
    Error {
        request_id: String,
        model_variant: ModelVariant,
        error: ErrorInfo,
    },
}

impl PredictionResult {
    /// Checks the candidate-count invariants from `docs/phase-0-contracts.md`.
    pub fn validate(&self) -> Result<(), String> {
        let PredictionResult::Ok {
            candidates, votes, ..
        } = self
        else {
            return Ok(());
        };
        if candidates.len() > 5 {
            return Err(format!("result has {} candidates", candidates.len()));
        }
        if candidates.iter().any(String::is_empty) {
            return Err("result has an empty candidate".to_string());
        }
        let unique = candidates.iter().collect::<std::collections::HashSet<_>>();
        if unique.len() != candidates.len() {
            return Err("result has duplicate candidates".to_string());
        }
        if let Some(votes) = votes {
            if votes.len() != candidates.len() {
                return Err(format!(
                    "result has {} votes for {} candidates",
                    votes.len(),
                    candidates.len()
                ));
            }
            if votes.iter().any(|&v| v == 0) {
                return Err("result has a zero vote".to_string());
            }
        }
        Ok(())
    }

    pub fn request_id(&self) -> &str {
        match self {
            PredictionResult::Ok { request_id, .. } => request_id,
            PredictionResult::Error { request_id, .. } => request_id,
        }
    }
}

/// What ended a writing burst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Idle,
    Punctuation,
    Return,
    Shortcut,
}

/// A rectangle in logical screen coordinates (origin top-left). Used for the
/// caret so the suggestion bar can be anchored above it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Result of Accessibility capture for one burst. `destination_id` is opaque:
/// the UI returns it on commit without interpreting it. The typed text stays
/// in the destination; `caret` anchors the candidate bar. Secure and
/// unsupported fields produce `Unavailable` and never reach inference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CaptureResult {
    Ready {
        burst_id: String,
        destination_id: String,
        profile_id: String,
        draft: String,
        trigger: Trigger,
        caret: Rect,
        /// Index of the draft's first word within the composition session
        /// (0-based, counted from the last session boundary). Lets the edit
        /// accumulator track corrections per word across overlapping bursts.
        /// Omitted by producers that do not track session word positions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        word_offset: Option<u32>,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ready,
    Degraded,
    Unavailable,
}

/// Which local artifacts the inference sidecar has loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadedArtifacts {
    pub base: bool,
    pub global_adapter: bool,
    pub user_adapter: bool,
}

/// Health report from the inference sidecar for the health UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SidecarHealth {
    pub status: HealthStatus,
    pub fixture_available: bool,
    pub loaded: LoadedArtifacts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
}

/// The shared fixture file at `docs/fixtures/phase-0-examples.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureFile {
    pub version: u32,
    pub prediction_exchanges: Vec<PredictionExchange>,
    pub capture_results: Vec<CaptureCase>,
    pub health_cases: Vec<HealthCase>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PredictionExchange {
    pub case_id: String,
    pub request: PredictionRequest,
    pub result: PredictionResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureCase {
    pub case_id: String,
    pub result: CaptureResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthCase {
    pub case_id: String,
    pub health: SidecarHealth,
}
