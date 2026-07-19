# Inference sidecar (Workstream 2)

The sidecar has a deterministic fixture backend and an opt-in live backend.
Base Qwen3.5 and the selected exported global Freesolo LoRA run as separate
loopback-only MLX-VLM servers over the same matching 4-bit base. Quip is locked
to Qwen3.5 2B; the current Global model is the v2 step-80 adapter.
Fixture mode remains the demo-safe fallback and requires no model installation.

The sidecar speaks the Phase 0 shapes (`crates/quip-contracts`,
`docs/phase-0.schema.json`): it answers `prediction_request` with
`prediction_result` and reports `sidecar_health`. The app-side client lives in
`src-tauri/src/inference/`. Model binaries and adapters are local artifacts
under `artifacts/models/` and are never committed.

## Development protocol

The sidecar reads one newline-delimited JSON command at a time from standard
input and writes one Phase 0 result to standard output. It stays alive for
multiple commands and does not log drafts or generated text.

```json
{"operation":"health"}
{"operation":"health","case_id":"adapter_degraded"}
{"operation":"predict","request":{"request_id":"fresh-id","profile_id":"profile_default","model_variant":"base","draft":"cnt cm tmrw","context_snippets":[],"personal_patterns":[]}}
```

Run the repository validation skill after sidecar or contract changes:

```text
.agents/skills/validate-quip-sidecar/scripts/validate.sh
```

## Phrase tester

Compare the fixture base and global variants interactively:

```bash
cargo run --quiet -p quip-inference-sidecar --bin quip-phrase-tester
```

Or test one phrase directly:

```bash
cargo run --quiet -p quip-inference-sidecar --bin quip-phrase-tester -- "cnt cm tmrw"
```

The tester identifies itself as fixture mode because it does not load Qwen.
It lists the prerecorded phrases that currently have results and reports an
unavailable result for other phrases.

## Live phrase inference

Prepare the exported PEFT adapter once. This creates an ignored MLX runtime,
converts the text-model LoRA weights, and downloads the matching 4-bit MLX
base model; it does not modify the exported adapter:

```bash
src-tauri/sidecars/inference/scripts/setup-global-adapter.sh
```

Then run one command from the repository root. It starts the selected global
adapter on port 1235 and opens the same interactive tester in live mode:

```bash
src-tauri/sidecars/inference/scripts/run-live-phrase-tester.sh
```

`QUIP_GLOBAL_MODEL_PRESET` accepts only `2b`. The same locked configuration is
used by `run-live-app.sh` and the validators.

The app launcher enables demo diversity by default. It follows the prototype
playground's exact trained system message and user JSON: `{\"text\": ...}` plus
the request's real `context_snippets` and `personal_patterns`, when present.
One request produces five completions. The repository-owned MLX server keeps a
process-lifetime system-prompt KV cache, reuses the context/pattern cache until
those inputs change, and prefills only the dynamic draft suffix. It then forks
the completed cache and decodes the five candidates as one concurrent model
batch at temperature 0.8. Each row has an independent deterministic sampling
identity. Normal deduplication and vote ranking still produce zero to five
distinct candidates under the existing output contracts.
Set `QUIP_DEMO_DIVERSITY=0` to restore the accuracy/latency mode (temperature
0.1 with 3-vote early exit). Explicit `QUIP_TEMPERATURE` and
`QUIP_EARLY_EXIT_AGREEMENT` values override either mode.

Pass a phrase to perform one inference and exit:

```bash
src-tauri/sidecars/inference/scripts/run-live-phrase-tester.sh "cnt cm tmr"
```

Each available result includes measured end-to-end latency. `global_plus_personal` still
returns `adapter_not_loaded` until a per-user adapter is integrated; it never
silently falls back to the global or base model.

The tuned 2B Global model is the default to avoid keeping two model copies
resident. Set `QUIP_START_BASE_MODEL=1` only when a simultaneous Base comparison
is worth the additional memory; otherwise the tester reports Base as unavailable.

The launcher reuses the Hugging Face cache without replacing model files. The
Base and Global endpoints can be changed with `QUIP_MODEL_ADDR` and
`QUIP_GLOBAL_MODEL_ADDR`. `QUIP_GLOBAL_ADAPTER_DIR` can select another
compatible 2B adapter for an explicit comparison.

The model-server response is normalized before candidates enter the shared
Quip boundary. Both locked 2B endpoints use guided `json_suggestion`; the tuned
checkpoint was trained with this contract:

```json
{"suggestion":"send the draft"}
```

This does not change the app-side `PredictionResult` contract.

The Rust inference layer requests exactly five choices in that cache-forked
batch, removes exact-draft suggestions, deduplicates changed text, ranks
duplicate votes with earliest-completion tie-breaking, and returns zero to five
candidates. Zero candidates means skip and no suggestion bar.

## Latency and model comparison

Benchmark the current 2B v2 adapter under the app's real settings:

```bash
src-tauri/sidecars/inference/scripts/compare-global-models.sh
```

The models run sequentially so only one is resident at a time. Each uses the
same layered KV cache, five-way cache-fork decode, three warmups, latency
phrases, and eval sample. The command prints one aggregate table and deletes
its temporary detailed output. Adjust sample sizes with `--runs`, `--warmups`,
and `--eval-sample`.

Run the benchmark launcher from the repository root. It starts an isolated
loopback model server at the checkpoint's native precision on port 1240,
performs warmups, and then reports mean, median, p95, minimum, and maximum
latency for each stage:

```bash
src-tauri/sidecars/inference/scripts/run-latency-benchmark.sh --runs 10
```

The tester keeps one real NDJSON sidecar child alive. With `--cache-fork`, its
model-server table separates preparation, cache-hit prefill, system/context/
draft prefill, cache forking, concurrent decode, postprocessing, overhead, and
total time. The inference table separately reports sidecar round trip, backend,
protocol/process overhead, the completion batch, and normalization/ranking.
The legacy OpenAI-compatible path remains benchmarkable with
`--concurrent-http`; its completion table retains the transport and response
decoding breakdown.

When the server reports OpenAI token usage, the benchmark records prompt and
completion tokens. mistral.rs additionally reports model time for prompt
prefill and completion decoding, so the profile exposes exact completion
milliseconds per token and decomposes the server interval into queue/overhead,
prefill, and decoding. It also estimates the tokens and decode latency spent
emitting the deterministic `{"suggestion":...}` wrapper. That wrapper split is
estimated from character share because the response supplies aggregate token
counts, not token boundaries. Candidate text and token text are not written to
benchmark output.

Write a self-contained drill-down visualization alongside JSON output:

```bash
src-tauri/sidecars/inference/scripts/run-latency-benchmark.sh \
  --runs 10 --completions 1 --json --html /tmp/quip-latency.html \
  > /tmp/quip-latency.json
open /tmp/quip-latency.html
```

The profile separates sidecar/process overhead, backend work, model-server
inference, transport, parsing, schema decoding, and ranking. Click a component
to decompose it. The statistic selector switches the hierarchy between median,
p95, mean, and maximum, and the file control can load another benchmark JSON.
Mean is the default because component means are additive; independently
calculated percentile components need not sum to the parent percentile.

Benchmark another compatible 2B adapter through explicit artifact overrides. A
server started by the script is stopped after each run:

```bash
QUIP_SERVER_MODEL=Qwen/Qwen3.5-2B \
  src-tauri/sidecars/inference/scripts/run-latency-benchmark.sh --runs 20
```

Useful controls include `QUIP_SERVER_QUANT`, `QUIP_SERVER_READY_TIMEOUT_SECONDS`,
`QUIP_BENCHMARK_PORT`,
`--completions`, `--temperature`, `--max-tokens`, `--html`, and repeatable `--phrase`
arguments. Quantization is disabled by default; set `QUIP_SERVER_QUANT` to a
bit width only when intentionally testing a quantized configuration. Use
`QUIP_SERVER_READY_TIMEOUT_SECONDS` to override the 600-second first-load wait. Use
`--label` when the endpoint's request model ID is `default`, or `--model-id`
for an OpenAI-compatible server that routes by model ID. Add `--json` for
machine-readable samples and summaries; the JSON records phrase indexes and
character counts but does not copy phrase text or candidates.

Benchmark an already-running loopback server without the launcher:

```bash
cargo build --quiet -p quip-inference-sidecar
target/debug/quip-latency-tester \
  --address 127.0.0.1:1234 --label "Qwen3.5-2B 4-bit" --runs 10
```

## Layered KV caching and legacy streaming

The local MLX launcher uses `QUIP_CACHE_FORK=true` by default. Cache keys are
hashes of token IDs, not draft or context text. The system cache lives for the
model-server process. A bounded context/pattern cache is replaced when either
input changes. Every request clones that layer, prefills only the remaining
draft tokens, and then forks the completed KV state across five decode rows.
The server returns cache-hit and per-stage timings to the sidecar benchmark;
these diagnostics never cross the product `PredictionResult` contract.

`QUIP_SCHEMA_TOKEN_ELISION=true` additionally derives the compact
`{"suggestion":"` prefix from the tokenizer in the exact assistant-continuation
context, teacher-forces it into the completed cache once, and forks that state.
Each row decodes only the dynamic string through its first valid unescaped
closing quote; code then synthesizes the closing JSON. Set the variable to
`false` to run canonical full-schema decoding. Both paths preserve the same
token history, guided grammar, five-row sampling, and external JSON contract.
Timings expose fixed-prefix prefill, dynamic-value decode, generated tokens,
avoided schema tokens, and total latency.

`model_input` deliberately serializes `context_snippets` and
`personal_patterns` before `text`, preserving the prototype fields while
maximizing the safe token prefix that can be cached independently of the
sliding draft.

The following streaming path is retained for standard OpenAI-compatible
servers; it is not used by the repository MLX cache-fork launcher.

`LiveConfig.streaming` (`QUIP_STREAMING=true`, or `--streaming` on the
latency tester) switches each of the `completion_count` concurrent
completions from one buffered response to an incrementally-decoded SSE
stream (`stream: true` against `/v1/chat/completions`, handling both
`Transfer-Encoding: chunked` and plain `Connection: close` framing — the
server's actual choice is unconfirmed against a real mistral.rs instance,
which is why both are implemented and covered by mock-server tests in
`live.rs`). These controls apply only when `QUIP_CACHE_FORK=false` and the
configured endpoint implements `/v1/chat/completions`.

With streaming on, `run_streaming_batch` reads completions in the order they
actually finish rather than the order they were dispatched, and once
`early_exit_agreement` of them (`QUIP_EARLY_EXIT_AGREEMENT`, default 3, must
be between 1 and `completion_count`) land on the same suggestion, the rest
are cancelled — their sockets are shut down from the coordinator thread,
which unblocks their blocked `read`s (`CancelHandle` in `live.rs`). A
cancelled completion is not a failure: `normalize_model_outputs` only fails
the batch on a genuine transport/parse error, distinct from an intentional
cancellation.

A/B buffered and streamed legacy HTTP against a compatible server:

```bash
target/debug/quip-latency-tester --address 127.0.0.1:1234 --runs 20 --json > batched.json
target/debug/quip-latency-tester --address 127.0.0.1:1234 --runs 20 --streaming --json > streaming.json
```

Streaming's benefit depends on whether early-exit agreement typically lands
before all five completions finish. The three-model comparison command measures
that optimized path on the current Mac.

## Run the full app with live inference

The team can launch the Tauri app, Rust sidecar, and local Global adapter with
one command:

```bash
src-tauri/sidecars/inference/scripts/run-live-app.sh
```

This development launcher builds the sidecar, starts the Global loopback model
server when necessary, sends three unmeasured warmup requests, and forces
`live` / `global` for that app session without overwriting persisted settings.
It opens the demo harness automatically;
click **Try tuned adapter** to load `cancel next meetihng` and see the adapter
offer `cancel next meeting`. The app keeps one NDJSON sidecar process alive for health and
prediction requests. The model is fixed to `QUIP_GLOBAL_MODEL_PRESET=2b`; set
`QUIP_DEMO_WARMUP_RUNS=0` only when intentionally measuring a cold launch.
