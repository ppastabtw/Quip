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
const COMPLETION_ATTEMPTS: usize = 3;
const DEFAULT_MODEL_ID: &str = "default";
const DEFAULT_COMPLETION_COUNT: usize = 5;
const DEFAULT_TEMPERATURE: f64 = 0.1;
const DEFAULT_MAX_TOKENS: u64 = 64;
const SYSTEM_PROMPT: &str = include_str!("../../../../training/flash/system_prompt.txt");

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
        let (model_outputs, cancelled_count, completion_timings) = if self.config.streaming {
            self.run_streaming_batch(request)?
        } else {
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
            let (outputs, timings): (Vec<_>, Vec<_>) = timed_outputs
                .into_iter()
                .map(|timed| (timed.output, timed.timing))
                .unzip();
            (outputs, 0, timings)
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
            },
        })
    }

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
                        self.complete_base_streaming(request, &handle)
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

    fn complete_base_streaming(
        &self,
        request: &PredictionRequest,
        cancel: &CancelHandle,
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
            "stream": true
        });
        let body = serde_json::to_vec(&body)?;
        let request_build_us = elapsed_us(build_started);
        let (content, http_timing) =
            self.request_streaming("POST", "/v1/chat/completions", &body, cancel)?;
        let response_decode_started = Instant::now();
        let output = content.trim().to_owned();
        if output.is_empty() {
            return Err(LiveBackendError::MissingModelContent);
        }
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
    if !(1..=config.completion_count).contains(&config.early_exit_agreement) {
        return Err(LiveBackendError::InvalidConfig(
            "early_exit_agreement must be between 1 and completion_count".to_owned(),
        ));
    }
    Ok(())
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u64::MAX as u128) as u64
}

/// Stable-first field order: `context_snippets` and `personal_patterns` stay
/// constant for an entire composition session, while `text` (the sliding
/// window's draft) changes on every burst. Serializing the stable fields
/// first — and `text` last — means consecutive sliding-window requests
/// within a session share the longest possible literal byte prefix up to
/// where the draft actually diverges, which is what a server-side prefix
/// cache keys on. Whether this actually reduces measured latency (vs. the
/// server's own batching behavior) needs the 5/10/15-word matrix from
/// `run-latency-benchmark.sh` on real hardware — unverified here.
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
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    #[serde(rename = "index")]
    _index: usize,
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

    use quip_contracts::{ModelVariant, PredictionRequest};
    use serde_json::json;

    use super::{
        is_implausibly_truncated, is_model_scaffolding, normalize_model_outputs, token_profile,
        CancelHandle, ChatUsage, LiveBackend, LiveConfig, SYSTEM_PROMPT,
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
    fn drain_one_request(stream: &mut TcpStream) {
        let mut buf = Vec::new();
        let mut chunk = [0_u8; 4096];
        let content_length = loop {
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
                break content_length;
            }
        };
        let _ = content_length;
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
