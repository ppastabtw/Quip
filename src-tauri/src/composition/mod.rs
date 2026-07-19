//! Workstream 4: composition state machine, IME model.
//!
//! The user types directly into their own textbox; Quip observes bursts and
//! floats a candidate bar above the caret. Settled predictions become
//! *offers* that queue oldest-first: the bar always shows the oldest
//! unresolved offer, later batches wait behind it, and a burst can be
//! inferring while an earlier offer is still on display. Invariants: a
//! result whose candidates all equal the typed draft shows nothing,
//! candidates replace the burst in place only on explicit selection,
//! dismissal and stale results change nothing, and resolving one offer
//! surfaces the next already-computed one immediately.
//!
//! Alongside the offer queue, every settled result also feeds the
//! session-scoped word-level edit accumulator (`edits::SessionEdits`):
//! corrections that stay stable across passes harden into quiet inline
//! marks the user can apply all at once, and a sentence boundary can
//! consolidate them into one full-sentence offer.

pub mod edits;

use crate::commit::{self, CommitOutcome};
use crate::inference::{FixtureBackend, Metrics};
use crate::learning::{self, LabeledExample, LearningStore};
use crate::settings::{AppSettings, BackendMode};
use quip_contracts::{
    Backend, ContextSnippet, ErrorInfo, ModelVariant, PredictionRequest, PredictionResult, Rect,
    SidecarHealth, Trigger,
};
use serde::Serialize;
use std::collections::VecDeque;
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
    /// The candidate bar is visible above `caret`, showing the oldest
    /// unresolved offer. `candidates` holds one to five model replacements
    /// (empty only in the error state).
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
    /// Context visible when this burst was captured. Stored for learning even
    /// when the request-side context toggle is off.
    pub context_snippets: Vec<ContextSnippet>,
    pub context_used_for_model: bool,
    /// Session word index of the draft's first word, when the capture side
    /// tracks it. Required for the edit accumulator; without it a burst's
    /// results still make offers but never update word slots.
    pub word_offset: Option<u32>,
    /// Sliding-window cadence: results feed the accumulator only, no bar.
    pub barless: bool,
}

/// Input to `begin_burst`: a `capture_result.ready` from the playground or a
/// manual focused Accessibility capture.
#[derive(Debug, Clone)]
pub struct BurstInput {
    pub draft: String,
    pub trigger: Trigger,
    pub caret: Rect,
    pub context_snippets: Vec<ContextSnippet>,
    pub burst_id: Option<String>,
    pub destination_id: Option<String>,
    pub profile_id: Option<String>,
    pub word_offset: Option<u32>,
    /// True for sliding-window cadences that feed the edit accumulator only:
    /// results update word slots but never create a candidate-bar offer.
    pub barless: bool,
}

/// A settled prediction waiting for the user to act on it.
struct Offer {
    burst: Burst,
    candidates: Vec<String>,
    selected: usize,
    backend: Option<Backend>,
    latency_ms: Option<u64>,
    error: Option<ErrorInfo>,
    /// Learning-label source when a candidate is confirmed.
    source: &'static str,
}

/// How `apply_result` disposed of a finished prediction.
pub enum ApplyDisposition {
    /// The result produced (or refreshed) an offer; the snapshot is the
    /// current view, which may still be an older offer at the head of the
    /// queue.
    Offered(Snapshot),
    /// The result was consumed but offered nothing: zero candidates, or every
    /// candidate equaled the typed draft. The typed text stands.
    Skipped(Snapshot),
    /// The burst was retracted or superseded while inference ran; the result
    /// is dropped, never shown.
    Stale,
}

pub struct Engine {
    pub settings: AppSettings,
    pub learning: LearningStore,
    pub backend: FixtureBackend,
    pub metrics: Metrics,
    /// The burst currently awaiting a prediction, if any. Independent of the
    /// offer queue: inference on a new batch overlaps the display of earlier
    /// offers.
    in_flight: Option<Burst>,
    /// Settled offers, oldest first. The front is what the bar shows.
    offers: VecDeque<Offer>,
    /// Word-level correction evidence for the current composition session.
    session: edits::SessionEdits,
    /// Latest captured session context, used by hardened per-word edits whose
    /// evidence may span several overlapping bursts.
    session_context_snippets: Vec<ContextSnippet>,
    session_context_used_for_model: bool,
    /// The most recent burst that entered the pipeline: the consolidation
    /// offer inherits its destination, profile, and caret.
    last_burst: Option<Burst>,
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
            in_flight: None,
            offers: VecDeque::new(),
            session: edits::SessionEdits::default(),
            session_context_snippets: Vec::new(),
            session_context_used_for_model: false,
            last_burst: None,
            burst_seq: 0,
            data_dir: data_dir.to_path_buf(),
        }
    }

    pub fn save_settings(&self) {
        self.settings.save(&self.data_dir);
    }

    /// Starts a burst: builds the bounded prediction request and puts the
    /// burst in flight. Returns the request for the caller to execute so the
    /// lock is not held during inference. Visible offers are untouched: they
    /// stay on display (and acceptable) while the new burst infers.
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
            return Err(self.current_snapshot());
        }
        self.burst_seq += 1;
        let context_used_for_model = self.settings.window_context;
        let captured_context = input.context_snippets;
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
            context_snippets: captured_context.clone(),
            context_used_for_model,
            word_offset: input.word_offset,
            barless: input.barless,
        };
        let request = PredictionRequest {
            request_id: format!("req_{}", burst.burst_id),
            profile_id: burst.profile_id.clone(),
            model_variant: burst.model_variant,
            draft: burst.draft.clone(),
            context_snippets: if context_used_for_model {
                captured_context.clone()
            } else {
                Vec::new()
            },
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
        self.last_burst = Some(burst.clone());
        self.session_context_snippets = captured_context;
        self.session_context_used_for_model = context_used_for_model;
        self.in_flight = Some(burst);
        Ok((snapshot, request, self.settings.backend_mode))
    }

    /// Runs the fixture backend and records the result. Split from
    /// `begin_burst` so the async caller can simulate latency between the two
    /// without holding the lock. Live results come from the sidecar outside
    /// the engine lock entirely (a live inference can take a second; holding
    /// the lock that long stalls every synchronous command on the main
    /// thread) and re-enter through `record_result`.
    pub fn predict_fixture(&mut self, request: &PredictionRequest) -> PredictionResult {
        let result = self.backend.predict(request);
        self.record_result(request, result)
    }

    /// Counts a finished prediction and downgrades schema-invalid results to
    /// an explicit error, whichever backend produced them.
    pub fn record_result(
        &mut self,
        request: &PredictionRequest,
        result: PredictionResult,
    ) -> PredictionResult {
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

    /// Applies a finished prediction. Candidates equal to the typed draft are
    /// filtered out — leaving the text alone is not a correction — and a
    /// result with nothing left is skipped without an offer. Every consumed
    /// result also feeds the session edit accumulator when the burst carries
    /// a word offset: changed candidates as correction evidence, no-change
    /// results as decay ("the text is fine" is evidence too). A surviving
    /// result becomes an offer unless the burst was barless: it replaces an
    /// existing offer whose draft it extends (a grown burst re-predicting
    /// the same words), or queues behind unrelated offers so an earlier
    /// batch is never skipped by a later one.
    pub fn apply_result(&mut self, burst_id: &str, result: PredictionResult) -> ApplyDisposition {
        if self
            .in_flight
            .as_ref()
            .is_none_or(|burst| burst.burst_id != burst_id)
        {
            return ApplyDisposition::Stale;
        }
        let burst = self.in_flight.take().expect("checked above");

        let (candidates, backend, latency_ms, error) = match result {
            PredictionResult::Ok {
                candidates,
                votes,
                backend,
                latency_ms,
                ..
            } => {
                // Filter no-change candidates, keeping votes aligned.
                let votes = votes.unwrap_or_else(|| vec![1; candidates.len()]);
                let sampled = candidates.len();
                let (changed, changed_votes): (Vec<String>, Vec<u32>) = candidates
                    .into_iter()
                    .zip(votes)
                    .filter(|(candidate, _)| candidate.trim() != burst.draft)
                    .unzip();
                if changed.is_empty() {
                    if let Some(offset) = burst.word_offset {
                        self.session
                            .observe_no_change(offset as usize, &burst.draft);
                    }
                    // The typed text stands; the user is never interrupted.
                    return ApplyDisposition::Skipped(self.current_snapshot());
                }
                if let Some(offset) = burst.word_offset {
                    self.session.observe(
                        offset as usize,
                        &burst.draft,
                        &changed[0],
                        edits::PassSignal {
                            top_votes: Some(changed_votes[0]),
                            // The raw samples deduplicated to one distinct
                            // changed candidate: unanimity.
                            unanimous: sampled == 1,
                        },
                    );
                }
                (changed, Some(backend), Some(latency_ms), None)
            }
            // Failures surface as an explicit error chip in the bar; the
            // typed text is untouched either way.
            PredictionResult::Error { error, .. } => (Vec::new(), None, None, Some(error)),
        };

        if burst.barless {
            // Sliding-window cadence: evidence only, never a bar.
            return ApplyDisposition::Skipped(self.current_snapshot());
        }

        let offer = Offer {
            burst,
            candidates,
            selected: 0,
            backend,
            latency_ms,
            error,
            source: "confirmed_candidate",
        };
        if let Some(existing) = self
            .offers
            .iter_mut()
            .find(|existing| offer.burst.draft.starts_with(&existing.burst.draft))
        {
            *existing = offer;
        } else {
            self.offers.push_back(offer);
        }
        ApplyDisposition::Offered(self.current_snapshot())
    }

    /// Moves the highlight left/right through the shown offer's candidates
    /// with wrap-around (arrow keys). Returns None when no candidates are
    /// visible.
    pub fn move_selection(&mut self, delta: i64) -> Option<Snapshot> {
        let offer = self.offers.front_mut()?;
        if offer.candidates.is_empty() {
            return None;
        }
        let len = offer.candidates.len() as i64;
        offer.selected = (offer.selected as i64 + delta).rem_euclid(len) as usize;
        Some(self.current_snapshot())
    }

    /// Replaces the shown offer's burst with the selected candidate. Only
    /// ever called with an explicit selection (number key or click); there is
    /// no auto-apply. Queued offers and the in-flight burst survive: the next
    /// offer surfaces right after.
    pub fn select(&mut self, index: usize) -> Result<(Snapshot, CommitOutcome), String> {
        let offer = self.offers.front().ok_or("no suggestions to select")?;
        let text = offer
            .candidates
            .get(index)
            .ok_or("candidate index out of range")?
            .clone();
        let offer = self.offers.pop_front().expect("checked above");
        let source = offer.source;
        let burst = offer.burst;

        let outcome = commit::replace_burst(&burst.destination_id, &burst.burst_id, &text)?;
        if !self.settings.learning_paused {
            self.learning.append_example(&LabeledExample {
                ts_ms: learning::now_ms(),
                burst_id: burst.burst_id.clone(),
                profile_id: burst.profile_id.clone(),
                draft: burst.draft.clone(),
                label: "replace".to_string(),
                committed: text.clone(),
                source: source.to_string(),
                model_variant: burst.model_variant,
                context_snippets: burst.context_snippets.clone(),
                context_used_for_model: burst.context_used_for_model,
            });
            for (shorthand, expansion) in learning::extract_patterns(&burst.draft, &text) {
                self.learning
                    .record_pattern(&burst.profile_id, &shorthand, &expansion);
            }
        }
        // The commit changed word counts in the destination: keep the
        // session's word indices true and resolve overlapping slots.
        if let Some(offset) = burst.word_offset {
            let old_len = burst.draft.split_whitespace().count();
            self.session
                .shift_after_commit(offset as usize, old_len, &text);
        }
        Ok((
            Snapshot::Applied {
                burst_id: burst.burst_id,
                destination_id: outcome.destination_id.clone(),
                text: outcome.text.clone(),
            },
            outcome,
        ))
    }

    /// Dismisses the shown offer (Escape). Changes nothing in the
    /// destination; a stable dismissal of real candidates becomes a `keep`
    /// example per the spec. The next queued offer (or the in-flight burst)
    /// becomes the current view.
    pub fn dismiss(&mut self) -> Snapshot {
        if let Some(offer) = self.offers.pop_front() {
            self.record_keep(&offer);
        }
        self.current_snapshot()
    }

    /// Ends the composition session (sentence boundary, or the destination
    /// was edited out from under the tracked burst): the visible offer counts
    /// as a stable dismissal, queued offers and the in-flight burst are
    /// dropped without labels. If the session accumulated hardened word
    /// corrections that were never applied, they consolidate into one final
    /// full-sentence offer — accepting it replaces the whole sentence,
    /// dismissing it changes nothing.
    pub fn end_session(&mut self) -> Snapshot {
        if let Some(offer) = self.offers.pop_front() {
            self.record_keep(&offer);
        }
        self.offers.clear();
        self.in_flight = None;

        let consolidated = self.session.consolidated();
        let original = self.session.original_text();
        self.session = edits::SessionEdits::default();
        if let (Some(text), Some(last)) = (consolidated, self.last_burst.take()) {
            self.burst_seq += 1;
            let burst = Burst {
                burst_id: format!("consolidated_{}_{}", self.burst_seq, learning::now_ms()),
                destination_id: last.destination_id,
                profile_id: last.profile_id,
                draft: original.unwrap_or_default(),
                trigger: Trigger::Punctuation,
                caret: last.caret,
                model_variant: last.model_variant,
                context_snippets: last.context_snippets,
                context_used_for_model: last.context_used_for_model,
                // The session that produced it is already reset; accepting
                // this offer must not shift fresh-session indices.
                word_offset: None,
                barless: false,
            };
            self.offers.push_back(Offer {
                burst,
                candidates: vec![text],
                selected: 0,
                backend: None,
                latency_ms: None,
                error: None,
                source: "sentence_pass",
            });
            return self.current_snapshot();
        }
        self.last_burst = None;
        Snapshot::Idle
    }

    /// Abandons a composition whose destination text was destructively
    /// edited (for example, Select All + Delete). Unlike `end_session`, this
    /// is not a user decision about a visible candidate: it records no keep
    /// label and never synthesizes a consolidation offer from invalid text.
    pub fn cancel_session(&mut self) -> Snapshot {
        self.offers.clear();
        self.in_flight = None;
        self.session = edits::SessionEdits::default();
        self.session_context_snippets.clear();
        self.session_context_used_for_model = false;
        self.last_burst = None;
        Snapshot::Idle
    }

    /// Drops one burst wherever it is — a queued offer or the in-flight
    /// prediction — without recording a label. Used when editing invalidates
    /// the words the model saw.
    pub fn retract(&mut self, burst_id: &str) -> Snapshot {
        self.offers.retain(|offer| offer.burst.burst_id != burst_id);
        if self
            .in_flight
            .as_ref()
            .is_some_and(|burst| burst.burst_id == burst_id)
        {
            self.in_flight = None;
        }
        self.current_snapshot()
    }

    /// Every current word-slot proposal for the marks overlay and stats.
    pub fn marks(&self) -> Vec<edits::Mark> {
        self.session.marks()
    }

    /// Applies every hardened mark (⌘⏎ / apply-all): records one learning
    /// example per edit — cleaner shorthand→expansion pairs than a
    /// whole-burst diff — and returns the marks (pre-apply indices, oldest
    /// first) for the caller to replay onto the destination text right to
    /// left.
    pub fn apply_marks(&mut self) -> Vec<edits::Mark> {
        let applied = self.session.take_hardened();
        if !self.settings.learning_paused {
            let profile_id = self.settings.active_profile.clone();
            for mark in &applied {
                self.learning.append_example(&LabeledExample {
                    ts_ms: learning::now_ms(),
                    burst_id: format!("mark_{}_{}", mark.start_word, learning::now_ms()),
                    profile_id: profile_id.clone(),
                    draft: mark.original.clone(),
                    label: "replace".to_string(),
                    committed: mark.replacement.clone(),
                    source: "hardened_edit".to_string(),
                    model_variant: self.settings.model_variant,
                    context_snippets: self.session_context_snippets.clone(),
                    context_used_for_model: self.session_context_used_for_model,
                });
                for (shorthand, expansion) in
                    learning::extract_patterns(&mark.original, &mark.replacement)
                {
                    self.learning
                        .record_pattern(&profile_id, &shorthand, &expansion);
                }
            }
        }
        applied
    }

    /// Explicitly reverts the pending marks (Escape with no bar visible).
    /// Hardened marks record a keep example — the user saw a stable
    /// correction and chose their own text — and all word-slot evidence
    /// resets.
    pub fn clear_marks(&mut self) {
        if !self.settings.learning_paused {
            let profile_id = self.settings.active_profile.clone();
            for mark in self.session.marks() {
                if !mark.stable {
                    continue;
                }
                self.learning.append_example(&LabeledExample {
                    ts_ms: learning::now_ms(),
                    burst_id: format!("mark_{}_{}", mark.start_word, learning::now_ms()),
                    profile_id: profile_id.clone(),
                    draft: mark.original.clone(),
                    label: "keep".to_string(),
                    committed: String::new(),
                    source: "revert".to_string(),
                    model_variant: self.settings.model_variant,
                    context_snippets: self.session_context_snippets.clone(),
                    context_used_for_model: self.session_context_used_for_model,
                });
            }
        }
        self.session = edits::SessionEdits::default();
    }

    fn record_keep(&mut self, offer: &Offer) {
        if !offer.candidates.is_empty() && !self.settings.learning_paused {
            self.learning.append_example(&LabeledExample {
                ts_ms: learning::now_ms(),
                burst_id: offer.burst.burst_id.clone(),
                profile_id: offer.burst.profile_id.clone(),
                draft: offer.burst.draft.clone(),
                label: "keep".to_string(),
                committed: String::new(),
                source: "dismissal".to_string(),
                model_variant: offer.burst.model_variant,
                context_snippets: offer.burst.context_snippets.clone(),
                context_used_for_model: offer.burst.context_used_for_model,
            });
        }
    }

    /// Rebuilds the current UI snapshot, used to sync a webview on load and
    /// by the selftest to observe state between steps. The oldest unresolved
    /// offer outranks the in-flight burst: the bar never flickers back to a
    /// predicting state while something is still on display.
    pub fn current_snapshot(&self) -> Snapshot {
        if let Some(offer) = self.offers.front() {
            return Snapshot::Suggesting {
                burst_id: offer.burst.burst_id.clone(),
                draft: offer.burst.draft.clone(),
                candidates: offer.candidates.clone(),
                recommended: 0,
                selected: offer.selected,
                caret: offer.burst.caret,
                model_variant: offer.burst.model_variant,
                backend: offer.backend,
                latency_ms: offer.latency_ms,
                error: offer.error.clone(),
            };
        }
        if let Some(burst) = &self.in_flight {
            return Snapshot::Predicting {
                burst_id: burst.burst_id.clone(),
                draft: burst.draft.clone(),
                model_variant: burst.model_variant,
            };
        }
        Snapshot::Idle
    }

    /// Fixture-mode health. Live health is answered by the sidecar client
    /// outside the engine lock (see `get_health` in `main.rs`).
    pub fn fixture_health(&self) -> SidecarHealth {
        self.backend.health()
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
            context_snippets: Vec::new(),
            burst_id: None,
            destination_id: Some("destination_test".to_string()),
            profile_id: None,
            word_offset: None,
            barless: false,
        }
    }

    fn settled(disposition: ApplyDisposition) -> Snapshot {
        match disposition {
            ApplyDisposition::Offered(snapshot) | ApplyDisposition::Skipped(snapshot) => snapshot,
            ApplyDisposition::Stale => panic!("result was unexpectedly stale"),
        }
    }

    fn typed_with_context(draft: &str) -> BurstInput {
        BurstInput {
            context_snippets: vec![ContextSnippet {
                app_name: "Notes".to_string(),
                window_title: "Trip planning".to_string(),
                visible_text: "Tomorrow: meet at Union Station at 8:30 AM.".to_string(),
            }],
            ..typed(draft)
        }
    }

    fn run_burst(engine: &mut Engine, draft: &str) -> Snapshot {
        let (_, request, _) = engine.begin_burst(typed(draft)).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        let result = engine.predict_fixture(&request);
        settled(engine.apply_result(&burst_id, result))
    }

    fn shown_candidates(engine: &Engine) -> Vec<String> {
        match engine.current_snapshot() {
            Snapshot::Suggesting { candidates, .. } => candidates,
            other => panic!("expected suggesting, got {other:?}"),
        }
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
    fn window_context_setting_controls_prediction_context() {
        let (mut engine, dir) = test_engine();
        engine.settings.window_context = true;
        let (_, request, _) = engine
            .begin_burst(typed_with_context("meet there tmrw"))
            .unwrap();
        assert_eq!(request.context_snippets.len(), 1);

        engine.settings.window_context = false;
        let (_, request, _) = engine
            .begin_burst(typed_with_context("meet there tmrw"))
            .unwrap();
        assert!(request.context_snippets.is_empty());
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
    fn unchanged_candidates_are_filtered_and_never_shown() {
        let (mut engine, dir) = test_engine();
        let (_, request, _) = engine.begin_burst(typed("cnt cm tmrw")).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        // Every candidate equals the typed draft (modulo whitespace): the
        // whole result is skipped and no bar appears.
        let unchanged = PredictionResult::Ok {
            request_id: request.request_id.clone(),
            model_variant: request.model_variant,
            backend: Backend::Live,
            candidates: vec!["cnt cm tmrw".to_string(), " cnt cm tmrw ".to_string()],
            votes: None,
            latency_ms: 12,
        };
        assert!(matches!(
            engine.apply_result(&burst_id, unchanged),
            ApplyDisposition::Skipped(Snapshot::Idle)
        ));

        // A mixed result keeps only the changed candidate.
        let (_, request, _) = engine.begin_burst(typed("cnt cm tmrw")).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        let mixed = PredictionResult::Ok {
            request_id: request.request_id.clone(),
            model_variant: request.model_variant,
            backend: Backend::Live,
            candidates: vec![
                "cnt cm tmrw".to_string(),
                "Can't come tomorrow.".to_string(),
            ],
            votes: None,
            latency_ms: 12,
        };
        settled(engine.apply_result(&burst_id, mixed));
        assert_eq!(shown_candidates(&engine), vec!["Can't come tomorrow."]);
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
        engine.settings.window_context = false;
        let (_, request, _) = engine
            .begin_burst(typed_with_context("ship spec tn"))
            .unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        let result = engine.predict_fixture(&request);
        settled(engine.apply_result(&burst_id, result));
        let (snapshot, outcome) = engine.select(0).unwrap();
        assert!(matches!(snapshot, Snapshot::Applied { .. }));
        assert_eq!(outcome.destination_id, "destination_test");
        assert_eq!(outcome.text, "Ship spec tonight.");
        let raw = std::fs::read_to_string(dir.join("profiles/profile_a/examples.jsonl")).unwrap();
        assert!(raw.contains("\"confirmed_candidate\""));
        let saved: serde_json::Value = serde_json::from_str(raw.trim()).unwrap();
        assert_eq!(saved["context_snippets"][0]["app_name"], "Notes");
        assert_eq!(saved["context_used_for_model"], false);
        // Selecting twice is impossible: the offer was consumed.
        assert!(engine.select(0).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_real_commit_does_not_learn() {
        let (mut engine, dir) = test_engine();
        let mut input = typed("ship spec tn");
        input.destination_id = Some("destination_missing".to_string());
        let (_, request, _mode) = engine.begin_burst(input).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        let result = engine.predict_fixture(&request);
        engine.apply_result(&burst_id, result);

        let error = engine.select(0).unwrap_err();

        assert!(error.contains("real accessibility commit failed"));
        assert!(!dir.join("profiles/profile_a/examples.jsonl").exists());
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
        // No selection to move once the queue is empty.
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
    fn destructive_reset_cancels_without_learning_or_consolidating() {
        let (mut engine, dir) = test_engine();
        run_burst(&mut engine, "cnt cm tmrw");

        assert_eq!(engine.cancel_session(), Snapshot::Idle);
        assert_eq!(engine.current_snapshot(), Snapshot::Idle);
        assert!(engine.marks().is_empty());
        assert!(!dir.join("profiles/profile_a/examples.jsonl").exists());

        // A new composer session can begin immediately after the reset.
        let snapshot = run_burst(&mut engine, "ship spec tn");
        assert!(matches!(snapshot, Snapshot::Suggesting { .. }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn later_batches_queue_behind_the_shown_offer() {
        let (mut engine, dir) = test_engine();
        run_burst(&mut engine, "cnt cm tmrw");
        // A disjoint second batch settles while the first is on display: it
        // queues behind instead of replacing what the user is looking at.
        let snapshot = run_burst(&mut engine, "ship spec tn");
        let Snapshot::Suggesting { candidates, .. } = snapshot else {
            panic!("expected suggesting");
        };
        assert_eq!(candidates[0], "Can't come tomorrow.");
        // Resolving the first offer surfaces the second immediately.
        let next = engine.dismiss();
        let Snapshot::Suggesting { candidates, .. } = next else {
            panic!("expected the queued offer, got {next:?}");
        };
        assert_eq!(candidates[0], "Ship spec tonight.");
        // Accepting the queued offer works exactly like a fresh one.
        let (_, outcome) = engine.select(0).unwrap();
        assert_eq!(outcome.text, "Ship spec tonight.");
        assert_eq!(engine.current_snapshot(), Snapshot::Idle);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn ok_result(request: &PredictionRequest, candidates: &[&str]) -> PredictionResult {
        PredictionResult::Ok {
            request_id: request.request_id.clone(),
            model_variant: request.model_variant,
            backend: Backend::Live,
            candidates: candidates.iter().map(ToString::to_string).collect(),
            votes: None,
            latency_ms: 12,
        }
    }

    #[test]
    fn grown_burst_replaces_its_shorter_offer_in_place() {
        let (mut engine, dir) = test_engine();
        let (_, request, _) = engine.begin_burst(typed("cnt cm")).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        settled(engine.apply_result(&burst_id, ok_result(&request, &["Can't come."])));
        // The typist paused mid-chunk (offer for "cnt cm"), then continued:
        // the grown burst re-predicts a superset of the same words, so its
        // result replaces the stale offer instead of queueing behind it.
        let (_, request, _) = engine.begin_burst(typed("cnt cm tmrw")).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        settled(engine.apply_result(&burst_id, ok_result(&request, &["Can't come tomorrow."])));
        assert_eq!(shown_candidates(&engine), vec!["Can't come tomorrow."]);
        engine.dismiss();
        assert_eq!(engine.current_snapshot(), Snapshot::Idle);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn typing_on_leaves_the_shown_offer_standing() {
        let (mut engine, dir) = test_engine();
        run_burst(&mut engine, "cnt cm tmrw");
        // The next batch starts inferring while the offer is on display: the
        // offer stays, and no keep label is recorded (not a dismissal).
        let _ = engine.begin_burst(typed("ok going now")).unwrap();
        assert_eq!(shown_candidates(&engine)[0], "Can't come tomorrow.");
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
    fn retracted_bursts_drop_their_results_as_stale() {
        let (mut engine, dir) = test_engine();
        let (_, request, _) = engine.begin_burst(typed("cnt cm tmrw")).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        let result = engine.predict_fixture(&request);
        // Editing invalidated the words the model saw before the result
        // landed: the in-flight burst is retracted and the result dropped.
        engine.retract(&burst_id);
        assert!(matches!(
            engine.apply_result(&burst_id, result),
            ApplyDisposition::Stale
        ));
        // Retract also removes a settled offer without a keep label.
        run_burst(&mut engine, "cnt cm tmrw");
        engine.retract("burst_2_unknown"); // unknown ids are a no-op
        assert!(matches!(
            engine.current_snapshot(),
            Snapshot::Suggesting { .. }
        ));
        let shown_id = match engine.current_snapshot() {
            Snapshot::Suggesting { burst_id, .. } => burst_id,
            _ => unreachable!(),
        };
        engine.retract(&shown_id);
        assert_eq!(engine.current_snapshot(), Snapshot::Idle);
        assert!(!dir.join("profiles/profile_a/examples.jsonl").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn end_session_dismisses_shown_and_drops_the_rest() {
        let (mut engine, dir) = test_engine();
        run_burst(&mut engine, "cnt cm tmrw");
        run_burst(&mut engine, "ship spec tn");
        let _ = engine.begin_burst(typed("still typing mo")).unwrap();
        assert_eq!(engine.end_session(), Snapshot::Idle);
        assert_eq!(engine.current_snapshot(), Snapshot::Idle);
        // Only the visible offer counts as a stable dismissal.
        let raw = std::fs::read_to_string(dir.join("profiles/profile_a/examples.jsonl")).unwrap();
        assert_eq!(raw.matches("\"keep\"").count(), 1);
        assert!(raw.contains("cnt cm tmrw"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A barless sliding-window burst at a session word offset.
    fn typed_at(draft: &str, offset: u32) -> BurstInput {
        BurstInput {
            word_offset: Some(offset),
            barless: true,
            ..typed(draft)
        }
    }

    fn voted_result(
        request: &PredictionRequest,
        candidates: &[&str],
        votes: &[u32],
    ) -> PredictionResult {
        PredictionResult::Ok {
            request_id: request.request_id.clone(),
            model_variant: request.model_variant,
            backend: Backend::Live,
            candidates: candidates.iter().map(ToString::to_string).collect(),
            votes: Some(votes.to_vec()),
            latency_ms: 12,
        }
    }

    #[test]
    fn barless_bursts_harden_marks_without_offers() {
        let (mut engine, dir) = test_engine();
        let (_, request, _) = engine.begin_burst(typed_at("cnt cm tmrw", 0)).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        // 4-of-5 votes on the correction: very strong, but the caret has not
        // moved past it yet, and no bar ever opens in barless mode.
        let disposition = engine.apply_result(
            &burst_id,
            voted_result(
                &request,
                &["Can't come tomorrow.", "cnt come tmrw"],
                &[4, 1],
            ),
        );
        assert!(matches!(disposition, ApplyDisposition::Skipped(_)));
        assert_eq!(engine.current_snapshot(), Snapshot::Idle);
        assert!(engine.marks().iter().all(|m| !m.stable));

        // The next window reports the following words are fine; the caret is
        // now two words past the correction and it hardens.
        let (_, request, _) = engine.begin_burst(typed_at("ok going now", 3)).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        engine.apply_result(&burst_id, voted_result(&request, &["ok going now"], &[5]));
        let marks = engine.marks();
        let stable: Vec<_> = marks.iter().filter(|m| m.stable).collect();
        assert_eq!(stable.len(), 1);
        assert_eq!(stable[0].original, "cnt cm tmrw");
        assert_eq!(stable[0].replacement, "Can't come tomorrow.");

        // Apply-all records one example per edit and clears the mark.
        let applied = engine.apply_marks();
        assert_eq!(applied.len(), 1);
        assert!(engine.marks().iter().all(|m| !m.stable));
        let raw = std::fs::read_to_string(dir.join("profiles/profile_a/examples.jsonl")).unwrap();
        assert!(raw.contains("\"hardened_edit\""));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn end_session_consolidates_unapplied_marks_into_one_offer() {
        let (mut engine, dir) = test_engine();
        let (_, request, _) = engine.begin_burst(typed_at("cnt cm tmrw", 0)).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        engine.apply_result(
            &burst_id,
            voted_result(&request, &["Can't come tomorrow."], &[5]),
        );
        let (_, request, _) = engine.begin_burst(typed_at("ok going now", 3)).unwrap();
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        engine.apply_result(&burst_id, voted_result(&request, &["ok going now"], &[5]));

        // Sentence boundary with a hardened, never-applied correction: one
        // final offer holds the whole corrected sentence.
        let snapshot = engine.end_session();
        let Snapshot::Suggesting {
            candidates,
            burst_id,
            draft,
            ..
        } = snapshot
        else {
            panic!("expected the consolidation offer, got {snapshot:?}");
        };
        assert!(burst_id.starts_with("consolidated_"));
        assert_eq!(candidates, vec!["Can't come tomorrow. ok going now"]);
        assert_eq!(draft, "cnt cm tmrw ok going now");
        let (_, outcome) = engine.select(0).unwrap();
        assert_eq!(outcome.text, "Can't come tomorrow. ok going now");
        let raw = std::fs::read_to_string(dir.join("profiles/profile_a/examples.jsonl")).unwrap();
        assert!(raw.contains("\"sentence_pass\""));
        // A second end_session finds a fresh session: no offer, plain idle.
        assert_eq!(engine.end_session(), Snapshot::Idle);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recorded_live_errors_flow_through_apply_result() {
        let (mut engine, dir) = test_engine();
        engine.settings.backend_mode = BackendMode::Live;
        let (_, request, mode) = engine.begin_burst(typed("cnt cm tmrw")).unwrap();
        assert_eq!(mode, BackendMode::Live);
        let burst_id = request.request_id.strip_prefix("req_").unwrap().to_string();
        // The sidecar answers outside the engine lock; its result re-enters
        // through record_result and behaves like any other prediction.
        let raw = PredictionResult::Error {
            request_id: request.request_id.clone(),
            model_variant: request.model_variant,
            error: ErrorInfo {
                code: "sidecar_unavailable".to_string(),
                message: "The local inference sidecar is unavailable.".to_string(),
                retryable: true,
            },
        };
        let result = engine.record_result(&request, raw);
        let snapshot = settled(engine.apply_result(&burst_id, result));
        let Snapshot::Suggesting { error, .. } = snapshot else {
            panic!("expected suggesting");
        };
        assert_eq!(error.unwrap().code, "sidecar_unavailable");
        assert_eq!(engine.metrics.errors, 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
