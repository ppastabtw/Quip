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

/// Explicit failure payload. Failures never invent an action or candidates.
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

/// The model's decision for a successful prediction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PredictionAction {
    Keep,
    Replace,
}

/// Result of one prediction. `keep` carries no candidates; `replace` carries
/// one to three full-input replacements ordered best first. The application,
/// not the model, adds the exact-draft option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PredictionResult {
    Ok {
        request_id: String,
        model_variant: ModelVariant,
        backend: Backend,
        action: PredictionAction,
        candidates: Vec<String>,
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
        match self {
            PredictionResult::Ok {
                action: PredictionAction::Keep,
                candidates,
                ..
            } if !candidates.is_empty() => {
                Err(format!("keep result has {} candidates", candidates.len()))
            }
            PredictionResult::Ok {
                action: PredictionAction::Replace,
                candidates,
                ..
            } if candidates.is_empty() || candidates.len() > 3 => {
                Err(format!("replace result has {} candidates", candidates.len()))
            }
            _ => Ok(()),
        }
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

/// Result of Accessibility capture for one burst. `destination_id` is opaque:
/// the UI returns it on commit or cancel without interpreting it. Secure and
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
