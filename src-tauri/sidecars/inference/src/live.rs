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
const COMPLETION_COUNT: usize = 3;
const SYSTEM_PROMPT: &str = r#"You are an English text corrector.

Given a JSON object containing text, predict one full-text suggestion.

Return exactly one JSON object with no commentary:
{"suggestion":"best full text"}

Rules:
- Return one conservative correction of a confident typo, phonetic spelling, or compressed phrase.
- Make the smallest useful change. Do not add facts or change tone.
"#;

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
        let model_outputs = std::thread::scope(|scope| {
            let workers = (0..COMPLETION_COUNT)
                .map(|_| scope.spawn(|| self.complete_base(request)))
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .map(|worker| worker.join().expect("model completion worker panicked"))
                .collect::<Result<Vec<_>, LiveBackendError>>()
        })?;

        Ok(normalize_model_outputs(
            request,
            model_outputs,
            started.elapsed().as_millis() as u64,
        ))
    }

    fn complete_base(&self, request: &PredictionRequest) -> Result<ModelOutput, LiveBackendError> {
        let user_content = model_input(request).to_string();
        let body = json!({
            "model": "default",
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": user_content}
            ],
            "enable_thinking": false,
            "temperature": 0.1,
            "max_tokens": 64,
            "stop": ["<|endoftext|>"],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "quip_prediction",
                    "schema": prediction_schema()
                }
            }
        });
        let body = serde_json::to_vec(&body)?;
        let response = self.request("POST", "/v1/chat/completions", Some(&body))?;
        let response: ChatCompletion = serde_json::from_slice(&response)?;
        response
            .choices
            .into_iter()
            .next()
            .ok_or(LiveBackendError::MissingModelContent)
            .and_then(|choice| {
                let content = choice
                    .message
                    .content
                    .as_deref()
                    .ok_or(LiveBackendError::MissingModelContent)?;
                serde_json::from_str(content).map_err(LiveBackendError::from)
            })
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

fn prediction_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "suggestion": {
                "type": "string",
                "minLength": 1
            }
        },
        "required": ["suggestion"],
        "additionalProperties": false
    })
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
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelOutput {
    suggestion: String,
}

fn normalize_model_outputs(
    request: &PredictionRequest,
    outputs: Vec<ModelOutput>,
    latency_ms: u64,
) -> PredictionResult {
    let suggestions = outputs
        .into_iter()
        .map(|output| output.suggestion.trim().to_owned())
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
        if suggestion == request.draft {
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
    use serde_json::json;

    use super::{normalize_model_outputs, prediction_schema, LiveBackend, ModelOutput};

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
    fn model_schema_matches_the_freesolo_training_contract() {
        assert_eq!(
            prediction_schema(),
            json!({
                "type": "object",
                "properties": {"suggestion": {"type": "string", "minLength": 1}},
                "required": ["suggestion"],
                "additionalProperties": false,
            })
        );
    }

    #[test]
    fn ranks_deduplicated_suggestions_by_votes_then_earliest_completion() {
        let result = normalize_model_outputs(
            &request(),
            vec![
                ModelOutput {
                    suggestion: "candidate b".to_owned(),
                },
                ModelOutput {
                    suggestion: "candidate a".to_owned(),
                },
                ModelOutput {
                    suggestion: "candidate b".to_owned(),
                },
                ModelOutput {
                    suggestion: "candidate c".to_owned(),
                },
                ModelOutput {
                    suggestion: "candidate a".to_owned(),
                },
                ModelOutput {
                    suggestion: "candidate d".to_owned(),
                },
                ModelOutput {
                    suggestion: "candidate e".to_owned(),
                },
                ModelOutput {
                    suggestion: "candidate f".to_owned(),
                },
                ModelOutput {
                    suggestion: "cnt cm tmr".to_owned(),
                },
            ],
            12,
        );

        assert!(matches!(
            result,
            quip_contracts::PredictionResult::Ok {
                candidates,
                ..
            } if candidates == vec![
                "candidate b",
                "candidate a",
                "candidate c",
                "candidate d",
                "candidate e",
            ]
        ));
    }

    #[test]
    fn exact_draft_suggestion_becomes_zero_candidates() {
        let result = normalize_model_outputs(
            &request(),
            vec![ModelOutput {
                suggestion: "cnt cm tmr".to_owned(),
            }],
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
}
