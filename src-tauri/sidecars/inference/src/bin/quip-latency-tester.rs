use std::{
    env, fs,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{self, Child, ChildStdin, ChildStdout, Command, Stdio},
    time::Instant,
};

use quip_contracts::{
    HealthStatus, ModelVariant, PredictionRequest, PredictionResult, SidecarHealth,
};
use quip_inference_sidecar::{LiveBackend, LiveBenchmark, LiveConfig, PipelineTiming};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

const DEFAULT_PHRASES: &[&str] = &[
    "cnt cm tmr",
    "i went to the store instaed",
    "https://freesolo.co/docs",
];

fn main() {
    match Options::parse().and_then(run) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("latency benchmark failed: {error}");
            process::exit(1);
        }
    }
}

#[derive(Debug)]
struct Options {
    address: String,
    model_id: String,
    output_contract: String,
    label: Option<String>,
    runs: usize,
    warmup: usize,
    completion_count: usize,
    temperature: f64,
    max_tokens: u64,
    streaming: bool,
    early_exit_agreement: usize,
    cache_fork: bool,
    schema_token_elision: bool,
    phrases: Vec<String>,
    json: bool,
    html: Option<PathBuf>,
}

struct BenchmarkSidecar {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl BenchmarkSidecar {
    fn spawn(
        address: &str,
        config: &LiveConfig,
        output_contract: &str,
        cache_fork: bool,
        schema_token_elision: bool,
    ) -> Result<Self, String> {
        let executable = resolve_sidecar()?;
        let mut child = Command::new(&executable)
            .arg("--live")
            .env("QUIP_MODEL_ADDR", address)
            .env("QUIP_MODEL_ID", &config.model_id)
            .env("QUIP_MODEL_OUTPUT_CONTRACT", output_contract)
            .env("QUIP_COMPLETION_COUNT", config.completion_count.to_string())
            .env("QUIP_TEMPERATURE", config.temperature.to_string())
            .env("QUIP_MAX_TOKENS", config.max_tokens.to_string())
            .env("QUIP_STREAMING", config.streaming.to_string())
            .env(
                "QUIP_EARLY_EXIT_AGREEMENT",
                config.early_exit_agreement.to_string(),
            )
            .env("QUIP_CACHE_FORK", cache_fork.to_string())
            .env(
                "QUIP_SCHEMA_TOKEN_ELISION",
                schema_token_elision.to_string(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", executable.display()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "sidecar stdin was unavailable".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "sidecar stdout was unavailable".to_owned())?;
        Ok(Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
        })
    }

    fn health(&mut self) -> Result<SidecarHealth, String> {
        self.exchange(json!({"operation": "health"}))
    }

    fn benchmark(&mut self, request: &PredictionRequest) -> Result<LiveBenchmark, String> {
        self.exchange(json!({"operation": "benchmark", "request": request}))
    }

    fn exchange<T: DeserializeOwned>(&mut self, command: Value) -> Result<T, String> {
        serde_json::to_writer(&mut self.stdin, &command).map_err(|error| error.to_string())?;
        self.stdin
            .write_all(b"\n")
            .map_err(|error| error.to_string())?;
        self.stdin.flush().map_err(|error| error.to_string())?;

        let mut line = String::new();
        let bytes = self
            .stdout
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if bytes == 0 {
            return Err("the live sidecar closed its output stream".to_owned());
        }
        let value: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        if value.get("status").and_then(Value::as_str) == Some("benchmark_error") {
            let message = value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("the sidecar benchmark failed");
            return Err(message.to_owned());
        }
        serde_json::from_value(value).map_err(|error| error.to_string())
    }
}

impl Drop for BenchmarkSidecar {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Options {
    fn parse() -> Result<Self, String> {
        let mut options = Self {
            address: env::var("QUIP_MODEL_ADDR").unwrap_or_else(|_| "127.0.0.1:1234".to_owned()),
            model_id: env::var("QUIP_MODEL_ID").unwrap_or_else(|_| "default".to_owned()),
            output_contract: env::var("QUIP_MODEL_OUTPUT_CONTRACT")
                .unwrap_or_else(|_| "plain_text".to_owned()),
            label: env::var("QUIP_MODEL_LABEL").ok(),
            runs: 10,
            warmup: 2,
            completion_count: env_value("QUIP_COMPLETION_COUNT", 5)?,
            temperature: env_value("QUIP_TEMPERATURE", 0.1)?,
            max_tokens: env_value("QUIP_MAX_TOKENS", 64)?,
            streaming: env_value("QUIP_STREAMING", false)?,
            early_exit_agreement: env_value("QUIP_EARLY_EXIT_AGREEMENT", 3)?,
            cache_fork: env_value("QUIP_CACHE_FORK", false)?,
            schema_token_elision: env_value("QUIP_SCHEMA_TOKEN_ELISION", false)?,
            phrases: Vec::new(),
            json: false,
            html: None,
        };

        let mut arguments = env::args().skip(1);
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--address" => options.address = next_value(&mut arguments, "--address")?,
                "--model-id" => options.model_id = next_value(&mut arguments, "--model-id")?,
                "--output-contract" => {
                    options.output_contract = next_value(&mut arguments, "--output-contract")?
                }
                "--label" => options.label = Some(next_value(&mut arguments, "--label")?),
                "--runs" => {
                    options.runs = parse_value(&next_value(&mut arguments, "--runs")?, "--runs")?
                }
                "--warmup" => {
                    options.warmup =
                        parse_value(&next_value(&mut arguments, "--warmup")?, "--warmup")?
                }
                "--completions" => {
                    options.completion_count = parse_value(
                        &next_value(&mut arguments, "--completions")?,
                        "--completions",
                    )?
                }
                "--temperature" => {
                    options.temperature = parse_value(
                        &next_value(&mut arguments, "--temperature")?,
                        "--temperature",
                    )?
                }
                "--max-tokens" => {
                    options.max_tokens =
                        parse_value(&next_value(&mut arguments, "--max-tokens")?, "--max-tokens")?
                }
                "--streaming" => options.streaming = true,
                "--early-exit-agreement" => {
                    options.early_exit_agreement = parse_value(
                        &next_value(&mut arguments, "--early-exit-agreement")?,
                        "--early-exit-agreement",
                    )?
                }
                "--cache-fork" => options.cache_fork = true,
                "--concurrent-http" => options.cache_fork = false,
                "--schema-token-elision" => options.schema_token_elision = true,
                "--full-schema-decode" => options.schema_token_elision = false,
                "--phrase" => options
                    .phrases
                    .push(next_value(&mut arguments, "--phrase")?),
                "--json" => options.json = true,
                "--html" => {
                    options.html = Some(PathBuf::from(next_value(&mut arguments, "--html")?))
                }
                "--help" | "-h" => {
                    print_help();
                    process::exit(0);
                }
                unknown => return Err(format!("unknown argument: {unknown}")),
            }
        }

        if options.runs == 0 {
            return Err("--runs must be at least 1".to_owned());
        }
        if !matches!(
            options.output_contract.as_str(),
            "plain_text" | "json_suggestion"
        ) {
            return Err("--output-contract must be plain_text or json_suggestion".to_owned());
        }
        if options.phrases.is_empty() {
            options.phrases = DEFAULT_PHRASES
                .iter()
                .map(|phrase| (*phrase).to_owned())
                .collect();
        }
        Ok(options)
    }
}

fn run(options: Options) -> Result<(), String> {
    let config = LiveConfig {
        model_id: options.model_id.clone(),
        completion_count: options.completion_count,
        temperature: options.temperature,
        max_tokens: options.max_tokens,
        streaming: options.streaming,
        early_exit_agreement: options.early_exit_agreement,
    };
    let backend =
        LiveBackend::with_config(&options.address, config).map_err(|error| error.to_string())?;
    let address = backend.address().to_string();
    let config = backend.config().clone();
    let mut sidecar = BenchmarkSidecar::spawn(
        &address,
        &config,
        &options.output_contract,
        options.cache_fork,
        options.schema_token_elision,
    )?;

    let health_started = Instant::now();
    let health = sidecar.health()?;
    let health_us = elapsed_us(health_started);
    if health.status != HealthStatus::Ready {
        let detail = health
            .error
            .map(|error| format!("{}: {}", error.code, error.message))
            .unwrap_or_else(|| "no health detail".to_owned());
        return Err(format!(
            "model server is not ready at {} ({detail})",
            options.address
        ));
    }

    for index in 0..options.warmup {
        let request = request(index, &options.phrases[index % options.phrases.len()]);
        sidecar
            .benchmark(&request)
            .map_err(|error| format!("warmup {}: {error}", index + 1))?;
    }

    let mut samples = Vec::with_capacity(options.runs);
    for index in 0..options.runs {
        let phrase_index = index % options.phrases.len();
        let phrase = &options.phrases[phrase_index];
        let request = request(options.warmup + index, phrase);
        let sidecar_started = Instant::now();
        let benchmark = sidecar
            .benchmark(&request)
            .map_err(|error| format!("run {}: {error}", index + 1))?;
        let sidecar_round_trip_us = elapsed_us(sidecar_started);
        let (status, candidate_count) = match &benchmark.result {
            PredictionResult::Ok { candidates, .. } => ("ok".to_owned(), candidates.len()),
            PredictionResult::Error { error, .. } => (format!("error:{}", error.code), 0),
        };
        samples.push(RunSample {
            run: index + 1,
            phrase_index: phrase_index + 1,
            input_chars: phrase.chars().count(),
            status,
            candidate_count,
            sidecar_round_trip_us,
            sidecar_protocol_overhead_us: sidecar_round_trip_us
                .saturating_sub(benchmark.timing.backend_total_us),
            timing: benchmark.timing,
        });
    }

    let summary = build_summary(&samples);
    let token_summary = build_token_summary(&samples);
    let output = BenchmarkOutput {
        model_label: options.label.unwrap_or_else(|| options.model_id.clone()),
        address,
        output_contract: options.output_contract,
        cache_fork: options.cache_fork,
        schema_token_elision: options.schema_token_elision,
        warmup_runs: options.warmup,
        measured_runs: options.runs,
        phrase_count: options.phrases.len(),
        health_us,
        config,
        samples,
        summary,
        token_summary,
    };

    if let Some(path) = &options.html {
        write_html_profile(path, &output)?;
    }

    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
        );
    } else {
        print_human(&output);
        if let Some(path) = &options.html {
            println!("\nInteractive profile: {}", path.display());
        }
    }
    Ok(())
}

fn request(index: usize, phrase: &str) -> PredictionRequest {
    PredictionRequest {
        request_id: format!("latency-{index}"),
        profile_id: "profile_default".to_owned(),
        model_variant: ModelVariant::Base,
        draft: phrase.to_owned(),
        context_snippets: Vec::new(),
        personal_patterns: Vec::new(),
    }
}

#[derive(Debug, Serialize)]
struct RunSample {
    run: usize,
    phrase_index: usize,
    input_chars: usize,
    status: String,
    candidate_count: usize,
    sidecar_round_trip_us: u64,
    sidecar_protocol_overhead_us: u64,
    timing: PipelineTiming,
}

#[derive(Debug, Serialize)]
struct BenchmarkOutput {
    model_label: String,
    address: String,
    output_contract: String,
    cache_fork: bool,
    schema_token_elision: bool,
    warmup_runs: usize,
    measured_runs: usize,
    phrase_count: usize,
    health_us: u64,
    config: LiveConfig,
    samples: Vec<RunSample>,
    summary: Vec<StageSummary>,
    token_summary: Option<TokenSummary>,
}

#[derive(Debug, Serialize)]
struct TokenSummary {
    completions_with_usage: usize,
    mean_prompt_tokens: f64,
    mean_completion_tokens: f64,
    mean_estimated_suggestion_tokens: f64,
    mean_estimated_schema_tokens: f64,
    mean_schema_token_share_percent: f64,
    mean_server_ms_per_output_token: f64,
    mean_model_reported_total_ms: Option<f64>,
    mean_prompt_prefill_ms: Option<f64>,
    mean_completion_decode_ms: Option<f64>,
    mean_prompt_ms_per_token: Option<f64>,
    mean_completion_ms_per_token: Option<f64>,
    mean_server_queue_overhead_ms_estimate: Option<f64>,
    mean_estimated_schema_latency_ms: f64,
    max_tokens_reached: usize,
}

#[derive(Debug, Serialize)]
struct StageSummary {
    scope: &'static str,
    stage: &'static str,
    samples: usize,
    mean_ms: f64,
    median_ms: f64,
    p95_ms: f64,
    min_ms: f64,
    max_ms: f64,
}

fn build_summary(samples: &[RunSample]) -> Vec<StageSummary> {
    let mut summary = Vec::new();
    let inference_stage = |name, values: Vec<u64>| summarize("inference", name, values);
    summary.push(inference_stage(
        "sidecar_round_trip",
        samples
            .iter()
            .map(|sample| sample.sidecar_round_trip_us)
            .collect(),
    ));

    let model_server = samples
        .iter()
        .filter_map(|sample| sample.timing.model_server.as_ref())
        .collect::<Vec<_>>();
    let server_stage = |name, values: Vec<u64>| summarize("model_server", name, values);
    if !model_server.is_empty() {
        summary.push(server_stage(
            "prepare",
            model_server
                .iter()
                .map(|timing| timing.prepare_us)
                .collect(),
        ));
        summary.push(server_stage(
            "prefill",
            model_server
                .iter()
                .map(|timing| timing.prefill_us)
                .collect(),
        ));
        summary.push(server_stage(
            "system_prefill",
            model_server
                .iter()
                .map(|timing| timing.system_prefill_us)
                .collect(),
        ));
        summary.push(server_stage(
            "context_prefill",
            model_server
                .iter()
                .map(|timing| timing.context_prefill_us)
                .collect(),
        ));
        summary.push(server_stage(
            "draft_prefill",
            model_server
                .iter()
                .map(|timing| timing.draft_prefill_us)
                .collect(),
        ));
        summary.push(server_stage(
            "fixed_prefix_prefill",
            model_server
                .iter()
                .map(|timing| timing.fixed_prefix_prefill_us)
                .collect(),
        ));
        summary.push(server_stage(
            "cache_fork_batching",
            model_server
                .iter()
                .map(|timing| timing.batching_us)
                .collect(),
        ));
        summary.push(server_stage(
            "decode",
            model_server.iter().map(|timing| timing.decode_us).collect(),
        ));
        summary.push(server_stage(
            "dynamic_value_decode",
            model_server
                .iter()
                .map(|timing| timing.dynamic_decode_us)
                .collect(),
        ));
        summary.push(server_stage(
            "postprocess",
            model_server
                .iter()
                .map(|timing| timing.postprocess_us)
                .collect(),
        ));
        summary.push(server_stage(
            "overhead",
            model_server
                .iter()
                .map(|timing| timing.overhead_us)
                .collect(),
        ));
        summary.push(server_stage(
            "total",
            model_server.iter().map(|timing| timing.total_us).collect(),
        ));

        let cache_hits = model_server
            .iter()
            .filter(|timing| timing.system_cache_hit && timing.context_cache_hit)
            .collect::<Vec<_>>();
        if !cache_hits.is_empty() {
            summary.push(server_stage(
                "cache_hit_prefill",
                cache_hits.iter().map(|timing| timing.prefill_us).collect(),
            ));
            summary.push(server_stage(
                "cache_hit_total",
                cache_hits.iter().map(|timing| timing.total_us).collect(),
            ));
        }
    }
    summary.push(inference_stage(
        "backend_total",
        samples
            .iter()
            .map(|sample| sample.timing.backend_total_us)
            .collect(),
    ));
    summary.push(inference_stage(
        "completion_batch",
        samples
            .iter()
            .map(|sample| sample.timing.completion_batch_us)
            .collect(),
    ));
    summary.push(inference_stage(
        "normalization_ranking",
        samples
            .iter()
            .map(|sample| sample.timing.normalization_us)
            .collect(),
    ));
    summary.push(inference_stage(
        "sidecar_protocol_process",
        samples
            .iter()
            .map(|sample| sample.sidecar_protocol_overhead_us)
            .collect(),
    ));

    let completions = samples
        .iter()
        .flat_map(|sample| sample.timing.completions.iter())
        .collect::<Vec<_>>();
    if !completions.is_empty() {
        let completion_stage = |name, values: Vec<u64>| summarize("completion", name, values);
        summary.push(completion_stage(
            "total",
            completions.iter().map(|timing| timing.total_us).collect(),
        ));
        summary.push(completion_stage(
            "request_build",
            completions
                .iter()
                .map(|timing| timing.request_build_us)
                .collect(),
        ));
        summary.push(completion_stage(
            "connect",
            completions
                .iter()
                .map(|timing| timing.http.connect_us)
                .collect(),
        ));
        summary.push(completion_stage(
            "request_write",
            completions
                .iter()
                .map(|timing| timing.http.request_write_us)
                .collect(),
        ));
        summary.push(completion_stage(
            "server_wait_ttfb",
            completions
                .iter()
                .map(|timing| timing.http.time_to_first_byte_us)
                .collect(),
        ));
        summary.push(completion_stage(
            "response_read",
            completions
                .iter()
                .map(|timing| timing.http.response_read_us)
                .collect(),
        ));
        summary.push(completion_stage(
            "http_parse",
            completions
                .iter()
                .map(|timing| timing.http.http_parse_us)
                .collect(),
        ));
        summary.push(completion_stage(
            "response_decode",
            completions
                .iter()
                .map(|timing| timing.response_decode_us)
                .collect(),
        ));
        summary.push(completion_stage(
            "output_decode",
            completions
                .iter()
                .map(|timing| timing.output_decode_us)
                .collect(),
        ));
    }

    let token_profiles = completions
        .iter()
        .filter_map(|timing| timing.tokens.as_ref())
        .collect::<Vec<_>>();
    let mut push_model_stage = |name, values: Vec<Option<f64>>| {
        let values = values
            .into_iter()
            .flatten()
            .map(|milliseconds| (milliseconds * 1000.0).round() as u64)
            .collect::<Vec<_>>();
        if !values.is_empty() {
            summary.push(summarize("model", name, values));
        }
    };
    push_model_stage(
        "server_queue_overhead",
        token_profiles
            .iter()
            .map(|profile| profile.server_queue_overhead_ms_estimate)
            .collect(),
    );
    push_model_stage(
        "prompt_prefill",
        token_profiles
            .iter()
            .map(|profile| profile.prompt_prefill_ms)
            .collect(),
    );
    push_model_stage(
        "completion_decode",
        token_profiles
            .iter()
            .map(|profile| profile.completion_decode_ms)
            .collect(),
    );
    summary
}

fn build_token_summary(samples: &[RunSample]) -> Option<TokenSummary> {
    let profiles = samples
        .iter()
        .flat_map(|sample| sample.timing.completions.iter())
        .filter_map(|timing| timing.tokens.as_ref())
        .collect::<Vec<_>>();
    if profiles.is_empty() {
        return None;
    }
    let count = profiles.len() as f64;
    let mean = |values: Vec<f64>| values.into_iter().sum::<f64>() / count;
    let mean_optional = |values: Vec<Option<f64>>| {
        let values = values.into_iter().flatten().collect::<Vec<_>>();
        if values.is_empty() {
            None
        } else {
            Some(values.iter().sum::<f64>() / values.len() as f64)
        }
    };
    let mean_completion_tokens = mean(
        profiles
            .iter()
            .map(|profile| profile.completion_tokens as f64)
            .collect(),
    );
    let mean_estimated_schema_tokens = mean(
        profiles
            .iter()
            .map(|profile| profile.estimated_schema_tokens)
            .collect(),
    );
    Some(TokenSummary {
        completions_with_usage: profiles.len(),
        mean_prompt_tokens: mean(
            profiles
                .iter()
                .map(|profile| profile.prompt_tokens as f64)
                .collect(),
        ),
        mean_completion_tokens,
        mean_estimated_suggestion_tokens: mean(
            profiles
                .iter()
                .map(|profile| profile.estimated_suggestion_tokens)
                .collect(),
        ),
        mean_estimated_schema_tokens,
        mean_schema_token_share_percent: if mean_completion_tokens == 0.0 {
            0.0
        } else {
            mean_estimated_schema_tokens / mean_completion_tokens * 100.0
        },
        mean_server_ms_per_output_token: mean(
            profiles
                .iter()
                .map(|profile| profile.server_ms_per_output_token)
                .collect(),
        ),
        mean_model_reported_total_ms: mean_optional(
            profiles
                .iter()
                .map(|profile| profile.model_reported_total_ms)
                .collect(),
        ),
        mean_prompt_prefill_ms: mean_optional(
            profiles
                .iter()
                .map(|profile| profile.prompt_prefill_ms)
                .collect(),
        ),
        mean_completion_decode_ms: mean_optional(
            profiles
                .iter()
                .map(|profile| profile.completion_decode_ms)
                .collect(),
        ),
        mean_prompt_ms_per_token: mean_optional(
            profiles
                .iter()
                .map(|profile| profile.prompt_ms_per_token)
                .collect(),
        ),
        mean_completion_ms_per_token: mean_optional(
            profiles
                .iter()
                .map(|profile| profile.completion_ms_per_token)
                .collect(),
        ),
        mean_server_queue_overhead_ms_estimate: mean_optional(
            profiles
                .iter()
                .map(|profile| profile.server_queue_overhead_ms_estimate)
                .collect(),
        ),
        mean_estimated_schema_latency_ms: mean(
            profiles
                .iter()
                .map(|profile| profile.estimated_schema_latency_ms)
                .collect(),
        ),
        max_tokens_reached: profiles
            .iter()
            .filter(|profile| profile.max_tokens_reached)
            .count(),
    })
}

fn summarize(scope: &'static str, stage: &'static str, mut values: Vec<u64>) -> StageSummary {
    values.sort_unstable();
    let samples = values.len();
    let mean_us = values.iter().map(|value| *value as f64).sum::<f64>() / samples as f64;
    StageSummary {
        scope,
        stage,
        samples,
        mean_ms: mean_us / 1000.0,
        median_ms: percentile(&values, 50) as f64 / 1000.0,
        p95_ms: percentile(&values, 95) as f64 / 1000.0,
        min_ms: values[0] as f64 / 1000.0,
        max_ms: values[samples - 1] as f64 / 1000.0,
    }
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    let rank = (percentile * sorted.len()).div_ceil(100);
    sorted[rank.saturating_sub(1)]
}

fn print_human(output: &BenchmarkOutput) {
    println!("Quip model latency benchmark — {}", output.model_label);
    println!(
        "Endpoint {} · model id {} · {} · {} · schema elision {} · {} completions · temp {} · max {} tokens",
        output.address,
        output.config.model_id,
        output.output_contract,
        if output.cache_fork {
            "layered KV cache fork"
        } else {
            "concurrent HTTP"
        },
        output.schema_token_elision,
        output.config.completion_count,
        output.config.temperature,
        output.config.max_tokens,
    );
    println!(
        "{} warmup · {} measured runs · {} phrases · health {:.3} ms",
        output.warmup_runs,
        output.measured_runs,
        output.phrase_count,
        output.health_us as f64 / 1000.0,
    );
    for sample in &output.samples {
        println!(
            "run {:>2} · phrase {} ({} chars) · {:>2} candidates · {:>10} · {:.3} ms",
            sample.run,
            sample.phrase_index,
            sample.input_chars,
            sample.candidate_count,
            sample.status,
            sample.sidecar_round_trip_us as f64 / 1000.0,
        );
    }
    print_table(
        "Inference pipeline (one sample per run)",
        &output.summary,
        "inference",
    );
    if output.cache_fork {
        print_table(
            "Model server (one prefill, forked five-way decode)",
            &output.summary,
            "model_server",
        );
    } else {
        print_table(
            "Completion request stages (completions run concurrently)",
            &output.summary,
            "completion",
        );
        println!(
            "server_wait_ttfb contains queueing plus generation for this non-streaming endpoint; completion stages do not add to inference total because requests overlap."
        );
    }
    if let Some(tokens) = &output.token_summary {
        println!("\nToken profile (mean per completion)");
        println!(
            "prompt {:.1} · output {:.1} · suggestion est. {:.1} · schema est. {:.1} ({:.1}%)",
            tokens.mean_prompt_tokens,
            tokens.mean_completion_tokens,
            tokens.mean_estimated_suggestion_tokens,
            tokens.mean_estimated_schema_tokens,
            tokens.mean_schema_token_share_percent,
        );
        if let Some(completion_ms_per_token) = tokens.mean_completion_ms_per_token {
            println!(
                "decode {:.3} ms/output token · prefill {:.3} ms · decode total {:.3} ms · queue/overhead est. {:.3} ms",
                completion_ms_per_token,
                tokens.mean_prompt_prefill_ms.unwrap_or_default(),
                tokens.mean_completion_decode_ms.unwrap_or_default(),
                tokens
                    .mean_server_queue_overhead_ms_estimate
                    .unwrap_or_default(),
            );
        } else {
            println!(
                "server {:.3} ms/output token (prefill included)",
                tokens.mean_server_ms_per_output_token,
            );
        }
        println!(
            "schema est. {:.3} ms · max-token stops {}",
            tokens.mean_estimated_schema_latency_ms, tokens.max_tokens_reached,
        );
    }
}

fn write_html_profile(path: &Path, output: &BenchmarkOutput) -> Result<(), String> {
    let mut data = serde_json::to_string(output).map_err(|error| error.to_string())?;
    data = data
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    let html = include_str!("latency-profile-template.html")
        .replace("/*__QUIP_PROFILE_DATA__*/null", &data);
    fs::write(path, html).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn print_table(title: &str, summary: &[StageSummary], scope: &str) {
    println!("\n{title}");
    println!("stage                       mean    median       p95       min       max");
    for stage in summary.iter().filter(|stage| stage.scope == scope) {
        println!(
            "{:<24} {:>8.3} {:>9.3} {:>9.3} {:>9.3} {:>9.3}",
            stage.stage, stage.mean_ms, stage.median_ms, stage.p95_ms, stage.min_ms, stage.max_ms,
        );
    }
}

fn resolve_sidecar() -> Result<PathBuf, String> {
    if let Some(explicit) = env::var_os("QUIP_INFERENCE_SIDECAR") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "QUIP_INFERENCE_SIDECAR does not point to a file: {}",
            path.display()
        ));
    }

    let binary_name = if cfg!(windows) {
        "quip-inference-sidecar.exe"
    } else {
        "quip-inference-sidecar"
    };
    let sibling = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(binary_name)));
    sibling.filter(|path| path.is_file()).ok_or_else(|| {
        "quip-inference-sidecar was not found beside the latency tester; run `cargo build -p quip-inference-sidecar` first"
            .to_owned()
    })
}

fn env_value<T>(name: &str, default: T) -> Result<T, String>
where
    T: std::str::FromStr,
{
    match env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| format!("{name} has an invalid value")),
        Err(_) => Ok(default),
    }
}

fn next_value(arguments: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_value<T>(value: &str, flag: &str) -> Result<T, String>
where
    T: std::str::FromStr,
{
    value
        .parse()
        .map_err(|_| format!("{flag} has an invalid value: {value}"))
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u64::MAX as u128) as u64
}

fn print_help() {
    println!(
        "Quip latency tester\n\n\
Usage: quip-latency-tester [options]\n\n\
  --address HOST:PORT     Loopback model endpoint (default 127.0.0.1:1234)\n\
  --model-id ID           Model field sent to the endpoint (default: default)\n\
  --output-contract NAME  plain_text or json_suggestion (default: plain_text)\n\
  --label NAME            Human-readable model/config label\n\
  --runs N                Measured inference count (default: 10)\n\
  --warmup N              Warmup inference count (default: 2)\n\
  --completions N         Completions per inference, 1-5 (default: 5)\n\
  --temperature N         Sampling temperature, 0-2 (default: 0.1)\n\
  --max-tokens N          Maximum completion tokens, 1-512 (default: 64)\n\
  --streaming             Stream each completion and cancel the rest of the\n\
                          legacy HTTP batch once enough of them agree\n\
  --early-exit-agreement N  Agreeing streamed completions needed to cancel\n\
                          the rest, 1..completions (default: 3)\n\
  --cache-fork            Layered prefill, then fork the completed KV cache and\n\
                          decode all candidates as one model batch\n\
  --schema-token-elision  Prefill fixed suggestion JSON and decode its value only\n\
  --full-schema-decode    Decode the complete guided suggestion JSON\n\
  --concurrent-http       Legacy path: one HTTP request per completion\n\
  --phrase TEXT           Phrase to benchmark; repeat for a phrase set\n\
  --json                  Emit machine-readable samples and summaries\n\
  --html PATH             Write a self-contained interactive latency profile\n\
Environment equivalents: QUIP_MODEL_ADDR, QUIP_MODEL_ID, QUIP_MODEL_LABEL,\n\
QUIP_MODEL_OUTPUT_CONTRACT,\n\
QUIP_COMPLETION_COUNT, QUIP_TEMPERATURE, QUIP_MAX_TOKENS, QUIP_STREAMING,\n\
QUIP_EARLY_EXIT_AGREEMENT, QUIP_CACHE_FORK, QUIP_SCHEMA_TOKEN_ELISION.\n\n\
Benchmark the layered cache-fork path:\n\
  quip-latency-tester --runs 20 --cache-fork --json > cache-fork.json"
    );
}

#[cfg(test)]
mod tests {
    use super::{percentile, summarize};

    #[test]
    fn percentile_uses_nearest_rank() {
        let values = vec![100, 200, 300, 400, 500];
        assert_eq!(percentile(&values, 50), 300);
        assert_eq!(percentile(&values, 95), 500);
    }

    #[test]
    fn summary_reports_milliseconds() {
        let summary = summarize("test", "stage", vec![1_000, 2_000, 3_000]);
        assert_eq!(summary.samples, 3);
        assert_eq!(summary.mean_ms, 2.0);
        assert_eq!(summary.median_ms, 2.0);
        assert_eq!(summary.p95_ms, 3.0);
    }
}
