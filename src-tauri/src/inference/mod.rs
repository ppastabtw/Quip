//! Workstream 4 client / Workstream 2 boundary: prediction backends.
//!
//! The deterministic [`FixtureBackend`] answers from the shared Phase 0
//! fixtures plus the demo corpus, and applies personal-pattern substitution
//! for `global_plus_personal` so two profiles diverge before any model
//! exists. Live mode keeps one Workstream 2 sidecar child alive and exchanges
//! newline-delimited JSON commands over stdin/stdout. Every result is
//! schema-validated and counted.

mod sidecar_client;

pub use sidecar_client::SidecarClient;

use quip_contracts::{
    Backend, ContextSnippet, ErrorInfo, FixtureFile, HealthStatus, LoadedArtifacts, ModelVariant,
    PredictionRequest, PredictionResult, SidecarHealth,
};
use serde::{Deserialize, Serialize};

const PHASE0_FIXTURES: &str = include_str!("../../../docs/fixtures/phase-0-examples.json");
const DEMO_CORPUS: &str = include_str!("../../fixtures/demo_corpus.json");

/// One deterministic lookup entry: a known draft under a known variant and
/// context presence maps to a fixed successful result.
#[derive(Debug, Clone, Deserialize)]
struct CorpusEntry {
    draft: String,
    model_variant: ModelVariant,
    has_context: bool,
    candidates: Vec<String>,
    latency_ms: u64,
}

/// One side of a demo comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideSpec {
    pub label: String,
    pub model_variant: ModelVariant,
    pub profile_id: String,
    pub use_context: bool,
}

/// One deterministic corpus case shown in the demo harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DemoCase {
    pub case_id: String,
    pub title: String,
    pub description: String,
    pub draft: String,
    pub context_snippets: Vec<ContextSnippet>,
    pub left: SideSpec,
    pub right: SideSpec,
}

#[derive(Deserialize)]
struct CorpusFile {
    entries: Vec<CorpusEntry>,
    cases: Vec<DemoCase>,
}

pub struct FixtureBackend {
    entries: Vec<CorpusEntry>,
    pub cases: Vec<DemoCase>,
    /// Demo control: forces the missing-adapter failure path.
    pub simulate_failure: bool,
}

impl FixtureBackend {
    pub fn new() -> Self {
        let fixtures: FixtureFile =
            serde_json::from_str(PHASE0_FIXTURES).expect("phase 0 fixtures must parse");
        let corpus: CorpusFile =
            serde_json::from_str(DEMO_CORPUS).expect("demo corpus must parse");

        // Successful fixture exchanges become lookup entries; the error
        // fixture is reachable through `simulate_failure` instead. Corpus
        // entries come first so they can override a fixture draft with a
        // richer candidate list (lookup takes the first match).
        let mut entries = corpus.entries;
        entries.extend(fixtures.prediction_exchanges.iter().filter_map(
            |exchange| match &exchange.result {
                PredictionResult::Ok {
                    candidates,
                    latency_ms,
                    model_variant,
                    ..
                } => Some(CorpusEntry {
                    draft: exchange.request.draft.clone(),
                    model_variant: *model_variant,
                    has_context: !exchange.request.context_snippets.is_empty(),
                    candidates: candidates.clone(),
                    latency_ms: *latency_ms,
                }),
                PredictionResult::Error { .. } => None,
            },
        ));

        Self {
            entries,
            cases: corpus.cases,
            simulate_failure: false,
        }
    }

    pub fn predict(&self, request: &PredictionRequest) -> PredictionResult {
        if self.simulate_failure {
            return PredictionResult::Error {
                request_id: request.request_id.clone(),
                model_variant: request.model_variant,
                error: ErrorInfo {
                    code: "adapter_not_loaded".to_string(),
                    message: "The global adapter is unavailable.".to_string(),
                    retryable: false,
                },
            };
        }

        let has_context = !request.context_snippets.is_empty();

        // Personal patterns outrank draft lookup: the whole point of the
        // personal variant is that the same draft answers differently per
        // profile, so a shared fixture entry must not shadow it.
        if request.model_variant == ModelVariant::GlobalPlusPersonal {
            if let Some(candidates) = personal_substitute(request) {
                return ok_result(request, candidates, 641);
            }
        }

        if let Some(result) = self.lookup(request, request.model_variant, has_context) {
            return result;
        }

        // Personal variant with no applicable patterns: behave like global.
        if request.model_variant == ModelVariant::GlobalPlusPersonal {
            if let Some(result) = self.lookup(request, ModelVariant::Global, has_context) {
                return result;
            }
        }

        if request.model_variant == ModelVariant::Base {
            // The base model's demo signature: unnecessary surface edits on
            // unknown input instead of restraint.
            let rewritten = base_overedit(&request.draft);
            if rewritten != request.draft {
                return ok_result(request, vec![rewritten], 507);
            }
        }

        ok_result(request, Vec::new(), 356)
    }

    fn lookup(
        &self,
        request: &PredictionRequest,
        variant: ModelVariant,
        has_context: bool,
    ) -> Option<PredictionResult> {
        // Trailing terminators come along when the punctuation trigger fires
        // ("omw." should still match the "omw" entry).
        let draft = normalize_draft(&request.draft);
        self.entries
            .iter()
            .find(|e| {
                normalize_draft(&e.draft) == draft
                    && e.model_variant == variant
                    && e.has_context == has_context
            })
            .map(|e| ok_result(request, e.candidates.clone(), e.latency_ms))
    }

    pub fn case(&self, case_id: &str) -> Option<&DemoCase> {
        self.cases.iter().find(|c| c.case_id == case_id)
    }

    pub fn health(&self) -> SidecarHealth {
        let loaded = LoadedArtifacts {
            base: false,
            global_adapter: false,
            user_adapter: false,
        };
        if self.simulate_failure {
            SidecarHealth {
                status: HealthStatus::Degraded,
                fixture_available: true,
                loaded,
                error: Some(ErrorInfo {
                    code: "adapter_not_loaded".to_string(),
                    message: "The global adapter is unavailable.".to_string(),
                    retryable: false,
                }),
            }
        } else {
            SidecarHealth {
                status: HealthStatus::Ready,
                fixture_available: true,
                loaded,
                error: None,
            }
        }
    }
}

fn ok_result(
    request: &PredictionRequest,
    candidates: Vec<String>,
    latency_ms: u64,
) -> PredictionResult {
    PredictionResult::Ok {
        request_id: request.request_id.clone(),
        model_variant: request.model_variant,
        backend: Backend::Fixture,
        candidates,
        latency_ms,
    }
}

/// Applies the request's personal patterns token-by-token. Returns a tidied
/// best candidate plus the raw substitution as an alternative when they
/// differ; None when no pattern matched.
fn personal_substitute(request: &PredictionRequest) -> Option<Vec<String>> {
    let mut replaced = false;
    let tokens: Vec<String> = request
        .draft
        .split_whitespace()
        .map(|token| {
            let key = token
                .trim_matches(|c: char| c.is_ascii_punctuation())
                .to_lowercase();
            match request.personal_patterns.iter().find(|p| p.shorthand == key) {
                Some(pattern) => {
                    replaced = true;
                    token.to_lowercase().replace(&key, &pattern.expansion)
                }
                None => token.to_string(),
            }
        })
        .collect();
    if !replaced {
        return None;
    }
    let raw = tokens.join(" ");
    let tidied = base_overedit(&raw);
    let mut candidates = vec![tidied];
    if candidates[0] != raw {
        candidates.push(raw);
    }
    Some(candidates)
}

/// Lookup key normalization: case-insensitive, trailing sentence terminators
/// stripped, whitespace collapsed.
fn normalize_draft(draft: &str) -> String {
    draft
        .trim()
        .trim_end_matches(['.', '!', '?'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Capitalizes the first letter and appends a period — the deterministic
/// stand-in for a base model that edits when it should keep.
fn base_overedit(draft: &str) -> String {
    let mut text = draft.trim().to_string();
    if let Some(first) = text.get(..1) {
        let upper = first.to_uppercase();
        text.replace_range(..1, &upper);
    }
    if !text.is_empty() && !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    text
}

/// Demo-visible counters over every prediction the app has run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Metrics {
    pub requests: u64,
    pub ok: u64,
    pub errors: u64,
    pub schema_invalid: u64,
    pub last_latency_ms: Option<u64>,
    pub avg_latency_ms: Option<f64>,
}

impl Metrics {
    /// Counts a result and returns whether it satisfied the schema invariants.
    pub fn record(&mut self, result: &PredictionResult) -> bool {
        self.requests += 1;
        let valid = result.validate().is_ok();
        if !valid {
            self.schema_invalid += 1;
        }
        match result {
            PredictionResult::Ok { latency_ms, .. } => {
                self.ok += 1;
                self.last_latency_ms = Some(*latency_ms);
                let n = self.ok as f64;
                let prev = self.avg_latency_ms.unwrap_or(0.0);
                self.avg_latency_ms = Some(prev + (*latency_ms as f64 - prev) / n);
            }
            PredictionResult::Error { .. } => self.errors += 1,
        }
        valid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quip_contracts::PersonalPattern;

    fn request(draft: &str, variant: ModelVariant) -> PredictionRequest {
        PredictionRequest {
            request_id: "req_test".to_string(),
            profile_id: "profile_default".to_string(),
            model_variant: variant,
            draft: draft.to_string(),
            context_snippets: Vec::new(),
            personal_patterns: Vec::new(),
        }
    }

    fn candidates(result: &PredictionResult) -> Vec<String> {
        match result {
            PredictionResult::Ok { candidates, .. } => candidates.clone(),
            PredictionResult::Error { .. } => panic!("expected ok result"),
        }
    }

    #[test]
    fn fixture_lookup_distinguishes_variants() {
        let backend = FixtureBackend::new();
        let base = backend.predict(&request("open usr/bin and q3_finl_v2.pdf", ModelVariant::Base));
        let global =
            backend.predict(&request("open usr/bin and q3_finl_v2.pdf", ModelVariant::Global));
        assert_eq!(candidates(&base), vec!["Open /usr/bin and q3_final_v2.pdf."]);
        assert!(candidates(&global).is_empty());
    }

    #[test]
    fn context_presence_changes_the_answer() {
        let backend = FixtureBackend::new();
        let mut with_context = request("meet there tmrw", ModelVariant::Global);
        with_context.context_snippets.push(ContextSnippet {
            app_name: "Notes".into(),
            window_title: "Trip planning".into(),
            visible_text: "Tomorrow: meet at Union Station at 8:30 AM.".into(),
        });
        let without = backend.predict(&request("meet there tmrw", ModelVariant::Global));
        let with = backend.predict(&with_context);
        assert_eq!(candidates(&without)[0], "Meet there tomorrow.");
        assert_eq!(
            candidates(&with)[0],
            "Meet at Union Station at 8:30 AM tomorrow."
        );
    }

    #[test]
    fn personal_patterns_make_profiles_diverge() {
        let backend = FixtureBackend::new();
        let mut a = request("ship spec tn", ModelVariant::GlobalPlusPersonal);
        a.personal_patterns.push(PersonalPattern {
            shorthand: "tn".into(),
            expansion: "tonight".into(),
        });
        let mut b = request("ship spec tn", ModelVariant::GlobalPlusPersonal);
        b.personal_patterns.push(PersonalPattern {
            shorthand: "tn".into(),
            expansion: "tomorrow night".into(),
        });
        let a_candidates = candidates(&backend.predict(&a));
        let b_candidates = candidates(&backend.predict(&b));
        // Tidied best candidate plus the raw substitution as an alternative.
        assert_eq!(
            a_candidates,
            vec!["Ship spec tonight.", "ship spec tonight"]
        );
        assert_eq!(
            b_candidates,
            vec!["Ship spec tomorrow night.", "ship spec tomorrow night"]
        );
    }

    #[test]
    fn lookup_ignores_trailing_terminators_and_case() {
        let backend = FixtureBackend::new();
        let result = backend.predict(&request("Omw.", ModelVariant::Global));
        assert_eq!(candidates(&result)[0], "On my way.");
    }

    #[test]
    fn personal_variant_without_matching_patterns_falls_back_to_global() {
        let backend = FixtureBackend::new();
        let result = backend.predict(&request("cnt cm tmrw", ModelVariant::GlobalPlusPersonal));
        let list = candidates(&result);
        assert_eq!(list[0], "Can't come tomorrow.");
        // Corpus overrides the single-candidate fixture with alternatives.
        assert_eq!(list.len(), 5);
    }

    #[test]
    fn unknown_input_keeps_for_trained_and_overedits_for_base() {
        let backend = FixtureBackend::new();
        let trained = backend.predict(&request("totally novel text", ModelVariant::Global));
        assert!(candidates(&trained).is_empty());
        let base = backend.predict(&request("totally novel text", ModelVariant::Base));
        assert_eq!(candidates(&base), vec!["Totally novel text."]);
    }

    #[test]
    fn simulated_failure_reports_error_and_degraded_health() {
        let mut backend = FixtureBackend::new();
        backend.simulate_failure = true;
        let result = backend.predict(&request("cnt cm tmrw", ModelVariant::Global));
        assert!(matches!(result, PredictionResult::Error { ref error, .. }
            if error.code == "adapter_not_loaded"));
        assert_eq!(backend.health().status, HealthStatus::Degraded);
    }

    #[test]
    fn metrics_count_ok_errors_and_schema_validity() {
        let mut backend = FixtureBackend::new();
        let mut metrics = Metrics::default();
        assert!(metrics.record(&backend.predict(&request("cnt cm tmrw", ModelVariant::Global))));
        backend.simulate_failure = true;
        assert!(metrics.record(&backend.predict(&request("cnt cm tmrw", ModelVariant::Global))));
        let invalid = PredictionResult::Ok {
            request_id: "req_bad".into(),
            model_variant: ModelVariant::Global,
            backend: Backend::Fixture,
            candidates: (0..6).map(|index| format!("candidate {index}")).collect(),
            latency_ms: 1,
        };
        assert!(!metrics.record(&invalid));
        assert_eq!(metrics.requests, 3);
        assert_eq!(metrics.ok, 2);
        assert_eq!(metrics.errors, 1);
        assert_eq!(metrics.schema_invalid, 1);
    }

    #[test]
    fn every_demo_case_produces_two_distinct_valid_results() {
        let backend = FixtureBackend::new();
        assert_eq!(backend.cases.len(), 5);
        for case in &backend.cases {
            let build = |side: &SideSpec, patterns: Vec<PersonalPattern>| PredictionRequest {
                request_id: format!("req_{}", case.case_id),
                profile_id: side.profile_id.clone(),
                model_variant: side.model_variant,
                draft: case.draft.clone(),
                context_snippets: if side.use_context {
                    case.context_snippets.clone()
                } else {
                    Vec::new()
                },
                personal_patterns: patterns,
            };
            let seed = |profile: &str| -> Vec<PersonalPattern> {
                match profile {
                    "profile_a" => vec![PersonalPattern {
                        shorthand: "tn".into(),
                        expansion: "tonight".into(),
                    }],
                    "profile_b" => vec![PersonalPattern {
                        shorthand: "tn".into(),
                        expansion: "tomorrow night".into(),
                    }],
                    _ => Vec::new(),
                }
            };
            let left = backend.predict(&build(&case.left, seed(&case.left.profile_id)));
            let right = backend.predict(&build(&case.right, seed(&case.right.profile_id)));
            left.validate().unwrap();
            right.validate().unwrap();
            assert_ne!(
                left, right,
                "case {} must show a visible difference",
                case.case_id
            );
        }
    }
}
