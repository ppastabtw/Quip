//! Workstream 4: composition state machine
//! (`Idle → Predicting → Presenting → committed | cancelled`).
//!
//! Consumes capture input (typed into the box pre-Workstream 3, scripted
//! fixtures from the demo harness, or real `CaptureResult`s later), owns
//! candidate state, and enforces the UI invariants: the exact draft is always
//! the first option, `keep` never bypasses confirmation, errors render an
//! explicit unavailable state with only the exact draft, and cancel commits
//! nothing.

use crate::commit::{self, CommitOutcome};
use crate::inference::{sidecar_predict_stub, FixtureBackend, Metrics};
use crate::learning::{self, LabeledExample, LearningStore};
use crate::settings::{AppSettings, BackendMode};
use quip_contracts::{
    Backend, ErrorInfo, ModelVariant, PredictionAction, PredictionRequest, PredictionResult,
    SidecarHealth, Trigger,
};
use serde::Serialize;
use std::path::Path;

/// Destination used for text typed directly into the box before Workstream 3
/// provides real captures.
pub const LOCAL_DESTINATION: &str = "destination_local_box";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionKind {
    Exact,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OptionItem {
    pub text: String,
    pub kind: OptionKind,
}

/// What the UI renders. Every mutation of the engine returns one of these and
/// the command layer broadcasts it; the webview holds no authoritative state.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Snapshot {
    Idle,
    Predicting {
        burst_id: String,
        draft: String,
        model_variant: ModelVariant,
    },
    Presenting {
        burst_id: String,
        draft: String,
        options: Vec<OptionItem>,
        /// Index into `options` of the model's recommendation (0 = exact draft).
        recommended: usize,
        model_variant: ModelVariant,
        backend: Option<Backend>,
        latency_ms: Option<u64>,
        error: Option<ErrorInfo>,
    },
    Committed {
        burst_id: String,
        destination_id: String,
        text: String,
    },
    Unavailable {
        reason: String,
    },
}

/// A burst that entered the pipeline.
#[derive(Debug, Clone)]
pub struct Burst {
    pub burst_id: String,
    pub destination_id: String,
    pub profile_id: String,
    pub draft: String,
    pub trigger: Trigger,
    pub model_variant: ModelVariant,
}

/// Input to `begin_burst`: either typed text (most fields defaulted) or a
/// full capture from the demo harness / Workstream 3.
#[derive(Debug, Clone)]
pub struct BurstInput {
    pub draft: String,
    pub trigger: Trigger,
    pub burst_id: Option<String>,
    pub destination_id: Option<String>,
    pub profile_id: Option<String>,
}

enum State {
    Idle,
    Predicting(Burst),
    Presenting {
        burst: Burst,
        options: Vec<OptionItem>,
        recommended: usize,
        backend: Option<Backend>,
        latency_ms: Option<u64>,
        error: Option<ErrorInfo>,
    },
}

pub struct Engine {
    pub settings: AppSettings,
    pub learning: LearningStore,
    pub backend: FixtureBackend,
    pub metrics: Metrics,
    state: State,
    burst_seq: u64,
    data_dir: std::path::PathBuf,
}

impl Engine {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            settings: AppSettings::load(data_dir),
            learning: LearningStore::open(data_dir),
            backend: FixtureBackend::new(),
            metrics: Metrics::default(),
            state: State::Idle,
            burst_seq: 0,
            data_dir: data_dir.to_path_buf(),
        }
    }

    pub fn save_settings(&self) {
        self.settings.save(&self.data_dir);
    }

    /// Starts a burst: builds the bounded prediction request and moves to
    /// Predicting. Returns the request for the caller to execute so the lock
    /// is not held during (simulated) inference.
    pub fn begin_burst(
        &mut self,
        input: BurstInput,
    ) -> Result<(Snapshot, PredictionRequest, BackendMode), Snapshot> {
        if !self.settings.enabled {
            return Err(Snapshot::Unavailable {
                reason: "disabled".to_string(),
            });
        }
        let draft = input.draft.trim().to_string();
        if draft.is_empty() {
            return Err(Snapshot::Idle);
        }
        self.burst_seq += 1;
        let burst = Burst {
            burst_id: input
                .burst_id
                .unwrap_or_else(|| format!("burst_{}_{}", self.burst_seq, learning::now_ms())),
            destination_id: input
                .destination_id
                .unwrap_or_else(|| LOCAL_DESTINATION.to_string()),
            profile_id: input
                .profile_id
                .unwrap_or_else(|| self.settings.active_profile.clone()),
            draft,
            trigger: input.trigger,
            model_variant: self.settings.model_variant,
        };
        let request = PredictionRequest {
            request_id: format!("req_{}", burst.burst_id),
            profile_id: burst.profile_id.clone(),
            model_variant: burst.model_variant,
            draft: burst.draft.clone(),
            // Window context arrives with Workstream 3; the toggle already
            // gates it here so nothing leaks once real snippets exist.
            context_snippets: Vec::new(),
            personal_patterns: self.learning.patterns_for_request(&burst.profile_id),
        };
        tracing::info!(
            burst_id = %burst.burst_id,
            trigger = ?burst.trigger,
            profile_id = %burst.profile_id,
            chars = burst.draft.len(),
            "burst started"
        );
        let snapshot = Snapshot::Predicting {
            burst_id: burst.burst_id.clone(),
            draft: burst.draft.clone(),
            model_variant: burst.model_variant,
        };
        self.state = State::Predicting(burst);
        Ok((snapshot, request, self.settings.backend_mode))
    }

    /// Runs the configured backend. Split from `begin_burst` so the async
    /// caller can simulate latency between the two without holding the lock.
    pub fn predict(&mut self, request: &PredictionRequest, mode: BackendMode) -> PredictionResult {
        let result = match mode {
            BackendMode::Fixture => self.backend.predict(request),
            BackendMode::Live => sidecar_predict_stub(request),
        };
        let valid = self.metrics.record(&result);
        if valid {
            result
        } else {
            tracing::warn!(request_id = %request.request_id, "schema-invalid prediction result");
            PredictionResult::Error {
                request_id: request.request_id.clone(),
                model_variant: request.model_variant,
                error: ErrorInfo {
                    code: "schema_invalid".to_string(),
                    message: "The model returned a schema-invalid result.".to_string(),
                    retryable: true,
                },
            }
        }
    }

    /// Applies a finished prediction. Returns None when the burst was
    /// cancelled or superseded while inference ran (stale results are
    /// dropped, never presented).
    pub fn apply_result(&mut self, burst_id: &str, result: PredictionResult) -> Option<Snapshot> {
        let State::Predicting(burst) = &self.state else {
            return None;
        };
        if burst.burst_id != burst_id {
            return None;
        }
        let burst = burst.clone();

        // Invariant: the application adds the exact draft as option 0; the
        // model never returns it.
        let mut options = vec![OptionItem {
            text: burst.draft.clone(),
            kind: OptionKind::Exact,
        }];
        let (recommended, backend, latency_ms, error) = match result {
            PredictionResult::Ok {
                action,
                candidates,
                backend,
                latency_ms,
                ..
            } => {
                options.extend(candidates.into_iter().map(|text| OptionItem {
                    text,
                    kind: OptionKind::Candidate,
                }));
                let recommended = match action {
                    PredictionAction::Keep => 0,
                    PredictionAction::Replace => 1,
                };
                (recommended, Some(backend), Some(latency_ms), None)
            }
            // Failures present explicitly: exact draft only, error visible.
            PredictionResult::Error { error, .. } => (0, None, None, Some(error)),
        };

        let snapshot = Snapshot::Presenting {
            burst_id: burst.burst_id.clone(),
            draft: burst.draft.clone(),
            options: options.clone(),
            recommended,
            model_variant: burst.model_variant,
            backend,
            latency_ms,
            error: error.clone(),
        };
        self.state = State::Presenting {
            burst,
            options,
            recommended,
            backend,
            latency_ms,
            error,
        };
        Some(snapshot)
    }

    /// Commits the selected option. Only ever called with explicit user
    /// confirmation; there is no auto-commit path.
    pub fn confirm(&mut self, index: usize) -> Result<(Snapshot, CommitOutcome), String> {
        let State::Presenting { burst, options, .. } = &self.state else {
            return Err("nothing to confirm".to_string());
        };
        let option = options.get(index).ok_or("option index out of range")?.clone();
        let burst = burst.clone();
        let had_candidates = options.len() > 1;

        let outcome = commit::commit_text(&burst.destination_id, &option.text);
        self.record_confirmation(&burst, &option, had_candidates);
        self.state = State::Idle;
        Ok((
            Snapshot::Committed {
                burst_id: burst.burst_id,
                destination_id: outcome.destination_id.clone(),
                text: outcome.text.clone(),
            },
            outcome,
        ))
    }

    fn record_confirmation(&mut self, burst: &Burst, option: &OptionItem, had_candidates: bool) {
        if self.settings.learning_paused {
            return;
        }
        let (label, source) = match option.kind {
            // Choosing the exact draft over offered candidates is a `keep` label.
            OptionKind::Exact if had_candidates => ("keep", "exact_draft"),
            OptionKind::Exact => return, // nothing was suggested; nothing to learn
            OptionKind::Candidate => ("replace", "confirmed_candidate"),
        };
        self.learning.append_example(&LabeledExample {
            ts_ms: learning::now_ms(),
            burst_id: burst.burst_id.clone(),
            profile_id: burst.profile_id.clone(),
            draft: burst.draft.clone(),
            label: label.to_string(),
            committed: option.text.clone(),
            source: source.to_string(),
            model_variant: burst.model_variant,
        });
        if option.kind == OptionKind::Candidate {
            for (shorthand, expansion) in learning::extract_patterns(&burst.draft, &option.text) {
                self.learning
                    .record_pattern(&burst.profile_id, &shorthand, &expansion);
            }
        }
    }

    /// Cancels the current burst. Inserts nothing; a dismissal of visible
    /// candidates becomes a `keep` example per the spec.
    pub fn cancel(&mut self) -> Snapshot {
        if let State::Presenting { burst, options, .. } = &self.state {
            if options.len() > 1 && !self.settings.learning_paused {
                self.learning.append_example(&LabeledExample {
                    ts_ms: learning::now_ms(),
                    burst_id: burst.burst_id.clone(),
                    profile_id: burst.profile_id.clone(),
                    draft: burst.draft.clone(),
                    label: "keep".to_string(),
                    committed: String::new(),
                    source: "dismissal".to_string(),
                    model_variant: burst.model_variant,
                });
            }
        }
        self.state = State::Idle;
        Snapshot::Idle
    }

    /// Rebuilds the current UI snapshot, used to sync a webview on load and
    /// by the selftest to observe state between steps.
    pub fn current_snapshot(&self) -> Snapshot {
        match &self.state {
            State::Idle => Snapshot::Idle,
            State::Predicting(burst) => Snapshot::Predicting {
                burst_id: burst.burst_id.clone(),
                draft: burst.draft.clone(),
                model_variant: burst.model_variant,
            },
            State::Presenting {
                burst,
                options,
                recommended,
                backend,
                latency_ms,
                error,
            } => Snapshot::Presenting {
                burst_id: burst.burst_id.clone(),
                draft: burst.draft.clone(),
                options: options.clone(),
                recommended: *recommended,
                model_variant: burst.model_variant,
                backend: *backend,
                latency_ms: *latency_ms,
                error: error.clone(),
            },
        }
    }

    pub fn health(&self) -> SidecarHealth {
        match self.settings.backend_mode {
            BackendMode::Fixture => self.backend.health(),
            BackendMode::Live => crate::inference::sidecar_health_stub(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_engine() -> (Engine, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "quip-engine-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut engine = Engine::new(&dir);
        engine.settings = AppSettings::default();
        (engine, dir)
    }

    fn typed(draft: &str) -> BurstInput {
        BurstInput {
            draft: draft.to_string(),
            trigger: Trigger::Idle,
            burst_id: None,
            destination_id: None,
            profile_id: None,
        }
    }

    fn run_burst(engine: &mut Engine, draft: &str) -> Snapshot {
        let (_, request, mode) = engine.begin_burst(typed(draft)).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        let result = engine.predict(&request, mode);
        engine.apply_result(&burst_id, result).unwrap()
    }

    #[test]
    fn replace_result_presents_exact_draft_first() {
        let (mut engine, dir) = test_engine();
        let snapshot = run_burst(&mut engine, "cnt cm tmrw");
        let Snapshot::Presenting {
            options,
            recommended,
            error,
            ..
        } = snapshot
        else {
            panic!("expected presenting");
        };
        assert_eq!(options[0].kind, OptionKind::Exact);
        assert_eq!(options[0].text, "cnt cm tmrw");
        assert_eq!(options[1].text, "Can't come tomorrow.");
        assert_eq!(recommended, 1);
        assert!(error.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn keep_result_still_requires_confirmation() {
        let (mut engine, dir) = test_engine();
        engine.settings.model_variant = ModelVariant::Global;
        let snapshot = run_burst(&mut engine, "open usr/bin and q3_finl_v2.pdf");
        let Snapshot::Presenting {
            options,
            recommended,
            ..
        } = snapshot
        else {
            panic!("expected presenting");
        };
        // keep: exact draft is the only option and the recommendation,
        // but nothing commits until confirm() is called.
        assert_eq!(options.len(), 1);
        assert_eq!(recommended, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn error_result_presents_exact_draft_with_explicit_error() {
        let (mut engine, dir) = test_engine();
        engine.backend.simulate_failure = true;
        let snapshot = run_burst(&mut engine, "cnt cm tmrw");
        let Snapshot::Presenting { options, error, .. } = snapshot else {
            panic!("expected presenting");
        };
        assert_eq!(options.len(), 1);
        assert_eq!(error.unwrap().code, "adapter_not_loaded");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn confirming_candidate_commits_and_learns() {
        let (mut engine, dir) = test_engine();
        run_burst(&mut engine, "ship spec tn");
        let (snapshot, outcome) = engine.confirm(1).unwrap();
        assert!(matches!(snapshot, Snapshot::Committed { .. }));
        assert_eq!(outcome.destination_id, LOCAL_DESTINATION);
        assert_eq!(outcome.text, "Ship spec tonight.");
        // The confirmed replacement was mined back into the dictionary.
        let raw = std::fs::read_to_string(dir.join("profiles/profile_a/examples.jsonl")).unwrap();
        assert!(raw.contains("\"confirmed_candidate\""));
        // Confirming twice is impossible: state returned to idle.
        assert!(engine.confirm(1).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cancel_commits_nothing_and_records_dismissal() {
        let (mut engine, dir) = test_engine();
        run_burst(&mut engine, "cnt cm tmrw");
        assert_eq!(engine.cancel(), Snapshot::Idle);
        let raw = std::fs::read_to_string(dir.join("profiles/profile_a/examples.jsonl")).unwrap();
        assert!(raw.contains("\"dismissal\""));
        assert!(raw.contains("\"keep\""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn paused_learning_records_nothing() {
        let (mut engine, dir) = test_engine();
        engine.settings.learning_paused = true;
        run_burst(&mut engine, "cnt cm tmrw");
        engine.confirm(1).unwrap();
        assert!(!dir.join("profiles/profile_a/examples.jsonl").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn disabled_app_rejects_bursts() {
        let (mut engine, dir) = test_engine();
        engine.settings.enabled = false;
        let err = engine.begin_burst(typed("cnt cm tmrw")).unwrap_err();
        assert!(matches!(err, Snapshot::Unavailable { .. }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_results_are_dropped_after_cancel() {
        let (mut engine, dir) = test_engine();
        let (_, request, mode) = engine.begin_burst(typed("cnt cm tmrw")).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        let result = engine.predict(&request, mode);
        engine.cancel();
        assert!(engine.apply_result(&burst_id, result).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_mode_reports_sidecar_unavailable() {
        let (mut engine, dir) = test_engine();
        engine.settings.backend_mode = BackendMode::Live;
        let snapshot = run_burst(&mut engine, "cnt cm tmrw");
        let Snapshot::Presenting { error, .. } = snapshot else {
            panic!("expected presenting");
        };
        assert_eq!(error.unwrap().code, "sidecar_unavailable");
        assert_eq!(
            engine.health().status,
            quip_contracts::HealthStatus::Unavailable
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
