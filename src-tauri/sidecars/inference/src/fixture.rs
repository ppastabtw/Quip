use std::{collections::BTreeSet, error::Error, fmt};

use quip_contracts::{
    ErrorInfo, FixtureFile, HealthStatus, LoadedArtifacts, PredictionExchange, PredictionRequest,
    PredictionResult, SidecarHealth,
};

use crate::InferenceBackend;

const PHASE_0_FIXTURES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/fixtures/phase-0-examples.json"
));

#[derive(Debug)]
pub struct FixtureBackendError(serde_json::Error);

impl fmt::Display for FixtureBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "embedded Phase 0 fixtures are invalid: {}",
            self.0
        )
    }
}

impl Error for FixtureBackendError {}

/// A local-only backend that deterministically replays the shared fixtures.
#[derive(Debug, Clone)]
pub struct FixtureBackend {
    fixtures: FixtureFile,
}

impl FixtureBackend {
    pub fn embedded() -> Result<Self, FixtureBackendError> {
        let fixtures = serde_json::from_str(PHASE_0_FIXTURES).map_err(FixtureBackendError)?;
        Ok(Self { fixtures })
    }

    /// Returns the ready health fixture by default. A fixture case ID is an
    /// internal testing selector and is not part of the shared health value.
    pub fn health(&self, case_id: Option<&str>) -> SidecarHealth {
        let case_id = case_id.unwrap_or("fixture_ready");
        self.fixtures
            .health_cases
            .iter()
            .find(|case| case.case_id == case_id)
            .map(|case| case.health.clone())
            .unwrap_or_else(|| SidecarHealth {
                status: HealthStatus::Unavailable,
                fixture_available: true,
                loaded: LoadedArtifacts {
                    base: false,
                    global_adapter: false,
                    user_adapter: false,
                },
                error: Some(ErrorInfo {
                    code: "fixture_health_case_not_found".to_owned(),
                    message: "The requested health fixture is unavailable.".to_owned(),
                    retryable: false,
                }),
            })
    }

    /// Looks up a fixture by semantic request fields while allowing callers to
    /// supply a fresh request ID. The original fixture request ID acts as a
    /// test-only tie-breaker for duplicate semantic cases such as the injected
    /// missing-adapter failure.
    pub fn predict(&self, request: &PredictionRequest) -> PredictionResult {
        let exact = self
            .fixtures
            .prediction_exchanges
            .iter()
            .find(|exchange| exchange.request == *request);

        let selected = exact.or_else(|| {
            self.fixtures
                .prediction_exchanges
                .iter()
                .filter(|exchange| semantically_matches(&exchange.request, request))
                .find(|exchange| matches!(&exchange.result, PredictionResult::Ok { .. }))
        });

        let result = selected
            .map(|exchange| rewrite_result(exchange, request))
            .unwrap_or_else(|| not_found_result(request));

        validate_before_return(result, request)
    }

    /// Phrases that can be tried without adding context or personal patterns.
    /// This powers the development phrase tester without duplicating fixture
    /// strings in its user interface.
    pub fn simple_example_drafts(&self) -> Vec<String> {
        self.fixtures
            .prediction_exchanges
            .iter()
            .filter(|exchange| {
                exchange.request.context_snippets.is_empty()
                    && exchange.request.personal_patterns.is_empty()
                    && matches!(&exchange.result, PredictionResult::Ok { .. })
            })
            .map(|exchange| exchange.request.draft.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
}

impl InferenceBackend for FixtureBackend {
    fn health(&self, case_id: Option<&str>) -> SidecarHealth {
        self.health(case_id)
    }

    fn predict(&self, request: &PredictionRequest) -> PredictionResult {
        self.predict(request)
    }
}

fn semantically_matches(fixture: &PredictionRequest, request: &PredictionRequest) -> bool {
    fixture.profile_id == request.profile_id
        && fixture.model_variant == request.model_variant
        && fixture.draft == request.draft
        && fixture.context_snippets == request.context_snippets
        && fixture.personal_patterns == request.personal_patterns
}

fn rewrite_result(exchange: &PredictionExchange, request: &PredictionRequest) -> PredictionResult {
    match &exchange.result {
        PredictionResult::Ok {
            backend,
            candidates,
            latency_ms,
            ..
        } => PredictionResult::Ok {
            request_id: request.request_id.clone(),
            model_variant: request.model_variant,
            backend: *backend,
            candidates: candidates.clone(),
            // Fixture replays cannot vote; the contract says emit all 1s.
            votes: Some(vec![1; candidates.len()]),
            latency_ms: *latency_ms,
        },
        PredictionResult::Error { error, .. } => PredictionResult::Error {
            request_id: request.request_id.clone(),
            model_variant: request.model_variant,
            error: error.clone(),
        },
    }
}

fn not_found_result(request: &PredictionRequest) -> PredictionResult {
    PredictionResult::Error {
        request_id: request.request_id.clone(),
        model_variant: request.model_variant,
        error: ErrorInfo {
            code: "fixture_not_found".to_owned(),
            message: "No deterministic fixture matches this request.".to_owned(),
            retryable: false,
        },
    }
}

fn validate_before_return(
    result: PredictionResult,
    request: &PredictionRequest,
) -> PredictionResult {
    let invalid_reason = result.validate().err().or_else(|| match &result {
        PredictionResult::Ok { candidates, .. }
            if candidates
                .iter()
                .any(|candidate| candidate == &request.draft) =>
        {
            Some("fixture returned the exact draft as a candidate".to_owned())
        }
        _ => None,
    });

    match invalid_reason {
        Some(reason) => PredictionResult::Error {
            request_id: request.request_id.clone(),
            model_variant: request.model_variant,
            error: ErrorInfo {
                code: "invalid_fixture_result".to_owned(),
                message: reason,
                retryable: false,
            },
        },
        None => result,
    }
}

#[cfg(test)]
mod tests {
    use quip_contracts::{Backend, ModelVariant, PredictionRequest};

    use super::FixtureBackend;

    fn shorthand_request(request_id: &str, model_variant: ModelVariant) -> PredictionRequest {
        PredictionRequest {
            request_id: request_id.to_owned(),
            profile_id: "profile_default".to_owned(),
            model_variant,
            draft: "cnt cm tmrw".to_owned(),
            context_snippets: vec![],
            personal_patterns: vec![],
        }
    }

    #[test]
    fn fresh_request_id_replays_a_semantic_fixture() {
        let backend = FixtureBackend::embedded().unwrap();
        let result = backend.predict(&shorthand_request("fresh-id", ModelVariant::Base));

        assert!(matches!(
            result,
            quip_contracts::PredictionResult::Ok {
                request_id,
                model_variant: ModelVariant::Base,
                backend: Backend::Fixture,
                ..
            } if request_id == "fresh-id"
        ));
    }

    #[test]
    fn original_request_id_selects_the_missing_adapter_fixture() {
        let backend = FixtureBackend::embedded().unwrap();
        let result = backend.predict(&shorthand_request(
            "pred_missing_adapter",
            ModelVariant::Global,
        ));

        assert!(matches!(
            result,
            quip_contracts::PredictionResult::Error { error, .. }
                if error.code == "adapter_not_loaded"
        ));
    }

    #[test]
    fn health_supports_ready_and_degraded_cases() {
        let backend = FixtureBackend::embedded().unwrap();
        assert_eq!(
            backend.health(None).status,
            quip_contracts::HealthStatus::Ready
        );
        assert_eq!(
            backend.health(Some("adapter_degraded")).status,
            quip_contracts::HealthStatus::Degraded
        );
    }

    #[test]
    fn simple_examples_only_include_phrases_that_need_no_extra_input() {
        let backend = FixtureBackend::embedded().unwrap();
        assert_eq!(
            backend.simple_example_drafts(),
            vec![
                "cnt cm tmrw".to_owned(),
                "open usr/bin and q3_finl_v2.pdf".to_owned(),
            ]
        );
    }
}
