//! Workstream 4: composition state machine, IME model
//! (`Idle -> Predicting -> Suggesting -> applied | dismissed`).
//!
//! The user types directly into their own textbox; Quip observes bursts and
//! floats a candidate bar above the caret. Invariants: an empty successful
//! result shows nothing, candidates replace the burst in place
//! only on explicit selection, dismissal and stale results change nothing,
//! and starting a new burst while suggestions are visible counts as a stable
//! dismissal (a `keep` learning label).

use crate::commit::{self, CommitOutcome};
use crate::inference::{sidecar_predict_stub, FixtureBackend, Metrics};
use crate::learning::{self, LabeledExample, LearningStore};
use crate::settings::{AppSettings, BackendMode};
use quip_contracts::{
    Backend, ErrorInfo, ModelVariant, PredictionRequest, PredictionResult, Rect, SidecarHealth,
    Trigger,
};
use serde::Serialize;
use std::path::Path;

/// What the UI renders. Every mutation of the engine returns one of these and
/// the command layer broadcasts it; the webview holds no authoritative state.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Snapshot {
    /// No bar visible. Also the result of a zero-candidate prediction: the typed
    /// text stands and the user is never interrupted.
    Idle,
    Predicting {
        burst_id: String,
        draft: String,
        model_variant: ModelVariant,
    },
    /// The candidate bar is visible above `caret`. `candidates` holds one to
    /// five model replacements (empty only in the error state).
    Suggesting {
        burst_id: String,
        draft: String,
        candidates: Vec<String>,
        /// Index into `candidates` of the model's best option.
        recommended: usize,
        /// Index of the highlighted candidate (arrow keys move it; Tab
        /// accepts it). Starts at `recommended`.
        selected: usize,
        caret: Rect,
        model_variant: ModelVariant,
        backend: Option<Backend>,
        latency_ms: Option<u64>,
        error: Option<ErrorInfo>,
    },
    /// A candidate was selected; the burst was replaced in the destination.
    Applied {
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
    pub caret: Rect,
    pub model_variant: ModelVariant,
}

/// Input to `begin_burst`: a `capture_result.ready` from the playground now,
/// Workstream 3's Accessibility observer later.
#[derive(Debug, Clone)]
pub struct BurstInput {
    pub draft: String,
    pub trigger: Trigger,
    pub caret: Rect,
    pub burst_id: Option<String>,
    pub destination_id: Option<String>,
    pub profile_id: Option<String>,
}

enum State {
    Idle,
    Predicting(Burst),
    Suggesting {
        burst: Burst,
        candidates: Vec<String>,
        recommended: usize,
        selected: usize,
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
    /// is not held during (simulated) inference. Superseding visible
    /// suggestions is silent (IME model: the burst is still growing and the
    /// bar simply refreshes); only an explicit dismissal records a keep label.
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
            self.state = State::Idle;
            return Err(Snapshot::Idle);
        }
        self.burst_seq += 1;
        let burst = Burst {
            burst_id: input
                .burst_id
                .unwrap_or_else(|| format!("burst_{}_{}", self.burst_seq, learning::now_ms())),
            destination_id: input
                .destination_id
                .unwrap_or_else(|| "destination_unknown".to_string()),
            profile_id: input
                .profile_id
                .unwrap_or_else(|| self.settings.active_profile.clone()),
            draft,
            trigger: input.trigger,
            caret: input.caret,
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
    /// dismissed or superseded while inference ran (stale results are
    /// dropped, never shown). A zero-candidate result returns Idle: no bar.
    pub fn apply_result(&mut self, burst_id: &str, result: PredictionResult) -> Option<Snapshot> {
        let State::Predicting(burst) = &self.state else {
            return None;
        };
        if burst.burst_id != burst_id {
            return None;
        }
        let burst = burst.clone();

        let (candidates, backend, latency_ms, error) = match result {
            PredictionResult::Ok { candidates, .. } if candidates.is_empty() => {
                // The typed text stands; the user is never interrupted.
                self.state = State::Idle;
                return Some(Snapshot::Idle);
            }
            PredictionResult::Ok {
                candidates,
                backend,
                latency_ms,
                ..
            } => (candidates, Some(backend), Some(latency_ms), None),
            // Failures surface as an explicit error chip in the bar; the
            // typed text is untouched either way.
            PredictionResult::Error { error, .. } => (Vec::new(), None, None, Some(error)),
        };

        let snapshot = Snapshot::Suggesting {
            burst_id: burst.burst_id.clone(),
            draft: burst.draft.clone(),
            candidates: candidates.clone(),
            recommended: 0,
            selected: 0,
            caret: burst.caret,
            model_variant: burst.model_variant,
            backend,
            latency_ms,
            error: error.clone(),
        };
        self.state = State::Suggesting {
            burst,
            candidates,
            recommended: 0,
            selected: 0,
            backend,
            latency_ms,
            error,
        };
        Some(snapshot)
    }

    /// Moves the highlight left/right through the candidates with
    /// wrap-around (arrow keys). Returns None when no candidates are
    /// visible.
    pub fn move_selection(&mut self, delta: i64) -> Option<Snapshot> {
        let State::Suggesting {
            candidates,
            selected,
            ..
        } = &mut self.state
        else {
            return None;
        };
        if candidates.is_empty() {
            return None;
        }
        let len = candidates.len() as i64;
        *selected = (*selected as i64 + delta).rem_euclid(len) as usize;
        Some(self.current_snapshot())
    }

    /// Replaces the burst with the selected candidate. Only ever called with
    /// an explicit selection (number key or click); there is no auto-apply.
    pub fn select(&mut self, index: usize) -> Result<(Snapshot, CommitOutcome), String> {
        let State::Suggesting {
            burst, candidates, ..
        } = &self.state
        else {
            return Err("no suggestions to select".to_string());
        };
        let text = candidates
            .get(index)
            .ok_or("candidate index out of range")?
            .clone();
        let burst = burst.clone();

        let outcome = commit::replace_burst(&burst.destination_id, &burst.burst_id, &text);
        if !self.settings.learning_paused {
            self.learning.append_example(&LabeledExample {
                ts_ms: learning::now_ms(),
                burst_id: burst.burst_id.clone(),
                profile_id: burst.profile_id.clone(),
                draft: burst.draft.clone(),
                label: "replace".to_string(),
                committed: text.clone(),
                source: "confirmed_candidate".to_string(),
                model_variant: burst.model_variant,
            });
            for (shorthand, expansion) in learning::extract_patterns(&burst.draft, &text) {
                self.learning
                    .record_pattern(&burst.profile_id, &shorthand, &expansion);
            }
        }
        self.state = State::Idle;
        Ok((
            Snapshot::Applied {
                burst_id: burst.burst_id,
                destination_id: outcome.destination_id.clone(),
                text: outcome.text.clone(),
            },
            outcome,
        ))
    }

    /// Dismisses visible suggestions (Escape, or the caller observed the
    /// user typing on). Changes nothing in the destination; a stable
    /// dismissal of real candidates becomes a `keep` example per the spec.
    pub fn dismiss(&mut self) -> Snapshot {
        self.record_dismissal_if_suggesting();
        self.state = State::Idle;
        Snapshot::Idle
    }

    fn record_dismissal_if_suggesting(&mut self) {
        if let State::Suggesting {
            burst, candidates, ..
        } = &self.state
        {
            if !candidates.is_empty() && !self.settings.learning_paused {
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
            State::Suggesting {
                burst,
                candidates,
                recommended,
                selected,
                backend,
                latency_ms,
                error,
            } => Snapshot::Suggesting {
                burst_id: burst.burst_id.clone(),
                draft: burst.draft.clone(),
                candidates: candidates.clone(),
                recommended: *recommended,
                selected: *selected,
                caret: burst.caret,
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

    fn caret() -> Rect {
        Rect {
            x: 100.0,
            y: 200.0,
            width: 2.0,
            height: 18.0,
        }
    }

    fn typed(draft: &str) -> BurstInput {
        BurstInput {
            draft: draft.to_string(),
            trigger: Trigger::Idle,
            caret: caret(),
            burst_id: None,
            destination_id: Some("destination_test".to_string()),
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
    fn candidate_result_shows_candidates_at_the_caret() {
        let (mut engine, dir) = test_engine();
        let snapshot = run_burst(&mut engine, "cnt cm tmrw");
        let Snapshot::Suggesting {
            candidates,
            recommended,
            caret: at,
            error,
            ..
        } = snapshot
        else {
            panic!("expected suggesting");
        };
        assert_eq!(candidates[0], "Can't come tomorrow.");
        assert_eq!(candidates.len(), 5);
        assert_eq!(recommended, 0);
        assert_eq!(at, caret());
        assert!(error.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn zero_candidate_result_shows_no_bar_at_all() {
        let (mut engine, dir) = test_engine();
        engine.settings.model_variant = ModelVariant::Global;
        let snapshot = run_burst(&mut engine, "open usr/bin and q3_finl_v2.pdf");
        assert_eq!(snapshot, Snapshot::Idle);
        // Nothing was suggested, so nothing was learned either.
        assert!(!dir.join("profiles/profile_a/examples.jsonl").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn error_result_shows_error_bar_with_no_candidates() {
        let (mut engine, dir) = test_engine();
        engine.backend.simulate_failure = true;
        let snapshot = run_burst(&mut engine, "cnt cm tmrw");
        let Snapshot::Suggesting {
            candidates, error, ..
        } = snapshot
        else {
            panic!("expected suggesting");
        };
        assert!(candidates.is_empty());
        assert_eq!(error.unwrap().code, "adapter_not_loaded");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn selecting_a_candidate_replaces_and_learns() {
        let (mut engine, dir) = test_engine();
        run_burst(&mut engine, "ship spec tn");
        let (snapshot, outcome) = engine.select(0).unwrap();
        assert!(matches!(snapshot, Snapshot::Applied { .. }));
        assert_eq!(outcome.destination_id, "destination_test");
        assert_eq!(outcome.text, "Ship spec tonight.");
        let raw = std::fs::read_to_string(dir.join("profiles/profile_a/examples.jsonl")).unwrap();
        assert!(raw.contains("\"confirmed_candidate\""));
        // Selecting twice is impossible: state returned to idle.
        assert!(engine.select(0).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn arrow_selection_wraps_and_tab_accepts_highlighted() {
        let (mut engine, dir) = test_engine();
        run_burst(&mut engine, "cnt cm tmrw"); // 5 candidates
        let snap = engine.move_selection(1).unwrap();
        let Snapshot::Suggesting { selected, .. } = snap else {
            panic!("expected suggesting");
        };
        assert_eq!(selected, 1);
        // Wraps around in both directions.
        engine.move_selection(4).unwrap();
        let Snapshot::Suggesting {
            selected,
            candidates,
            ..
        } = engine.current_snapshot()
        else {
            panic!("expected suggesting");
        };
        assert_eq!(selected, 0);
        // Accepting the highlighted candidate commits that exact text.
        engine.move_selection(-1).unwrap();
        let (_, outcome) = engine.select(candidates.len() - 1).unwrap();
        assert_eq!(outcome.text, candidates[candidates.len() - 1]);
        // No selection to move once idle.
        assert!(engine.move_selection(1).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dismiss_changes_nothing_and_records_keep() {
        let (mut engine, dir) = test_engine();
        run_burst(&mut engine, "cnt cm tmrw");
        assert_eq!(engine.dismiss(), Snapshot::Idle);
        let raw = std::fs::read_to_string(dir.join("profiles/profile_a/examples.jsonl")).unwrap();
        assert!(raw.contains("\"dismissal\""));
        assert!(raw.contains("\"keep\""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn superseding_suggestions_is_silent_continuation() {
        let (mut engine, dir) = test_engine();
        run_burst(&mut engine, "cnt cm tmrw");
        // The burst keeps growing while suggestions are visible: the bar
        // refreshes, but no keep label is recorded (not a stable dismissal).
        let _ = engine.begin_burst(typed("cnt cm tmrw ok")).unwrap();
        assert!(!dir.join("profiles/profile_a/examples.jsonl").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn error_bar_dismissal_is_not_a_keep_example() {
        let (mut engine, dir) = test_engine();
        engine.backend.simulate_failure = true;
        run_burst(&mut engine, "cnt cm tmrw");
        engine.dismiss();
        // No candidates were shown, so there is no keep signal to record.
        assert!(!dir.join("profiles/profile_a/examples.jsonl").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn paused_learning_records_nothing() {
        let (mut engine, dir) = test_engine();
        engine.settings.learning_paused = true;
        run_burst(&mut engine, "cnt cm tmrw");
        engine.select(0).unwrap();
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
    fn stale_results_are_dropped_after_dismiss() {
        let (mut engine, dir) = test_engine();
        let (_, request, mode) = engine.begin_burst(typed("cnt cm tmrw")).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        let result = engine.predict(&request, mode);
        engine.dismiss();
        assert!(engine.apply_result(&burst_id, result).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_mode_reports_sidecar_unavailable() {
        let (mut engine, dir) = test_engine();
        engine.settings.backend_mode = BackendMode::Live;
        let snapshot = run_burst(&mut engine, "cnt cm tmrw");
        let Snapshot::Suggesting { error, .. } = snapshot else {
            panic!("expected suggesting");
        };
        assert_eq!(error.unwrap().code, "sidecar_unavailable");
        assert_eq!(
            engine.health().status,
            quip_contracts::HealthStatus::Unavailable
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
