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
const DEFAULT_COMPLETION_COUNT: usize = 5;
const DEFAULT_TEMPERATURE: f64 = 0.1;
const DEFAULT_MAX_TOKENS: u64 = 64;
const COMPLETION_ATTEMPTS: usize = 3;
const SYSTEM_PROMPT: &str = include_str!("../../../../training/flash/system_prompt.txt");

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
    output: String,
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
        let mut result =
            normalize_model_outputs(request, model_outputs, self.config.completion_count, 0);
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

    /// An unconstrained small model occasionally emits a whitespace-only
    /// reply; retry those instead of failing the whole completion batch.
    fn complete_base(
        &self,
        request: &PredictionRequest,
    ) -> Result<TimedModelOutput, LiveBackendError> {
        let mut last_error = None;
        for _ in 0..COMPLETION_ATTEMPTS {
            match self.complete_base_once(request) {
                Ok(output) => return Ok(output),
                Err(error @ LiveBackendError::MissingModelContent) => last_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or(LiveBackendError::MissingModelContent))
    }

    fn complete_base_once(
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
            "stop": ["<|endoftext|>"]
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
        let output = content.trim().to_owned();
        if output.is_empty() {
            return Err(LiveBackendError::MissingModelContent);
        }
        let tokens = response_body.usage.map(|usage| {
            token_profile(
                content,
                &output,
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
    outputs: Vec<String>,
    expected_output_count: usize,
    latency_ms: u64,
) -> PredictionResult {
    if outputs.len() != expected_output_count {
        return prediction_error(
            request,
            "incomplete_generation_batch",
            "live inference requires a complete configured generation batch",
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
        is_implausibly_truncated, is_model_scaffolding, normalize_model_outputs, token_profile,
        ChatUsage, LiveBackend, LiveConfig, SYSTEM_PROMPT,
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
                "candidate b result".to_owned(),
                "candidate a result".to_owned(),
                "candidate b result".to_owned(),
                "candidate c result".to_owned(),
                "candidate a result".to_owned(),
            ],
            5,
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
            5,
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
            5,
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
            5,
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
