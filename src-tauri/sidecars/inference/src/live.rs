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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::InferenceBackend;

const DEFAULT_MODEL_ADDR: &str = "127.0.0.1:1234";
const DEFAULT_MODEL_ID: &str = "default";
const DEFAULT_COMPLETION_COUNT: usize = 3;
const DEFAULT_TEMPERATURE: f64 = 0.1;
const DEFAULT_MAX_TOKENS: u64 = 64;
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
    InvalidConfig(String),
    Io(std::io::Error),
    InvalidHttpResponse(&'static str),
    HttpStatus { status: u16, body: String },
    InvalidJson(serde_json::Error),
    MissingModelContent,
    WorkerPanicked,
}

impl fmt::Display for LiveBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAddress(address) => {
                write!(formatter, "invalid local model address: {address}")
            }
            Self::InvalidConfig(reason) => write!(formatter, "invalid live model config: {reason}"),
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
            Self::WorkerPanicked => write!(formatter, "local model completion worker panicked"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveConfig {
    pub model_id: String,
    pub completion_count: usize,
    pub temperature: f64,
    pub max_tokens: u64,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            model_id: DEFAULT_MODEL_ID.to_owned(),
            completion_count: DEFAULT_COMPLETION_COUNT,
            temperature: DEFAULT_TEMPERATURE,
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpTiming {
    pub connect_us: u64,
    pub request_write_us: u64,
    pub time_to_first_byte_us: u64,
    pub response_read_us: u64,
    pub http_parse_us: u64,
    pub total_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionTiming {
    pub request_build_us: u64,
    pub http: HttpTiming,
    pub response_decode_us: u64,
    pub output_decode_us: u64,
    pub total_us: u64,
    pub tokens: Option<TokenProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenProfile {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub output_chars: usize,
    pub suggestion_chars: usize,
    pub deterministic_schema_chars: usize,
    pub estimated_suggestion_tokens: f64,
    pub estimated_schema_tokens: f64,
    pub server_ms_per_output_token: f64,
    pub model_reported_total_ms: Option<f64>,
    pub prompt_prefill_ms: Option<f64>,
    pub completion_decode_ms: Option<f64>,
    pub prompt_ms_per_token: Option<f64>,
    pub completion_ms_per_token: Option<f64>,
    pub server_queue_overhead_ms_estimate: Option<f64>,
    pub estimated_schema_latency_ms: f64,
    pub finish_reason: Option<String>,
    pub max_tokens_reached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineTiming {
    pub completion_batch_us: u64,
    pub normalization_us: u64,
    pub backend_total_us: u64,
    pub completions: Vec<CompletionTiming>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveBenchmark {
    pub config: LiveConfig,
    pub result: PredictionResult,
    pub timing: PipelineTiming,
}

struct TimedHttpResponse {
    body: Vec<u8>,
    timing: HttpTiming,
}

struct TimedModelOutput {
    output: ModelOutput,
    timing: CompletionTiming,
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
    config: LiveConfig,
}

impl LiveBackend {
    pub fn local_default() -> Result<Self, LiveBackendError> {
        let address =
            std::env::var("QUIP_MODEL_ADDR").unwrap_or_else(|_| DEFAULT_MODEL_ADDR.to_owned());
        let mut config = LiveConfig::default();
        if let Ok(model_id) = std::env::var("QUIP_MODEL_ID") {
            config.model_id = model_id;
        }
        if let Ok(value) = std::env::var("QUIP_COMPLETION_COUNT") {
            config.completion_count = value.parse().map_err(|_| {
                LiveBackendError::InvalidConfig(
                    "QUIP_COMPLETION_COUNT must be an integer".to_owned(),
                )
            })?;
        }
        if let Ok(value) = std::env::var("QUIP_TEMPERATURE") {
            config.temperature = value.parse().map_err(|_| {
                LiveBackendError::InvalidConfig("QUIP_TEMPERATURE must be a number".to_owned())
            })?;
        }
        if let Ok(value) = std::env::var("QUIP_MAX_TOKENS") {
            config.max_tokens = value.parse().map_err(|_| {
                LiveBackendError::InvalidConfig("QUIP_MAX_TOKENS must be an integer".to_owned())
            })?;
        }
        Self::with_config(&address, config)
    }

    pub fn new(address: &str) -> Result<Self, LiveBackendError> {
        Self::with_config(address, LiveConfig::default())
    }

    pub fn with_config(address: &str, config: LiveConfig) -> Result<Self, LiveBackendError> {
        let address: SocketAddr = address
            .parse()
            .map_err(|_| LiveBackendError::InvalidAddress(address.to_owned()))?;
        if !address.ip().is_loopback() {
            return Err(LiveBackendError::InvalidAddress(address.to_string()));
        }
        validate_config(&config)?;
        Ok(Self {
            address,
            timeout: Duration::from_secs(90),
            config,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn config(&self) -> &LiveConfig {
        &self.config
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<Vec<u8>, LiveBackendError> {
        self.request_timed(method, path, body)
            .map(|response| response.body)
    }

    fn request_timed(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<TimedHttpResponse, LiveBackendError> {
        let total_started = Instant::now();
        let connect_started = Instant::now();
        let mut stream = TcpStream::connect_timeout(&self.address, Duration::from_secs(2))?;
        let connect_us = elapsed_us(connect_started);
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;

        let body = body.unwrap_or_default();
        let headers = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.address,
            body.len()
        );
        let write_started = Instant::now();
        stream.write_all(headers.as_bytes())?;
        stream.write_all(body)?;
        stream.flush()?;
        let request_write_us = elapsed_us(write_started);

        let mut response = Vec::new();
        let first_byte_started = Instant::now();
        let mut first_chunk = [0_u8; 8192];
        let first_read = stream.read(&mut first_chunk)?;
        let time_to_first_byte_us = elapsed_us(first_byte_started);
        response.extend_from_slice(&first_chunk[..first_read]);
        let read_started = Instant::now();
        stream.read_to_end(&mut response)?;
        let response_read_us = elapsed_us(read_started);
        let parse_started = Instant::now();
        let body = parse_http_response(&response)?;
        let http_parse_us = elapsed_us(parse_started);

        Ok(TimedHttpResponse {
            body,
            timing: HttpTiming {
                connect_us,
                request_write_us,
                time_to_first_byte_us,
                response_read_us,
                http_parse_us,
                total_us: elapsed_us(total_started),
            },
        })
    }

    fn predict_base(
        &self,
        request: &PredictionRequest,
    ) -> Result<PredictionResult, LiveBackendError> {
        self.benchmark_prediction(request)
            .map(|benchmark| benchmark.result)
    }

    pub fn benchmark_prediction(
        &self,
        request: &PredictionRequest,
    ) -> Result<LiveBenchmark, LiveBackendError> {
        if request.model_variant != ModelVariant::Base {
            return Err(LiveBackendError::InvalidConfig(
                "latency benchmarking currently supports the base model variant".to_owned(),
            ));
        }

        let backend_started = Instant::now();
        let batch_started = Instant::now();
        let timed_outputs = std::thread::scope(|scope| {
            let workers = (0..self.config.completion_count)
                .map(|_| scope.spawn(|| self.complete_base(request)))
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .map(|worker| match worker.join() {
                    Ok(result) => result,
                    Err(_) => Err(LiveBackendError::WorkerPanicked),
                })
                .collect::<Result<Vec<_>, _>>()
        })?;
        let completion_batch_us = elapsed_us(batch_started);

        let (model_outputs, completion_timings): (Vec<_>, Vec<_>) = timed_outputs
            .into_iter()
            .map(|timed| (timed.output, timed.timing))
            .unzip();
        let normalization_started = Instant::now();
        let mut result = normalize_model_outputs(request, model_outputs, 0);
        let normalization_us = elapsed_us(normalization_started);
        let backend_total_us = elapsed_us(backend_started);
        if let PredictionResult::Ok { latency_ms, .. } = &mut result {
            *latency_ms = (backend_total_us + 999) / 1000;
        }

        Ok(LiveBenchmark {
            config: self.config.clone(),
            result,
            timing: PipelineTiming {
                completion_batch_us,
                normalization_us,
                backend_total_us,
                completions: completion_timings,
            },
        })
    }

    fn complete_base(
        &self,
        request: &PredictionRequest,
    ) -> Result<TimedModelOutput, LiveBackendError> {
        let total_started = Instant::now();
        let build_started = Instant::now();
        let user_content = model_input(request).to_string();
        let body = json!({
            "model": &self.config.model_id,
            "messages": [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": user_content}
            ],
            "enable_thinking": false,
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens,
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
        let request_build_us = elapsed_us(build_started);
        let response = self.request_timed("POST", "/v1/chat/completions", Some(&body))?;
        let response_decode_started = Instant::now();
        let response_body: ChatCompletion = serde_json::from_slice(&response.body)?;
        let response_decode_us = elapsed_us(response_decode_started);
        let output_decode_started = Instant::now();
        let choice = response_body
            .choices
            .into_iter()
            .next()
            .ok_or(LiveBackendError::MissingModelContent)?;
        let content = choice
            .message
            .content
            .as_deref()
            .ok_or(LiveBackendError::MissingModelContent)?;
        let output: ModelOutput = serde_json::from_str(content)?;
        let tokens = response_body.usage.map(|usage| {
            token_profile(
                content,
                &output.suggestion,
                usage,
                choice.finish_reason,
                response.timing.time_to_first_byte_us,
                self.config.max_tokens,
            )
        });
        let output_decode_us = elapsed_us(output_decode_started);

        Ok(TimedModelOutput {
            output,
            timing: CompletionTiming {
                request_build_us,
                http: response.timing,
                response_decode_us,
                output_decode_us,
                total_us: elapsed_us(total_started),
                tokens,
            },
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

    fn benchmark(&self, request: &PredictionRequest) -> Result<LiveBenchmark, String> {
        self.benchmark_prediction(request)
            .map_err(|error| error.to_string())
    }
}

fn validate_config(config: &LiveConfig) -> Result<(), LiveBackendError> {
    if config.model_id.trim().is_empty() {
        return Err(LiveBackendError::InvalidConfig(
            "model_id must not be empty".to_owned(),
        ));
    }
    if !(1..=5).contains(&config.completion_count) {
        return Err(LiveBackendError::InvalidConfig(
            "completion_count must be between 1 and 5".to_owned(),
        ));
    }
    if !config.temperature.is_finite() || !(0.0..=2.0).contains(&config.temperature) {
        return Err(LiveBackendError::InvalidConfig(
            "temperature must be finite and between 0 and 2".to_owned(),
        ));
    }
    if !(1..=512).contains(&config.max_tokens) {
        return Err(LiveBackendError::InvalidConfig(
            "max_tokens must be between 1 and 512".to_owned(),
        ));
    }
    Ok(())
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u64::MAX as u128) as u64
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
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    #[serde(default)]
    avg_prompt_tok_per_sec: Option<f64>,
    #[serde(default)]
    avg_compl_tok_per_sec: Option<f64>,
    #[serde(default)]
    total_time_sec: Option<f64>,
    #[serde(default)]
    total_prompt_time_sec: Option<f64>,
    #[serde(default)]
    total_completion_time_sec: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelOutput {
    suggestion: String,
}

fn token_profile(
    content: &str,
    suggestion: &str,
    usage: ChatUsage,
    finish_reason: Option<String>,
    model_server_us: u64,
    max_tokens: u64,
) -> TokenProfile {
    let output_chars = content.chars().count();
    let suggestion_chars = suggestion.chars().count().min(output_chars);
    let deterministic_schema_chars = output_chars.saturating_sub(suggestion_chars);
    let suggestion_share = if output_chars == 0 {
        0.0
    } else {
        suggestion_chars as f64 / output_chars as f64
    };
    let estimated_suggestion_tokens = usage.completion_tokens as f64 * suggestion_share;
    let estimated_schema_tokens = usage.completion_tokens as f64 - estimated_suggestion_tokens;
    let server_ms_per_output_token = if usage.completion_tokens == 0 {
        0.0
    } else {
        model_server_us as f64 / 1000.0 / usage.completion_tokens as f64
    };
    let model_reported_total_ms = usage.total_time_sec.map(|seconds| seconds * 1000.0);
    let prompt_prefill_ms = usage.total_prompt_time_sec.map(|seconds| seconds * 1000.0);
    let completion_decode_ms = usage
        .total_completion_time_sec
        .map(|seconds| seconds * 1000.0);
    let prompt_ms_per_token = usage
        .avg_prompt_tok_per_sec
        .filter(|rate| *rate > 0.0)
        .map(|rate| 1000.0 / rate)
        .or_else(|| {
            prompt_prefill_ms
                .filter(|_| usage.prompt_tokens > 0)
                .map(|ms| ms / usage.prompt_tokens as f64)
        });
    let completion_ms_per_token = usage
        .avg_compl_tok_per_sec
        .filter(|rate| *rate > 0.0)
        .map(|rate| 1000.0 / rate)
        .or_else(|| {
            completion_decode_ms
                .filter(|_| usage.completion_tokens > 0)
                .map(|ms| ms / usage.completion_tokens as f64)
        });
    let server_queue_overhead_ms_estimate = model_reported_total_ms
        .map(|model_ms| (model_server_us as f64 / 1000.0 - model_ms).max(0.0));
    let output_token_ms = completion_ms_per_token.unwrap_or(server_ms_per_output_token);
    let estimated_schema_latency_ms = output_token_ms * estimated_schema_tokens;
    let max_tokens_reached =
        finish_reason.as_deref() == Some("length") || usage.completion_tokens >= max_tokens;

    TokenProfile {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
        output_chars,
        suggestion_chars,
        deterministic_schema_chars,
        estimated_suggestion_tokens,
        estimated_schema_tokens,
        server_ms_per_output_token,
        model_reported_total_ms,
        prompt_prefill_ms,
        completion_decode_ms,
        prompt_ms_per_token,
        completion_ms_per_token,
        server_queue_overhead_ms_estimate,
        estimated_schema_latency_ms,
        finish_reason,
        max_tokens_reached,
    }
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

    use super::{
        normalize_model_outputs, prediction_schema, token_profile, ChatUsage, LiveBackend,
        LiveConfig, ModelOutput,
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
    fn rejects_out_of_range_benchmark_controls() {
        let invalid_counts = LiveConfig {
            completion_count: 0,
            ..LiveConfig::default()
        };
        assert!(LiveBackend::with_config("127.0.0.1:1234", invalid_counts).is_err());

        let invalid_temperature = LiveConfig {
            temperature: f64::NAN,
            ..LiveConfig::default()
        };
        assert!(LiveBackend::with_config("127.0.0.1:1234", invalid_temperature).is_err());
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
    fn token_profile_exposes_deterministic_schema_tax_without_output_text() {
        let profile = token_profile(
            r#"{"suggestion":"can't meet tomorrow"}"#,
            "can't meet tomorrow",
            ChatUsage {
                prompt_tokens: 80,
                completion_tokens: 10,
                total_tokens: 90,
                avg_prompt_tok_per_sec: Some(320.0),
                avg_compl_tok_per_sec: Some(40.0),
                total_time_sec: Some(0.5),
                total_prompt_time_sec: Some(0.25),
                total_completion_time_sec: Some(0.25),
            },
            Some("stop".to_owned()),
            500_000,
            32,
        );

        assert_eq!(profile.prompt_tokens, 80);
        assert_eq!(profile.completion_tokens, 10);
        assert!(profile.deterministic_schema_chars > 0);
        assert!(profile.estimated_schema_tokens > 0.0);
        assert_eq!(profile.server_ms_per_output_token, 50.0);
        assert_eq!(profile.completion_ms_per_token, Some(25.0));
        assert_eq!(profile.prompt_prefill_ms, Some(250.0));
        assert!(!profile.max_tokens_reached);
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
