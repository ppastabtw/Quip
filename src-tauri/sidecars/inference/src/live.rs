use std::{
    collections::HashMap,
    error::Error,
    fmt,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use quip_contracts::{
    Backend, ErrorInfo, HealthStatus, LoadedArtifacts, ModelVariant, PredictionRequest,
    PredictionResult, SidecarHealth,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::InferenceBackend;

const DEFAULT_MODEL_ADDR: &str = "127.0.0.1:1234";
const COMPLETION_COUNT: usize = 5;
const COMPLETION_ATTEMPTS: usize = 3;
const TEMPERATURE: f64 = 0.1;
const SYSTEM_PROMPT: &str = include_str!("../../../../training/flash/system_prompt.txt");

#[derive(Debug)]
pub enum LiveBackendError {
    InvalidAddress(String),
    Io(std::io::Error),
    InvalidHttpResponse(&'static str),
    HttpStatus { status: u16, body: String },
    InvalidJson(serde_json::Error),
    MissingModelContent,
}

impl fmt::Display for LiveBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAddress(address) => {
                write!(formatter, "invalid local model address: {address}")
            }
            Self::Io(error) => write!(formatter, "local model connection failed: {error}"),
            Self::InvalidHttpResponse(reason) => {
                write!(
                    formatter,
                    "local model returned an invalid HTTP response: {reason}"
                )
            }
            Self::HttpStatus { status, body } => {
                write!(formatter, "local model returned HTTP {status}: {body}")
            }
            Self::InvalidJson(error) => {
                write!(formatter, "local model returned invalid JSON: {error}")
            }
            Self::MissingModelContent => write!(formatter, "local model response had no content"),
        }
    }
}

impl Error for LiveBackendError {}

impl From<std::io::Error> for LiveBackendError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for LiveBackendError {
    fn from(error: serde_json::Error) -> Self {
        Self::InvalidJson(error)
    }
}

#[derive(Debug, Clone)]
pub struct LiveBackend {
    address: SocketAddr,
    timeout: Duration,
}

impl LiveBackend {
    pub fn local_default() -> Result<Self, LiveBackendError> {
        let address =
            std::env::var("QUIP_MODEL_ADDR").unwrap_or_else(|_| DEFAULT_MODEL_ADDR.to_owned());
        Self::new(&address)
    }

    pub fn new(address: &str) -> Result<Self, LiveBackendError> {
        let address: SocketAddr = address
            .parse()
            .map_err(|_| LiveBackendError::InvalidAddress(address.to_owned()))?;
        if !address.ip().is_loopback() {
            return Err(LiveBackendError::InvalidAddress(address.to_string()));
        }
        Ok(Self {
            address,
            timeout: Duration::from_secs(90),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<Vec<u8>, LiveBackendError> {
        let mut stream = TcpStream::connect_timeout(&self.address, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;

        let body = body.unwrap_or_default();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.address,
            body.len()
        )?;
        stream.write_all(body)?;
        stream.flush()?;

        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
        parse_http_response(&response)
    }

    fn predict_base(
        &self,
        request: &PredictionRequest,
    ) -> Result<PredictionResult, LiveBackendError> {
        let started = Instant::now();
        let model_outputs = self.complete_base(request)?;

        Ok(normalize_model_outputs(
            request,
            model_outputs,
            started.elapsed().as_millis() as u64,
        ))
    }

    fn complete_base(
        &self,
        request: &PredictionRequest,
    ) -> Result<Vec<String>, LiveBackendError> {
        let mut outputs = Vec::with_capacity(COMPLETION_COUNT);
        let mut last_error = None;
        for _ in 0..COMPLETION_ATTEMPTS {
            match self.complete_base_once(request, COMPLETION_COUNT - outputs.len()) {
                Ok(mut batch) => {
                    outputs.append(&mut batch);
                    if outputs.len() >= COMPLETION_COUNT {
                        outputs.truncate(COMPLETION_COUNT);
                        return Ok(outputs);
                    }
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or(LiveBackendError::MissingModelContent))
    }

    fn complete_base_once(
        &self,
        request: &PredictionRequest,
        completion_count: usize,
    ) -> Result<Vec<String>, LiveBackendError> {
        let user_content = model_input(request).to_string();
        let body = json!({
            "model": "default",
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": user_content}
            ],
            "enable_thinking": false,
            "temperature": TEMPERATURE,
            "n": completion_count,
            "max_tokens": 64,
            "stop": ["<|endoftext|>"]
        });
        let body = serde_json::to_vec(&body)?;
        let response = self.request("POST", "/v1/chat/completions", Some(&body))?;
        let response: ChatCompletion = serde_json::from_slice(&response)?;
        let mut choices = response.choices;
        choices.sort_by_key(|choice| choice.index);
        Ok(choices
            .into_iter()
            .filter_map(|choice| {
                choice
                    .message
                    .content
                    .map(|content| content.trim().to_owned())
                    .filter(|content| !content.is_empty())
            })
            .collect())
    }
}

impl InferenceBackend for LiveBackend {
    fn health(&self, _case_id: Option<&str>) -> SidecarHealth {
        match self.request("GET", "/health", None) {
            Ok(body) if body == b"OK" => SidecarHealth {
                status: HealthStatus::Ready,
                fixture_available: true,
                loaded: LoadedArtifacts {
                    base: true,
                    global_adapter: false,
                    user_adapter: false,
                },
                error: None,
            },
            Ok(_) => unavailable_health("The local model health response was not OK."),
            Err(_) => unavailable_health(
                "The local Qwen server is not reachable at the configured loopback address.",
            ),
        }
    }

    fn predict(&self, request: &PredictionRequest) -> PredictionResult {
        if request.model_variant != ModelVariant::Base {
            return prediction_error(
                request,
                "adapter_not_loaded",
                "The global Freesolo adapter is not loaded yet.",
                false,
            );
        }

        self.predict_base(request).unwrap_or_else(|error| {
            prediction_error(request, "live_inference_failed", &error.to_string(), true)
        })
    }
}

fn model_input(request: &PredictionRequest) -> Value {
    if request.context_snippets.is_empty() && request.personal_patterns.is_empty() {
        json!({"text": request.draft})
    } else {
        json!({
            "text": request.draft,
            "context_snippets": request.context_snippets,
            "personal_patterns": request.personal_patterns,
        })
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletion {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    index: usize,
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

fn normalize_model_outputs(
    request: &PredictionRequest,
    outputs: Vec<String>,
    latency_ms: u64,
) -> PredictionResult {
    if outputs.len() != COMPLETION_COUNT {
        return prediction_error(
            request,
            "incomplete_generation_batch",
            "live inference requires exactly five completed generations",
            true,
        );
    }

    let suggestions = outputs
        .into_iter()
        .map(|output| output.trim().to_owned())
        .collect::<Vec<_>>();

    if suggestions.iter().any(String::is_empty) {
        return prediction_error(
            request,
            "invalid_model_output",
            "suggestion must be a non-empty string",
            false,
        );
    }

    let mut votes = HashMap::<String, (usize, usize)>::new();
    for (index, suggestion) in suggestions.into_iter().enumerate() {
        if suggestion == request.draft
            || is_model_scaffolding(&suggestion)
            || is_implausibly_truncated(&request.draft, &suggestion)
        {
            continue;
        }
        let (vote_count, _) = votes.entry(suggestion).or_insert((0, index));
        *vote_count += 1;
    }
    let mut ranked = votes.into_iter().collect::<Vec<_>>();
    ranked.sort_by(
        |(_, (left_votes, left_index)), (_, (right_votes, right_index))| {
            right_votes
                .cmp(left_votes)
                .then_with(|| left_index.cmp(right_index))
        },
    );
    let candidates = ranked
        .into_iter()
        .take(5)
        .map(|(suggestion, _)| suggestion)
        .collect::<Vec<_>>();

    let result = PredictionResult::Ok {
        request_id: request.request_id.clone(),
        model_variant: request.model_variant,
        backend: Backend::Live,
        candidates,
        latency_ms,
    };

    match result.validate() {
        Ok(()) => result,
        Err(reason) => prediction_error(request, "invalid_model_output", &reason, false),
    }
}

/// Suppress literal schema/example text if a small model echoes prompt
/// scaffolding instead of producing a correction. Returning no candidate is
/// safer than offering text that was never derived from the user's draft.
fn is_model_scaffolding(suggestion: &str) -> bool {
    let normalized = suggestion.trim().to_ascii_lowercase();
    let repeats_prompt_policy = SYSTEM_PROMPT.lines().any(|line| {
        let line = line.trim().strip_prefix("- ").unwrap_or(line.trim());
        line.len() >= 12 && normalized.starts_with(&line.to_ascii_lowercase())
    });
    repeats_prompt_policy
        || normalized.starts_with("best full text")
        || normalized == "text"
        || normalized == "suggestion"
        || normalized.starts_with("suggestion:")
        || normalized.starts_with("suggestion suggestion:")
        || normalized.starts_with("uggestionuggestion:")
        || normalized.starts_with("text_")
        || normalized.starts_with("i'm quip")
        || normalized.starts_with("i am quip")
        || normalized.starts_with("you are quip")
        || matches!(normalized.chars().next(), Some('{' | '}' | '[' | ']'))
}

/// A full-text correction may expand shorthand, but conservatively reject a
/// candidate that drops one or more words from a multi-word draft. This catches
/// generic fragments without blocking same-length corrections or expansions.
fn is_implausibly_truncated(draft: &str, suggestion: &str) -> bool {
    let draft_words = draft.split_whitespace().count();
    let suggestion_words = suggestion.split_whitespace().count();
    draft_words >= 3 && suggestion_words < draft_words
}

fn prediction_error(
    request: &PredictionRequest,
    code: &str,
    message: &str,
    retryable: bool,
) -> PredictionResult {
    PredictionResult::Error {
        request_id: request.request_id.clone(),
        model_variant: request.model_variant,
        error: ErrorInfo {
            code: code.to_owned(),
            message: message.to_owned(),
            retryable,
        },
    }
}

fn unavailable_health(message: &str) -> SidecarHealth {
    SidecarHealth {
        status: HealthStatus::Unavailable,
        fixture_available: true,
        loaded: LoadedArtifacts {
            base: false,
            global_adapter: false,
            user_adapter: false,
        },
        error: Some(ErrorInfo {
            code: "live_backend_unavailable".to_owned(),
            message: message.to_owned(),
            retryable: true,
        }),
    }
}

fn parse_http_response(response: &[u8]) -> Result<Vec<u8>, LiveBackendError> {
    let separator = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(LiveBackendError::InvalidHttpResponse("missing headers"))?;
    let headers = std::str::from_utf8(&response[..separator])
        .map_err(|_| LiveBackendError::InvalidHttpResponse("headers were not UTF-8"))?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or(LiveBackendError::InvalidHttpResponse("missing status code"))?;
    let body = response[separator + 4..].to_vec();

    if !(200..300).contains(&status) {
        return Err(LiveBackendError::HttpStatus {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    }

    Ok(body)
}

#[cfg(test)]
mod tests {
    use quip_contracts::{ModelVariant, PredictionRequest};

    use super::{
        is_implausibly_truncated, is_model_scaffolding, normalize_model_outputs, LiveBackend,
        SYSTEM_PROMPT,
    };

    fn request() -> PredictionRequest {
        PredictionRequest {
            request_id: "live-test".to_owned(),
            profile_id: "profile_default".to_owned(),
            model_variant: ModelVariant::Base,
            draft: "cnt cm tmr".to_owned(),
            context_snippets: vec![],
            personal_patterns: vec![],
        }
    }

    #[test]
    fn rejects_non_loopback_style_addresses_that_do_not_parse() {
        assert!(LiveBackend::new("not-an-address").is_err());
        assert!(LiveBackend::new("0.0.0.0:1234").is_err());
    }

    #[test]
    fn prompt_contains_policy_without_answer_shaped_text() {
        assert!(SYSTEM_PROMPT.contains("actual complete text"));
        assert!(SYSTEM_PROMPT.contains("If no confident correction is needed"));
        assert!(!SYSTEM_PROMPT.contains("Suggestion text:"));
        assert!(!SYSTEM_PROMPT
            .to_ascii_lowercase()
            .contains("best full text"));
    }

    #[test]
    fn recognizes_model_scaffolding_without_rejecting_normal_text() {
        for leaked in [
            "best full text",
            "best full text \"full text\"",
            "text",
            "text_1,",
            "suggestion",
            "} tomorrow",
            "Correct confident keyboard typos, phonetic spellings, and common compressed phrases.",
            "I'm Quip, a conservative English text corrector. I can help with that.",
            "uggestionuggestion:see you tomorrow",
        ] {
            assert!(is_model_scaffolding(leaked), "{leaked}");
        }
        assert!(!is_model_scaffolding("this is a sentence"));
    }

    #[test]
    fn rejects_truncated_fragments_but_keeps_corrections_and_expansions() {
        assert!(is_implausibly_truncated(
            "went to the store instaed",
            "the text"
        ));
        assert!(!is_implausibly_truncated(
            "went to the store instaed",
            "went to the store instead"
        ));
        assert!(!is_implausibly_truncated(
            "cnt cm tmrw",
            "can't come tomorrow"
        ));
        assert!(is_implausibly_truncated("this sa sntece", "This sentence"));
    }

    #[test]
    fn ranks_deduplicated_suggestions_by_votes_then_earliest_completion() {
        let result = normalize_model_outputs(
            &request(),
            vec![
                "candidate b result".to_owned(),
                "candidate a result".to_owned(),
                "candidate b result".to_owned(),
                "candidate c result".to_owned(),
                "candidate a result".to_owned(),
            ],
            12,
        );

        assert!(matches!(
            result,
            quip_contracts::PredictionResult::Ok {
                candidates,
                ..
            } if candidates == vec![
                "candidate b result",
                "candidate a result",
                "candidate c result",
            ]
        ));
    }

    #[test]
    fn exact_draft_suggestion_becomes_zero_candidates() {
        let result = normalize_model_outputs(
            &request(),
            (0..5).map(|_| "cnt cm tmr".to_owned()).collect(),
            12,
        );

        assert!(matches!(
            result,
            quip_contracts::PredictionResult::Ok {
                candidates,
                ..
            } if candidates.is_empty()
        ));
    }

    #[test]
    fn prompt_placeholder_leakage_is_never_a_candidate() {
        let result = normalize_model_outputs(
            &request(),
            vec![
                "best full text".to_owned(),
                "best full text \"full text\"".to_owned(),
                "Best Full Text".to_owned(),
                "text_1,".to_owned(),
                "} tomorrow".to_owned(),
            ],
            12,
        );

        assert!(matches!(
            result,
            quip_contracts::PredictionResult::Ok {
                candidates,
                ..
            } if candidates.is_empty()
        ));
    }

    #[test]
    fn incomplete_generation_batch_is_an_explicit_error() {
        let result = normalize_model_outputs(
            &request(),
            (0..4).map(|index| format!("candidate {index}")).collect(),
            12,
        );

        assert!(matches!(
            result,
            quip_contracts::PredictionResult::Error {
                error,
                ..
            } if error.code == "incomplete_generation_batch" && error.retryable
        ));
    }
}
