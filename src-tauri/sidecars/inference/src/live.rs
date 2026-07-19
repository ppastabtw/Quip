use std::{
    collections::HashMap,
    error::Error,
    fmt,
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
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
const DEFAULT_GLOBAL_MODEL_ID: &str = "mlx-community/Qwen3.5-2B-MLX-4bit";
const COMPLETION_ATTEMPTS: usize = 3;
const DEFAULT_MODEL_ID: &str = "default";
const DEFAULT_COMPLETION_COUNT: usize = 5;
const DEFAULT_TEMPERATURE: f64 = 0.1;
const DEFAULT_MAX_TOKENS: u64 = 64;
const SYSTEM_PROMPT: &str = include_str!("../../../../training/flash/system_prompt.txt");
const SYSTEM_PROMPT_V0_JSON: &str =
    include_str!("../../../../training/flash/system_prompt_v0_json.txt");
const COMPLETION_SEEDS: [u64; DEFAULT_COMPLETION_COUNT] = [101, 202, 303, 404, 505];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputContract {
    PlainText,
    JsonSuggestion,
}

impl OutputContract {
    fn parse(value: &str, variable: &str) -> Result<Self, LiveBackendError> {
        match value {
            "plain_text" => Ok(Self::PlainText),
            "json_suggestion" => Ok(Self::JsonSuggestion),
            _ => Err(LiveBackendError::InvalidConfig(format!(
                "{variable} must be plain_text or json_suggestion"
            ))),
        }
    }

    fn system_prompt(self) -> &'static str {
        match self {
            Self::PlainText => SYSTEM_PROMPT,
            Self::JsonSuggestion => SYSTEM_PROMPT_V0_JSON,
        }
    }
}

#[derive(Debug)]
pub enum LiveBackendError {
    InvalidAddress(String),
    InvalidConfig(String),
    Io(std::io::Error),
    InvalidHttpResponse(&'static str),
    HttpStatus {
        status: u16,
        body: String,
    },
    InvalidJson(serde_json::Error),
    MissingModelContent,
    WorkerPanicked,
    /// The coordinator shut this completion's connection down deliberately
    /// (early-exit agreement, or a superseded sliding-window burst). Not a
    /// failure: `normalize_model_outputs` excludes it from the batch-size
    /// check that guards against genuine transport/parse errors.
    Cancelled,
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
            Self::Cancelled => write!(formatter, "completion was cancelled"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveConfig {
    pub model_id: String,
    pub completion_count: usize,
    pub temperature: f64,
    pub max_tokens: u64,
    /// Streams each completion instead of waiting for the full buffered
    /// response, so the coordinator can see partial agreement and cancel the
    /// rest of the batch early. Defaults off: the batched path is the one
    /// validated against a live server so far, and this is the A/B toggle
    /// for benchmarking streaming against it (see `run-latency-benchmark.sh`).
    #[serde(default)]
    pub streaming: bool,
    /// Early-exit threshold: once this many streamed completions agree on
    /// the same suggestion, the rest of the batch is cancelled. Only takes
    /// effect when `streaming` is true.
    #[serde(default = "default_early_exit_agreement")]
    pub early_exit_agreement: usize,
}

fn default_early_exit_agreement() -> usize {
    3
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            model_id: DEFAULT_MODEL_ID.to_owned(),
            completion_count: DEFAULT_COMPLETION_COUNT,
            temperature: DEFAULT_TEMPERATURE,
            max_tokens: DEFAULT_MAX_TOKENS,
            streaming: false,
            early_exit_agreement: default_early_exit_agreement(),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_server: Option<ModelServerTiming>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelServerTiming {
    pub system_cache_hit: bool,
    pub context_cache_hit: bool,
    #[serde(default)]
    pub schema_token_elision: bool,
    pub prepare_us: u64,
    pub prefill_us: u64,
    pub system_prefill_us: u64,
    pub context_prefill_us: u64,
    pub draft_prefill_us: u64,
    #[serde(default)]
    pub fixed_prefix_prefill_us: u64,
    pub batching_us: u64,
    pub decode_us: u64,
    #[serde(default)]
    pub dynamic_decode_us: u64,
    #[serde(default)]
    pub generated_tokens: u64,
    #[serde(default)]
    pub avoided_schema_tokens: u64,
    pub postprocess_us: u64,
    pub overhead_us: u64,
    pub total_us: u64,
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

#[derive(Debug)]
struct TimedModelOutput {
    output: String,
    timing: CompletionTiming,
}

#[derive(Debug)]
struct TimedBatchOutputs {
    outputs: Vec<String>,
    timing: CompletionTiming,
    model_server: ModelServerTiming,
}

/// Lets the coordinator thread abort one streaming completion's socket from
/// outside the worker thread that owns it. `try_clone` on a `TcpStream`
/// duplicates the file descriptor for the same underlying socket, so
/// `shutdown` here unblocks a concurrent blocking `read` on the worker's
/// half — the standard way to cancel a blocking std socket read in Rust.
#[derive(Default)]
struct CancelHandle {
    stream: Mutex<Option<TcpStream>>,
    cancelled: AtomicBool,
}

impl CancelHandle {
    /// Never itself panics on a poisoned lock: `cancel()` runs from the
    /// coordinator thread and must still be able to reach in and shut a
    /// worker's socket down even if something else already went wrong.
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<TcpStream>> {
        self.stream
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn arm(&self, stream: &TcpStream) -> std::io::Result<()> {
        *self.lock() = Some(stream.try_clone()?);
        Ok(())
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(stream) = self.lock().take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// One decoded Server-Sent-Events chunk from an OpenAI-compatible streaming
/// `/v1/chat/completions` response.
#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    choices: Vec<ChatChunkChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChunkChoice {
    delta: ChatChunkDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatChunkDelta {
    content: Option<String>,
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

fn retryable_completion_error(error: &LiveBackendError) -> bool {
    match error {
        // Local generation servers can occasionally close a stream between
        // JSON/SSE fragments. A fresh completion is safe: no user-visible
        // state has been committed yet, and the request is idempotent.
        LiveBackendError::Io(_)
        | LiveBackendError::InvalidHttpResponse(_)
        | LiveBackendError::InvalidJson(_)
        | LiveBackendError::MissingModelContent => true,
        LiveBackendError::HttpStatus { status, .. } => {
            *status == 408 || *status == 429 || (500..600).contains(status)
        }
        LiveBackendError::InvalidAddress(_)
        | LiveBackendError::InvalidConfig(_)
        | LiveBackendError::WorkerPanicked
        | LiveBackendError::Cancelled => false,
    }
}

#[derive(Debug, Clone)]
pub struct LiveBackend {
    address: SocketAddr,
    timeout: Duration,
    config: LiveConfig,
    output_contract: OutputContract,
    global_endpoint: Option<ModelEndpoint>,
    cache_fork: bool,
    schema_token_elision: bool,
}

#[derive(Debug, Clone)]
struct ModelEndpoint {
    address: SocketAddr,
    model_id: String,
    output_contract: OutputContract,
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
        if let Ok(value) = std::env::var("QUIP_STREAMING") {
            config.streaming = value.parse().map_err(|_| {
                LiveBackendError::InvalidConfig("QUIP_STREAMING must be true or false".to_owned())
            })?;
        }
        if let Ok(value) = std::env::var("QUIP_EARLY_EXIT_AGREEMENT") {
            config.early_exit_agreement = value.parse().map_err(|_| {
                LiveBackendError::InvalidConfig(
                    "QUIP_EARLY_EXIT_AGREEMENT must be an integer".to_owned(),
                )
            })?;
        }
        let mut backend = Self::with_config(&address, config)?;
        if let Ok(value) = std::env::var("QUIP_CACHE_FORK") {
            backend.cache_fork = value.parse().map_err(|_| {
                LiveBackendError::InvalidConfig("QUIP_CACHE_FORK must be true or false".to_owned())
            })?;
        }
        if let Ok(value) = std::env::var("QUIP_SCHEMA_TOKEN_ELISION") {
            backend.schema_token_elision = value.parse().map_err(|_| {
                LiveBackendError::InvalidConfig(
                    "QUIP_SCHEMA_TOKEN_ELISION must be true or false".to_owned(),
                )
            })?;
        }
        if let Ok(value) = std::env::var("QUIP_MODEL_OUTPUT_CONTRACT") {
            backend.output_contract = OutputContract::parse(&value, "QUIP_MODEL_OUTPUT_CONTRACT")?;
        }
        if let Ok(global_address) = std::env::var("QUIP_GLOBAL_MODEL_ADDR") {
            let global_model_id = std::env::var("QUIP_GLOBAL_MODEL_ID")
                .unwrap_or_else(|_| DEFAULT_GLOBAL_MODEL_ID.to_owned());
            let output_contract = std::env::var("QUIP_GLOBAL_OUTPUT_CONTRACT")
                .ok()
                .map(|value| OutputContract::parse(&value, "QUIP_GLOBAL_OUTPUT_CONTRACT"))
                .transpose()?
                .unwrap_or(OutputContract::PlainText);
            backend = backend.with_global_endpoint_contract(
                &global_address,
                global_model_id,
                output_contract,
            )?;
        }
        Ok(backend)
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
            output_contract: OutputContract::PlainText,
            global_endpoint: None,
            cache_fork: false,
            schema_token_elision: false,
        })
    }

    pub fn with_global_endpoint(
        self,
        address: &str,
        model_id: String,
    ) -> Result<Self, LiveBackendError> {
        self.with_global_endpoint_contract(address, model_id, OutputContract::PlainText)
    }

    fn with_global_endpoint_contract(
        mut self,
        address: &str,
        model_id: String,
        output_contract: OutputContract,
    ) -> Result<Self, LiveBackendError> {
        let address: SocketAddr = address
            .parse()
            .map_err(|_| LiveBackendError::InvalidAddress(address.to_owned()))?;
        if !address.ip().is_loopback() {
            return Err(LiveBackendError::InvalidAddress(address.to_string()));
        }
        if model_id.trim().is_empty() {
            return Err(LiveBackendError::InvalidConfig(
                "global model_id must not be empty".to_owned(),
            ));
        }
        self.global_endpoint = Some(ModelEndpoint {
            address,
            model_id,
            output_contract,
        });
        Ok(self)
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn config(&self) -> &LiveConfig {
        &self.config
    }

    fn for_endpoint(&self, endpoint: &ModelEndpoint) -> Self {
        let mut backend = self.clone();
        backend.address = endpoint.address;
        backend.config.model_id = endpoint.model_id.clone();
        backend.output_contract = endpoint.output_contract;
        backend.global_endpoint = None;
        backend
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
        self.run_prediction(request)
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

        self.run_prediction(request)
    }

    fn run_prediction(
        &self,
        request: &PredictionRequest,
    ) -> Result<LiveBenchmark, LiveBackendError> {
        let backend_started = Instant::now();
        let batch_started = Instant::now();
        let (model_outputs, cancelled_count, completion_timings, model_server) = if self.cache_fork
        {
            let batch = self.complete_cache_fork_batch(request)?;
            (
                batch.outputs,
                0,
                vec![batch.timing],
                Some(batch.model_server),
            )
        } else if self.config.streaming {
            let (outputs, cancelled, timings) = self.run_streaming_batch(request)?;
            (outputs, cancelled, timings, None)
        } else {
            let timed_outputs = std::thread::scope(|scope| {
                let workers = (0..self.config.completion_count)
                    .map(|index| scope.spawn(move || self.complete_base(request, index)))
                    .collect::<Vec<_>>();
                workers
                    .into_iter()
                    .map(|worker| match worker.join() {
                        Ok(result) => result,
                        Err(_) => Err(LiveBackendError::WorkerPanicked),
                    })
                    .collect::<Result<Vec<_>, _>>()
            })?;
            let (outputs, timings): (Vec<_>, Vec<_>) = timed_outputs
                .into_iter()
                .map(|timed| (timed.output, timed.timing))
                .unzip();
            (outputs, 0, timings, None)
        };
        let completion_batch_us = elapsed_us(batch_started);

        let normalization_started = Instant::now();
        let mut result = normalize_model_outputs(
            request,
            model_outputs,
            self.config.completion_count,
            cancelled_count,
            0,
        );
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
                model_server,
            },
        })
    }

    fn complete_cache_fork_batch(
        &self,
        request: &PredictionRequest,
    ) -> Result<TimedBatchOutputs, LiveBackendError> {
        let mut last_error = None;
        for _ in 0..COMPLETION_ATTEMPTS {
            match self.complete_cache_fork_batch_once(request) {
                Ok(output) => return Ok(output),
                Err(error) if retryable_completion_error(&error) => last_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or(LiveBackendError::MissingModelContent))
    }

    fn complete_cache_fork_batch_once(
        &self,
        request: &PredictionRequest,
    ) -> Result<TimedBatchOutputs, LiveBackendError> {
        let total_started = Instant::now();
        let build_started = Instant::now();
        let user_content = model_input(request).to_string();
        let mut body = json!({
            "model": &self.config.model_id,
            "messages": [
                {"role": "system", "content": self.output_contract.system_prompt()},
                {"role": "user", "content": user_content}
            ],
            "enable_thinking": false,
            "temperature": self.config.temperature,
            "seed": completion_seed(0),
            "max_tokens": self.config.max_tokens,
            "stop": ["<|endoftext|>"],
            "quip_completion_count": self.config.completion_count,
            "quip_schema_token_elision": self.schema_token_elision
        });
        if self.output_contract == OutputContract::JsonSuggestion {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "quip_prediction",
                    "schema": prediction_schema()
                }
            });
        }
        let body = serde_json::to_vec(&body)?;
        let request_build_us = elapsed_us(build_started);
        let response = self.request_timed("POST", "/v1/quip/completions", Some(&body))?;
        let response_decode_started = Instant::now();
        let mut completion: ChatCompletion = serde_json::from_slice(&response.body)?;
        let response_decode_us = elapsed_us(response_decode_started);
        let model_server = completion
            .quip_timings
            .take()
            .ok_or(LiveBackendError::MissingModelContent)?;
        completion.choices.sort_by_key(|choice| choice.index);

        let output_decode_started = Instant::now();
        let outputs = completion
            .choices
            .into_iter()
            .map(|choice| {
                let content = choice
                    .message
                    .content
                    .as_deref()
                    .ok_or(LiveBackendError::MissingModelContent)?;
                decode_model_output(content, self.output_contract)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let output_decode_us = elapsed_us(output_decode_started);

        Ok(TimedBatchOutputs {
            outputs,
            model_server,
            timing: CompletionTiming {
                request_build_us,
                http: response.timing,
                response_decode_us,
                output_decode_us,
                total_us: elapsed_us(total_started),
                tokens: None,
            },
        })
    }

    fn complete_base(
        &self,
        request: &PredictionRequest,
        completion_index: usize,
    ) -> Result<TimedModelOutput, LiveBackendError> {
        let mut last_error = None;
        for _ in 0..COMPLETION_ATTEMPTS {
            match self.complete_base_once(request, completion_index) {
                Ok(output) => return Ok(output),
                Err(error) if retryable_completion_error(&error) => last_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or(LiveBackendError::MissingModelContent))
    }

    fn complete_base_once(
        &self,
        request: &PredictionRequest,
        completion_index: usize,
    ) -> Result<TimedModelOutput, LiveBackendError> {
        let total_started = Instant::now();
        let build_started = Instant::now();
        let user_content = model_input(request).to_string();
        let mut body = json!({
            "model": &self.config.model_id,
            "messages": [
                {"role": "system", "content": self.output_contract.system_prompt()},
                {"role": "user", "content": user_content}
            ],
            "enable_thinking": false,
            "temperature": self.config.temperature,
            "seed": completion_seed(completion_index),
            "max_tokens": self.config.max_tokens,
            "stop": ["<|endoftext|>"]
        });
        if self.output_contract == OutputContract::JsonSuggestion {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "quip_prediction",
                    "schema": prediction_schema()
                }
            });
        }
        let body = serde_json::to_vec(&body)?;
        let request_build_us = elapsed_us(build_started);
        let response = self.request_timed("POST", "/v1/chat/completions", Some(&body))?;
        let response_decode_started = Instant::now();
        let ChatCompletion {
            choices,
            usage,
            timings,
            quip_timings: _,
        } = serde_json::from_slice(&response.body)?;
        let response_decode_us = elapsed_us(response_decode_started);
        let output_decode_started = Instant::now();
        let choice = choices
            .into_iter()
            .next()
            .ok_or(LiveBackendError::MissingModelContent)?;
        let content = choice
            .message
            .content
            .as_deref()
            .ok_or(LiveBackendError::MissingModelContent)?;
        let output = decode_model_output(content, self.output_contract)?;
        let tokens = usage.map(|usage| {
            token_profile(
                content,
                &output,
                usage,
                timings,
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

    /// Streams all `completion_count` completions concurrently and cancels
    /// the rest of the batch once `early_exit_agreement` of them land on the
    /// same suggestion. A real transport/parse failure on any completion
    /// still fails the whole batch (bubbles up as `Err`); only deliberate
    /// cancellations are folded into `cancelled_count` and treated as
    /// expected by `normalize_model_outputs`.
    fn run_streaming_batch(
        &self,
        request: &PredictionRequest,
    ) -> Result<(Vec<String>, usize, Vec<CompletionTiming>), LiveBackendError> {
        let handles: Vec<Arc<CancelHandle>> = (0..self.config.completion_count)
            .map(|_| Arc::new(CancelHandle::default()))
            .collect();
        let (tx, rx) = mpsc::channel::<(usize, Result<TimedModelOutput, LiveBackendError>)>();

        let slots = std::thread::scope(|scope| {
            for (index, handle) in handles.iter().enumerate() {
                let tx = tx.clone();
                let handle = Arc::clone(handle);
                scope.spawn(move || {
                    // Caught, not propagated: the coordinator loop below
                    // only terminates once every index has reported back
                    // through `tx`. An uncaught panic here would starve that
                    // loop of its final message and hang the batch forever,
                    // the same failure mode `.join()` already guards against
                    // on the non-streaming path.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        self.complete_base_streaming_indexed(request, &handle, index)
                    }))
                    .unwrap_or(Err(LiveBackendError::WorkerPanicked));
                    let _ = tx.send((index, result));
                });
            }
            drop(tx);

            let mut slots: Vec<Option<Result<TimedModelOutput, LiveBackendError>>> =
                (0..self.config.completion_count).map(|_| None).collect();
            let mut votes: HashMap<String, usize> = HashMap::new();
            let mut settled = 0;
            for (index, result) in rx {
                if let Ok(timed) = &result {
                    let suggestion = timed.output.trim().to_owned();
                    if !suggestion.is_empty() && suggestion != request.draft {
                        let count = votes.entry(suggestion).or_insert(0);
                        *count += 1;
                        if *count >= self.config.early_exit_agreement {
                            // Agreement reached: cancel every completion that
                            // hasn't reported back yet.
                            for (other, other_handle) in handles.iter().enumerate() {
                                if other != index && slots[other].is_none() {
                                    other_handle.cancel();
                                }
                            }
                        }
                    }
                }
                slots[index] = Some(result);
                settled += 1;
                if settled == self.config.completion_count {
                    break;
                }
            }
            slots
        });

        let mut outputs = Vec::new();
        let mut timings = Vec::new();
        let mut cancelled_count = 0;
        for slot in slots {
            match slot.expect("every worker reports before the batch loop exits") {
                Ok(timed) => {
                    outputs.push(timed.output);
                    timings.push(timed.timing);
                }
                Err(LiveBackendError::Cancelled) => cancelled_count += 1,
                Err(error) => return Err(error),
            }
        }
        Ok((outputs, cancelled_count, timings))
    }

    #[cfg(test)]
    fn complete_base_streaming(
        &self,
        request: &PredictionRequest,
        cancel: &CancelHandle,
    ) -> Result<TimedModelOutput, LiveBackendError> {
        self.complete_base_streaming_indexed(request, cancel, 0)
    }

    fn complete_base_streaming_indexed(
        &self,
        request: &PredictionRequest,
        cancel: &CancelHandle,
        completion_index: usize,
    ) -> Result<TimedModelOutput, LiveBackendError> {
        let mut last_error = None;
        for _ in 0..COMPLETION_ATTEMPTS {
            if cancel.is_cancelled() {
                return Err(LiveBackendError::Cancelled);
            }
            match self.complete_base_streaming_once(request, cancel, completion_index) {
                Ok(output) => return Ok(output),
                Err(error) if retryable_completion_error(&error) => last_error = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or(LiveBackendError::MissingModelContent))
    }

    fn complete_base_streaming_once(
        &self,
        request: &PredictionRequest,
        cancel: &CancelHandle,
        completion_index: usize,
    ) -> Result<TimedModelOutput, LiveBackendError> {
        let total_started = Instant::now();
        let build_started = Instant::now();
        let user_content = model_input(request).to_string();
        let mut body = json!({
            "model": &self.config.model_id,
            "messages": [
                {"role": "system", "content": self.output_contract.system_prompt()},
                {"role": "user", "content": user_content}
            ],
            "enable_thinking": false,
            "temperature": self.config.temperature,
            "seed": completion_seed(completion_index),
            "max_tokens": self.config.max_tokens,
            "stop": ["<|endoftext|>"],
            "stream": true
        });
        if self.output_contract == OutputContract::JsonSuggestion {
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "quip_prediction",
                    "schema": prediction_schema()
                }
            });
        }
        let body = serde_json::to_vec(&body)?;
        let request_build_us = elapsed_us(build_started);
        let (content, http_timing) =
            self.request_streaming("POST", "/v1/chat/completions", &body, cancel)?;
        let response_decode_started = Instant::now();
        let output = decode_model_output(&content, self.output_contract)?;
        let response_decode_us = elapsed_us(response_decode_started);

        Ok(TimedModelOutput {
            output,
            timing: CompletionTiming {
                request_build_us,
                http: http_timing,
                response_decode_us,
                output_decode_us: 0,
                total_us: elapsed_us(total_started),
                // Streaming doesn't request `usage`; token accounting stays
                // a non-streaming-only measurement for now.
                tokens: None,
            },
        })
    }

    /// Connects, sends one SSE request, and incrementally decodes the
    /// response — either `Transfer-Encoding: chunked` or a bare
    /// `Connection: close` / `Content-Length` body, whichever the server
    /// uses — accumulating the streamed `delta.content` fragments into the
    /// final text. Registers the connection into `cancel` right after
    /// connecting so the coordinator can abort a blocked read from another
    /// thread.
    fn request_streaming(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        cancel: &CancelHandle,
    ) -> Result<(String, HttpTiming), LiveBackendError> {
        let total_started = Instant::now();
        let connect_started = Instant::now();
        let mut stream = TcpStream::connect_timeout(&self.address, Duration::from_secs(2))?;
        let connect_us = elapsed_us(connect_started);
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        cancel.arm(&stream)?;
        if cancel.is_cancelled() {
            // Superseded before the connection even finished dialing.
            return Err(LiveBackendError::Cancelled);
        }

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

        let mut chunk = [0_u8; 8192];
        let mut raw = Vec::new();
        let first_byte_started = Instant::now();
        let head_end = loop {
            let read = self.read_or_cancelled(&mut stream, &mut chunk, cancel)?;
            if read == 0 {
                return Err(LiveBackendError::InvalidHttpResponse(
                    "connection closed before headers arrived",
                ));
            }
            raw.extend_from_slice(&chunk[..read]);
            if let Some(pos) = find_subslice(&raw, b"\r\n\r\n") {
                break pos;
            }
        };
        let time_to_first_byte_us = elapsed_us(first_byte_started);

        let header_text = std::str::from_utf8(&raw[..head_end])
            .map_err(|_| LiveBackendError::InvalidHttpResponse("headers were not UTF-8"))?
            .to_owned();
        let status = parse_status_line(header_text.lines().next().unwrap_or(""))?;
        let mut pending = raw[head_end + 4..].to_vec();
        if !(200..300).contains(&status) {
            // Best-effort: drain whatever body has arrived so far for the
            // error message rather than blocking further on a bad response.
            return Err(LiveBackendError::HttpStatus {
                status,
                body: String::from_utf8_lossy(&pending).into_owned(),
            });
        }
        let chunked = header_text
            .lines()
            .any(|line| line.eq_ignore_ascii_case("transfer-encoding: chunked"));
        let content_length = header_text.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if !name.trim().eq_ignore_ascii_case("content-length") {
                return None;
            }
            value.trim().parse::<usize>().ok()
        });

        let read_started = Instant::now();
        let mut decoded = Vec::new();
        let mut consumed_len = 0usize;
        let mut accumulated = String::new();
        let mut line_scan_start = 0usize;
        let mut done = false;

        loop {
            if chunked {
                loop {
                    match take_one_chunk(&pending) {
                        ChunkTake::Complete { data, consumed } => {
                            decoded.extend_from_slice(&data);
                            pending.drain(..consumed);
                        }
                        ChunkTake::Terminal { consumed } => {
                            pending.drain(..consumed);
                            done = true;
                            break;
                        }
                        ChunkTake::Incomplete => break,
                    }
                }
            } else if let Some(total) = content_length {
                let take = pending.len().min(total.saturating_sub(consumed_len));
                decoded.extend_from_slice(&pending[..take]);
                consumed_len += take;
                pending.drain(..take);
                if consumed_len >= total {
                    done = true;
                }
            } else {
                decoded.append(&mut pending);
            }

            while let Some(offset) = decoded[line_scan_start..].iter().position(|&b| b == b'\n') {
                let line_end = line_scan_start + offset;
                let line =
                    String::from_utf8_lossy(&decoded[line_scan_start..line_end]).into_owned();
                let line = line.trim_end_matches('\r');
                line_scan_start = line_end + 1;
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data == "[DONE]" {
                    done = true;
                    break;
                }
                if data.is_empty() {
                    continue;
                }
                let event: ChatCompletionChunk = serde_json::from_str(data)?;
                for choice in event.choices {
                    if let Some(content) = choice.delta.content {
                        accumulated.push_str(&content);
                    }
                    if choice.finish_reason.is_some() {
                        done = true;
                    }
                }
            }

            if done {
                break;
            }

            let read = self.read_or_cancelled(&mut stream, &mut chunk, cancel)?;
            if read == 0 {
                if !chunked && content_length.is_none() {
                    // EOF-terminated body: this is the normal end.
                    break;
                }
                return Err(LiveBackendError::InvalidHttpResponse(
                    "stream ended before its terminal marker",
                ));
            }
            pending.extend_from_slice(&chunk[..read]);
        }
        let response_read_us = elapsed_us(read_started);

        Ok((
            accumulated,
            HttpTiming {
                connect_us,
                request_write_us,
                time_to_first_byte_us,
                response_read_us,
                http_parse_us: 0,
                total_us: elapsed_us(total_started),
            },
        ))
    }

    fn read_or_cancelled(
        &self,
        stream: &mut TcpStream,
        buf: &mut [u8],
        cancel: &CancelHandle,
    ) -> Result<usize, LiveBackendError> {
        match stream.read(buf) {
            Ok(0) if cancel.is_cancelled() => Err(LiveBackendError::Cancelled),
            Ok(n) => Ok(n),
            Err(_) if cancel.is_cancelled() => Err(LiveBackendError::Cancelled),
            Err(error) => Err(LiveBackendError::Io(error)),
        }
    }
}

impl InferenceBackend for LiveBackend {
    fn health(&self, _case_id: Option<&str>) -> SidecarHealth {
        let base_ready = self
            .request("GET", "/health", None)
            .is_ok_and(|body| healthy_response(&body, None));
        let Some(global_endpoint) = &self.global_endpoint else {
            return if base_ready {
                ready_health(true, false)
            } else {
                unavailable_health(
                    "The local Base model server is not reachable at the configured loopback address.",
                )
            };
        };
        let global = self.for_endpoint(global_endpoint);
        let global_ready = global
            .request("GET", "/health", None)
            .is_ok_and(|body| healthy_response(&body, Some(&global_endpoint.model_id)));
        match (base_ready, global_ready) {
            (true, true) => ready_health(true, true),
            (true, false) => degraded_health(
                true,
                false,
                "global_adapter_unavailable",
                "Base is ready, but the configured Global adapter server is unavailable.",
            ),
            (false, true) => ready_health(false, true),
            (false, false) => unavailable_health(
                "Neither configured local model server is reachable at its loopback address.",
            ),
        }
    }

    fn predict(&self, request: &PredictionRequest) -> PredictionResult {
        let result = match request.model_variant {
            ModelVariant::Base => self.predict_base(request),
            ModelVariant::Global => {
                let Some(endpoint) = &self.global_endpoint else {
                    return prediction_error(
                        request,
                        "adapter_not_loaded",
                        "The global Freesolo adapter is not configured.",
                        false,
                    );
                };
                self.for_endpoint(endpoint).predict_base(request)
            }
            ModelVariant::GlobalPlusPersonal => {
                return prediction_error(
                    request,
                    "adapter_not_loaded",
                    "The personal adapter is not loaded yet.",
                    false,
                );
            }
        };

        result.unwrap_or_else(|error| {
            prediction_error(request, "live_inference_failed", &error.to_string(), true)
        })
    }

    fn benchmark(&self, request: &PredictionRequest) -> Result<LiveBenchmark, String> {
        self.benchmark_prediction(request)
            .map_err(|error| error.to_string())
    }
}

fn healthy_response(body: &[u8], expected_model: Option<&str>) -> bool {
    if body == b"OK" {
        return expected_model.is_none();
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    if value.get("status").and_then(Value::as_str) != Some("healthy") {
        return false;
    }
    expected_model.is_none_or(|expected| {
        value.get("loaded_model").and_then(Value::as_str) == Some(expected)
            && value
                .get("loaded_adapter")
                .and_then(Value::as_str)
                .is_some_and(|adapter| !adapter.trim().is_empty())
    })
}

fn ready_health(base: bool, global_adapter: bool) -> SidecarHealth {
    SidecarHealth {
        status: HealthStatus::Ready,
        fixture_available: true,
        loaded: LoadedArtifacts {
            base,
            global_adapter,
            user_adapter: false,
        },
        error: None,
    }
}

fn degraded_health(base: bool, global_adapter: bool, code: &str, message: &str) -> SidecarHealth {
    SidecarHealth {
        status: HealthStatus::Degraded,
        fixture_available: true,
        loaded: LoadedArtifacts {
            base,
            global_adapter,
            user_adapter: false,
        },
        error: Some(ErrorInfo {
            code: code.to_owned(),
            message: message.to_owned(),
            retryable: true,
        }),
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
    if !(1..=config.completion_count).contains(&config.early_exit_agreement) {
        return Err(LiveBackendError::InvalidConfig(
            "early_exit_agreement must be between 1 and completion_count".to_owned(),
        ));
    }
    Ok(())
}

fn completion_seed(index: usize) -> u64 {
    COMPLETION_SEEDS[index % COMPLETION_SEEDS.len()]
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

fn decode_model_output(
    content: &str,
    output_contract: OutputContract,
) -> Result<String, LiveBackendError> {
    let suggestion = match output_contract {
        OutputContract::PlainText => content.trim().to_owned(),
        OutputContract::JsonSuggestion => {
            let output: JsonSuggestion = serde_json::from_str(content.trim())?;
            output.suggestion.trim().to_owned()
        }
    };
    if suggestion.is_empty() {
        return Err(LiveBackendError::MissingModelContent);
    }
    Ok(suggestion)
}

/// Stable-first field order: `context_snippets` and `personal_patterns` stay
/// constant for an entire composition session, while `text` (the sliding
/// window's draft) changes on every burst. Serializing the stable fields
/// first — and `text` last — means consecutive sliding-window requests
/// within a session share the longest possible literal byte prefix up to
/// where the draft actually diverges. The repository MLX server uses this to
/// cache the stable system/context layers and prefill only the draft suffix.
fn model_input(request: &PredictionRequest) -> Value {
    if request.context_snippets.is_empty() && request.personal_patterns.is_empty() {
        json!({"text": request.draft})
    } else {
        json!({
            "context_snippets": request.context_snippets,
            "personal_patterns": request.personal_patterns,
            "text": request.draft,
        })
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletion {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
    #[serde(default)]
    timings: Option<ChatTimings>,
    #[serde(default)]
    quip_timings: Option<ModelServerTiming>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    index: usize,
    message: ChatMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonSuggestion {
    suggestion: String,
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

#[derive(Debug, Clone, Copy, Deserialize)]
struct ChatTimings {
    #[serde(default)]
    prompt_ms: Option<f64>,
    #[serde(default)]
    predicted_ms: Option<f64>,
    #[serde(default)]
    prompt_per_token_ms: Option<f64>,
    #[serde(default)]
    predicted_per_token_ms: Option<f64>,
}

fn token_profile(
    content: &str,
    suggestion: &str,
    usage: ChatUsage,
    timings: Option<ChatTimings>,
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
    let model_reported_total_ms = usage
        .total_time_sec
        .map(|seconds| seconds * 1000.0)
        .or_else(|| {
            timings.map(|value| value.prompt_ms.unwrap_or(0.0) + value.predicted_ms.unwrap_or(0.0))
        });
    let prompt_prefill_ms = usage
        .total_prompt_time_sec
        .map(|seconds| seconds * 1000.0)
        .or_else(|| timings.and_then(|value| value.prompt_ms));
    let completion_decode_ms = usage
        .total_completion_time_sec
        .map(|seconds| seconds * 1000.0)
        .or_else(|| timings.and_then(|value| value.predicted_ms));
    let prompt_ms_per_token = usage
        .avg_prompt_tok_per_sec
        .filter(|rate| *rate > 0.0)
        .map(|rate| 1000.0 / rate)
        .or_else(|| timings.and_then(|value| value.prompt_per_token_ms))
        .or_else(|| {
            prompt_prefill_ms
                .filter(|_| usage.prompt_tokens > 0)
                .map(|ms| ms / usage.prompt_tokens as f64)
        });
    let completion_ms_per_token = usage
        .avg_compl_tok_per_sec
        .filter(|rate| *rate > 0.0)
        .map(|rate| 1000.0 / rate)
        .or_else(|| timings.and_then(|value| value.predicted_per_token_ms))
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

/// `cancelled` counts completions the coordinator deliberately aborted once
/// early-exit agreement was reached (see `benchmark_prediction`) — those are
/// evidence the batch worked, not a failure. Anything else missing from
/// `outputs` (a real transport/parse error) still fails the whole batch: a
/// genuinely incomplete sample would corrupt the vote count.
fn normalize_model_outputs(
    request: &PredictionRequest,
    outputs: Vec<String>,
    expected_output_count: usize,
    cancelled: usize,
    latency_ms: u64,
) -> PredictionResult {
    if outputs.len() + cancelled != expected_output_count {
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
    let (candidates, votes): (Vec<String>, Vec<u32>) = ranked
        .into_iter()
        .take(5)
        .map(|(suggestion, (vote_count, _))| (suggestion, vote_count as u32))
        .unzip();

    let result = PredictionResult::Ok {
        request_id: request.request_id.clone(),
        model_variant: request.model_variant,
        backend: Backend::Live,
        candidates,
        votes: Some(votes),
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
    let status = parse_status_line(headers.lines().next().unwrap_or(""))?;
    let body = response[separator + 4..].to_vec();

    if !(200..300).contains(&status) {
        return Err(LiveBackendError::HttpStatus {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        });
    }

    Ok(body)
}

fn parse_status_line(line: &str) -> Result<u16, LiveBackendError> {
    line.split_whitespace()
        .nth(1)
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or(LiveBackendError::InvalidHttpResponse("missing status code"))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// One step of decoding an HTTP chunked-transfer-encoded body: `<hex
/// size>\r\n<data>\r\n`, terminated by a zero-size chunk. Trailers after the
/// terminal chunk (rare for SSE) are not parsed; the caller stops reading
/// once `Terminal` confirms the blank line that ends the zero chunk.
enum ChunkTake {
    Complete { data: Vec<u8>, consumed: usize },
    Terminal { consumed: usize },
    Incomplete,
}

fn take_one_chunk(buf: &[u8]) -> ChunkTake {
    let Some(line_end) = find_subslice(buf, b"\r\n") else {
        return ChunkTake::Incomplete;
    };
    let Ok(size_line) = std::str::from_utf8(&buf[..line_end]) else {
        return ChunkTake::Incomplete;
    };
    let size_hex = size_line.split(';').next().unwrap_or("").trim();
    let Ok(size) = usize::from_str_radix(size_hex, 16) else {
        return ChunkTake::Incomplete;
    };
    // Saturating: a malformed or hostile chunk-size header must degrade to
    // "not enough bytes yet" (and eventually a stalled-stream error), never
    // an arithmetic-overflow panic.
    let data_start = line_end.saturating_add(2);
    if size == 0 {
        if buf.len() < data_start.saturating_add(2) {
            return ChunkTake::Incomplete;
        }
        return ChunkTake::Terminal {
            consumed: data_start + 2,
        };
    }
    let data_end = data_start.saturating_add(size);
    if buf.len() < data_end.saturating_add(2) {
        return ChunkTake::Incomplete;
    }
    ChunkTake::Complete {
        data: buf[data_start..data_end].to_vec(),
        consumed: data_end + 2,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::Arc,
        thread,
        time::Duration,
    };

    use quip_contracts::{ContextSnippet, ModelVariant, PersonalPattern, PredictionRequest};
    use serde_json::json;

    use super::{
        completion_seed, decode_model_output, healthy_response, is_implausibly_truncated,
        is_model_scaffolding, model_input, normalize_model_outputs, token_profile, CancelHandle,
        ChatTimings, ChatUsage, LiveBackend, LiveConfig, OutputContract, DEFAULT_COMPLETION_COUNT,
        SYSTEM_PROMPT,
    };
    use crate::InferenceBackend;

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
    fn global_endpoint_must_be_loopback_and_identified() {
        let backend = LiveBackend::new("127.0.0.1:1234").unwrap();
        assert!(backend
            .clone()
            .with_global_endpoint("0.0.0.0:1235", "global".to_owned())
            .is_err());
        assert!(backend
            .with_global_endpoint("127.0.0.1:1235", " ".to_owned())
            .is_err());
    }

    #[test]
    fn recognizes_mistral_and_matching_mlx_health_shapes() {
        assert!(healthy_response(b"OK", None));
        assert!(!healthy_response(b"OK", Some("global")));
        let mlx = br#"{"status":"healthy","loaded_model":"global","loaded_adapter":"adapter"}"#;
        assert!(healthy_response(mlx, Some("global")));
        assert!(!healthy_response(mlx, Some("other")));
        assert!(!healthy_response(
            br#"{"status":"healthy","loaded_model":"global","loaded_adapter":null}"#,
            Some("global")
        ));
    }

    #[test]
    fn unloaded_adapter_variants_never_silently_run_base() {
        let backend = LiveBackend::new("127.0.0.1:1234").unwrap();
        for variant in [ModelVariant::Global, ModelVariant::GlobalPlusPersonal] {
            let mut request = request();
            request.model_variant = variant;
            assert!(matches!(
                backend.predict(&request),
                quip_contracts::PredictionResult::Error { error, .. }
                    if error.code == "adapter_not_loaded" && !error.retryable
            ));
        }
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
    fn output_contracts_decode_independently() {
        assert_eq!(
            decode_model_output("call me", OutputContract::PlainText).unwrap(),
            "call me"
        );
        assert_eq!(
            decode_model_output(
                r#"{"suggestion":"call me"}"#,
                OutputContract::JsonSuggestion
            )
            .unwrap(),
            "call me"
        );
        assert!(
            decode_model_output(r#"{"text":"call me"}"#, OutputContract::JsonSuggestion).is_err()
        );
        assert!(decode_model_output(
            r#"{"suggestion":"call me","reason":"typo"}"#,
            OutputContract::JsonSuggestion
        )
        .is_err());
    }

    #[test]
    fn five_completion_slots_use_distinct_stable_seeds() {
        let seeds = (0..DEFAULT_COMPLETION_COUNT)
            .map(completion_seed)
            .collect::<Vec<_>>();
        assert_eq!(seeds, vec![101, 202, 303, 404, 505]);
        assert_eq!(
            seeds.iter().collect::<std::collections::HashSet<_>>().len(),
            5
        );
    }

    #[test]
    fn model_input_extends_the_prototype_shape_only_with_real_context() {
        let mut input_request = request();
        assert_eq!(model_input(&input_request), json!({"text": "cnt cm tmr"}));

        input_request.context_snippets = vec![ContextSnippet {
            app_name: "Slack".to_owned(),
            window_title: "#launch".to_owned(),
            visible_text: "Mira asked for the report tomorrow".to_owned(),
        }];
        input_request.personal_patterns = vec![PersonalPattern {
            shorthand: "tmr".to_owned(),
            expansion: "tomorrow".to_owned(),
        }];
        assert_eq!(
            model_input(&input_request),
            json!({
                "context_snippets": [{
                    "app_name": "Slack",
                    "window_title": "#launch",
                    "visible_text": "Mira asked for the report tomorrow",
                }],
                "personal_patterns": [{
                    "shorthand": "tmr",
                    "expansion": "tomorrow",
                }],
                "text": "cnt cm tmr",
            })
        );
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
    fn token_profile_reports_zero_schema_tax_for_plain_text() {
        let profile = token_profile(
            "can't meet tomorrow",
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
            None,
            Some("stop".to_owned()),
            500_000,
            32,
        );

        assert_eq!(profile.prompt_tokens, 80);
        assert_eq!(profile.completion_tokens, 10);
        assert_eq!(profile.deterministic_schema_chars, 0);
        assert_eq!(profile.estimated_schema_tokens, 0.0);
        assert_eq!(profile.server_ms_per_output_token, 50.0);
        assert_eq!(profile.completion_ms_per_token, Some(25.0));
        assert_eq!(profile.prompt_prefill_ms, Some(250.0));
        assert!(!profile.max_tokens_reached);
    }

    #[test]
    fn token_profile_maps_mlx_server_timings() {
        let profile = token_profile(
            "controversy",
            "controversy",
            ChatUsage {
                prompt_tokens: 80,
                completion_tokens: 4,
                total_tokens: 84,
                avg_prompt_tok_per_sec: None,
                avg_compl_tok_per_sec: None,
                total_time_sec: None,
                total_prompt_time_sec: None,
                total_completion_time_sec: None,
            },
            Some(ChatTimings {
                prompt_ms: Some(400.0),
                predicted_ms: Some(200.0),
                prompt_per_token_ms: Some(5.0),
                predicted_per_token_ms: Some(50.0),
            }),
            Some("stop".to_owned()),
            700_000,
            32,
        );

        assert_eq!(profile.model_reported_total_ms, Some(600.0));
        assert_eq!(profile.prompt_prefill_ms, Some(400.0));
        assert_eq!(profile.completion_decode_ms, Some(200.0));
        assert_eq!(profile.prompt_ms_per_token, Some(5.0));
        assert_eq!(profile.completion_ms_per_token, Some(50.0));
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
            0,
            12,
        );

        assert!(matches!(
            result,
            quip_contracts::PredictionResult::Ok {
                candidates,
                votes,
                ..
            } if candidates == vec![
                "candidate b result",
                "candidate a result",
                "candidate c result",
            ] && votes == Some(vec![2, 2, 1])
        ));
    }

    #[test]
    fn exact_draft_suggestion_becomes_zero_candidates() {
        let result = normalize_model_outputs(
            &request(),
            (0..5).map(|_| "cnt cm tmr".to_owned()).collect(),
            5,
            0,
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
            0,
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
            (0..4)
                .map(|index| format!("candidate result {index}"))
                .collect(),
            5,
            0,
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

    #[test]
    fn cancelled_completions_are_not_an_incomplete_batch() {
        // Early-exit cancelled one of five completions: four real outputs
        // plus one deliberate cancellation still satisfies the batch.
        let result = normalize_model_outputs(
            &request(),
            (0..4)
                .map(|index| format!("candidate result {index}"))
                .collect(),
            5,
            1,
            12,
        );

        assert!(matches!(
            result,
            quip_contracts::PredictionResult::Ok { candidates, .. } if candidates.len() == 4
        ));
    }

    // ---- streaming / cancellation, against a mock TCP server ----
    //
    // No live Qwen server is available in this environment, so these tests
    // exercise the wire-level SSE decoding, chunked-transfer-encoding
    // framing, and cross-thread cancellation against a hand-rolled mock
    // server instead. They prove the client-side logic is correct; they do
    // not replace validating against a real mistral.rs server (its exact
    // framing choice — chunked vs EOF-terminated — is unconfirmed) before
    // trusting this in production.

    fn sse_body_for(suggestion: &str) -> Vec<u8> {
        let chunk1 = format!(
            "data: {}\n\n",
            json!({"choices": [{"delta": {"content": suggestion}}]})
        );
        let chunk2 = format!(
            "data: {}\n\n",
            json!({"choices": [{"delta": {}, "finish_reason": "stop"}]})
        );
        format!("{chunk1}{chunk2}data: [DONE]\n\n").into_bytes()
    }

    fn to_chunked_encoding(body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for piece in body.chunks(29) {
            out.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
            out.extend_from_slice(piece);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"0\r\n\r\n");
        out
    }

    /// Drains one HTTP request off `stream` (headers plus its
    /// Content-Length body) so the mock server behaves like a real one
    /// instead of racing the client's write.
    fn read_one_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0, "client closed before sending a full request");
            buf.extend_from_slice(&chunk[..read]);
            let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&buf[..head_end]).into_owned();
            let content_length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if buf.len() >= head_end + 4 + content_length {
                return buf;
            }
        }
    }

    fn drain_one_request(stream: &mut TcpStream) {
        let _ = read_one_request(stream);
    }

    #[test]
    fn cache_fork_sends_one_prototype_prompt_and_decodes_five_choices() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request_bytes = read_one_request(&mut stream);
            let request_text = String::from_utf8(request_bytes).unwrap();
            let (_, body) = request_text.split_once("\r\n\r\n").unwrap();
            let body: serde_json::Value = serde_json::from_str(body).unwrap();
            assert!(request_text.starts_with("POST /v1/quip/completions HTTP/1.1"));
            assert_eq!(body["quip_completion_count"], 5);
            assert_eq!(body["messages"][0]["content"], SYSTEM_PROMPT);
            assert_eq!(body["messages"][1]["content"], r#"{"text":"cnt cm tmr"}"#);

            let choices = (0..5)
                .map(|index| {
                    json!({
                        "index": index,
                        "message": {"content": format!("candidate {index}")},
                        "finish_reason": "stop"
                    })
                })
                .collect::<Vec<_>>();
            let response_body = serde_json::to_vec(&json!({
                "choices": choices,
                "quip_timings": {
                    "system_cache_hit": true,
                    "context_cache_hit": true,
                    "prepare_us": 1,
                    "prefill_us": 2,
                    "system_prefill_us": 0,
                    "context_prefill_us": 0,
                    "draft_prefill_us": 2,
                    "batching_us": 3,
                    "decode_us": 4,
                    "postprocess_us": 5,
                    "overhead_us": 6,
                    "total_us": 20
                }
            }))
            .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            )
            .unwrap();
            stream.write_all(&response_body).unwrap();
        });

        let backend = LiveBackend::new(&addr.to_string()).unwrap();
        let batch = backend
            .complete_cache_fork_batch_once(&request())
            .expect("the cache-fork batch should decode");
        assert_eq!(batch.outputs.len(), 5);
        assert!(batch.model_server.system_cache_hit);
        assert!(batch.model_server.context_cache_hit);
        server.join().unwrap();
    }

    #[test]
    fn streams_and_decodes_an_eof_terminated_sse_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            drain_one_request(&mut stream);
            let body = sse_body_for("Can't come tomorrow.");
            let mut response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
            response.extend_from_slice(&body);
            stream.write_all(&response).unwrap();
        });

        let backend = LiveBackend::new(&addr.to_string()).unwrap();
        let cancel = CancelHandle::default();
        let timed = backend
            .complete_base_streaming(&request(), &cancel)
            .expect("streaming completion should decode");
        assert_eq!(timed.output, "Can't come tomorrow.");
        server.join().unwrap();
    }

    #[test]
    fn streams_and_decodes_a_chunked_sse_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            drain_one_request(&mut stream);
            let body = to_chunked_encoding(&sse_body_for("Can't come tomorrow."));
            let mut response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
            response.extend_from_slice(&body);
            stream.write_all(&response).unwrap();
        });

        let backend = LiveBackend::new(&addr.to_string()).unwrap();
        let cancel = CancelHandle::default();
        let timed = backend
            .complete_base_streaming(&request(), &cancel)
            .expect("chunked streaming completion should decode");
        assert_eq!(timed.output, "Can't come tomorrow.");
        server.join().unwrap();
    }

    #[test]
    fn retries_a_malformed_stream_fragment() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            drain_one_request(&mut first);
            let malformed = b"data: {\"choices\":[{\"delta\":{\"content\":\"cut off\"}\n\n";
            let mut response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
            response.extend_from_slice(malformed);
            first.write_all(&response).unwrap();
            drop(first);

            let (mut second, _) = listener.accept().unwrap();
            drain_one_request(&mut second);
            let body = sse_body_for("Can't come tomorrow.");
            let mut response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
            response.extend_from_slice(&body);
            second.write_all(&response).unwrap();
        });

        let backend = LiveBackend::new(&addr.to_string()).unwrap();
        let cancel = CancelHandle::default();
        let timed = backend
            .complete_base_streaming(&request(), &cancel)
            .expect("the malformed first stream should be retried");
        assert_eq!(timed.output, "Can't come tomorrow.");
        server.join().unwrap();
    }

    #[test]
    fn cancellation_unblocks_a_stalled_read() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            drain_one_request(&mut stream);
            // Never respond: park on a read that only ends when the client
            // (the cancellation under test) shuts the socket down.
            let mut park = [0_u8; 1];
            let _ = stream.read(&mut park);
        });

        let backend = LiveBackend::new(&addr.to_string()).unwrap();
        let cancel = Arc::new(CancelHandle::default());
        let worker_cancel = Arc::clone(&cancel);
        let worker =
            thread::spawn(move || backend.complete_base_streaming(&request(), &worker_cancel));

        // Give the worker time to connect and register its stream before
        // cancelling, without depending on exact timing for correctness —
        // `cancel()` is a no-op-safe idempotent call either way.
        thread::sleep(Duration::from_millis(100));
        cancel.cancel();

        let result = worker.join().unwrap();
        assert!(
            matches!(result, Err(super::LiveBackendError::Cancelled)),
            "expected Cancelled, got {result:?}"
        );
        server.join().unwrap();
    }

    #[test]
    fn run_streaming_batch_cancels_stragglers_once_early_exit_agrees() {
        let completion_count = 5;
        let early_exit_agreement = 3;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let mut served_fast = 0;
            for _ in 0..completion_count {
                let (mut stream, _) = listener.accept().unwrap();
                drain_one_request(&mut stream);
                if served_fast < early_exit_agreement {
                    served_fast += 1;
                    let body = sse_body_for("Can't come tomorrow.");
                    let mut response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
                    response.extend_from_slice(&body);
                    let _ = stream.write_all(&response);
                } else {
                    // Straggler: never respond. A correct coordinator
                    // cancels this connection instead of waiting on it.
                    let mut park = [0_u8; 1];
                    let _ = stream.read(&mut park);
                }
            }
        });

        let config = LiveConfig {
            completion_count,
            streaming: true,
            early_exit_agreement,
            ..LiveConfig::default()
        };
        let backend = LiveBackend::with_config(&addr.to_string(), config).unwrap();
        let (outputs, cancelled_count, timings) = backend
            .run_streaming_batch(&request())
            .expect("early-exit agreement should still produce a successful batch");

        assert_eq!(outputs.len(), early_exit_agreement);
        assert_eq!(cancelled_count, completion_count - early_exit_agreement);
        assert_eq!(timings.len(), early_exit_agreement);
        assert!(outputs
            .iter()
            .all(|output| output == "Can't come tomorrow."));

        server.join().unwrap();
    }
}
