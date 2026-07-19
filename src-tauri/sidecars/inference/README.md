# Inference sidecar (Workstream 2)

The sidecar has a deterministic fixture backend and an opt-in live backend.
Base Qwen3.5 and the selected exported global Freesolo LoRA run as separate
loopback-only MLX-VLM servers over the same matching 8-bit base. The default
`4b` preset uses the step-70 adapter; the `0.8b` and `2b` presets use step 80.
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
converts the text-model LoRA weights, and downloads the matching 8-bit MLX
base model; it does not modify the exported adapter:

```bash
src-tauri/sidecars/inference/scripts/setup-global-adapter.sh
```

Prepare either smaller tuned model instead:

```bash
QUIP_GLOBAL_MODEL_PRESET=0.8b \
  src-tauri/sidecars/inference/scripts/setup-global-adapter.sh

QUIP_GLOBAL_MODEL_PRESET=2b \
  src-tauri/sidecars/inference/scripts/setup-global-adapter.sh
```

Then run one command from the repository root. It starts the selected global
adapter on port 1235 and opens the same interactive tester in live mode:

```bash
src-tauri/sidecars/inference/scripts/run-live-phrase-tester.sh
```

Prefix the command with `QUIP_GLOBAL_MODEL_PRESET=0.8b` or `2b` to test a
smaller adapter. Omit it, or set it to `4b`, to retain the existing 4B path.
The same preset works with `run-live-app.sh` for the demo harness.

Pass a phrase to perform one inference and exit:

```bash
src-tauri/sidecars/inference/scripts/run-live-phrase-tester.sh "cnt cm tmr"
```

Each available result includes measured end-to-end latency. `global_plus_personal` still
returns `adapter_not_loaded` until a per-user adapter is integrated; it never
silently falls back to the global or base model.

The tuned Global model is the default to avoid keeping two 8-bit model copies
resident. Set `QUIP_START_BASE_MODEL=1` only when a simultaneous Base comparison
is worth the additional memory; otherwise the tester reports Base as unavailable.

The launcher reuses the Hugging Face cache without replacing model files. The
Base and Global endpoints can be changed with `QUIP_MODEL_ADDR` and
`QUIP_GLOBAL_MODEL_ADDR`. Explicit `QUIP_GLOBAL_MODEL_ID`,
`QUIP_GLOBAL_ADAPTER_DIR`, and `QUIP_GLOBAL_OUTPUT_CONTRACT` values override
the selected preset.

The model-server response contract is selected per endpoint and is normalized
before candidates enter the shared Quip boundary. The 4B preset keeps the
existing `plain_text` response. The 0.8B and 2B checkpoints were trained with
guided JSON and therefore use `json_suggestion`:

```json
{"suggestion":"send the draft"}
```

This is additive: changing one endpoint's response decoder does not change the
app-side `PredictionResult` contract or the other model preset.

The Rust inference layer requests exactly five choices in one batched completion,
removes exact-draft suggestions, deduplicates changed text, ranks duplicate votes with
earliest-completion tie-breaking, and returns zero to five candidates. Zero
candidates means skip and no suggestion bar.

## Latency and model comparison

Compare all three tuned adapters under identical app settings with one command:

```bash
src-tauri/sidecars/inference/scripts/compare-global-models.sh
```

The models run sequentially so only one is resident at a time. Each uses APC,
streaming five-completion inference, 3-vote early exit, three warmups, the same
latency phrases, and the same eval sample. The command prints one aggregate
table and deletes its temporary detailed output. Adjust sample sizes with
`--runs`, `--warmups`, and `--eval-sample`.

Run the benchmark launcher from the repository root. It starts an isolated
loopback model server at the checkpoint's native precision on port 1240,
performs warmups, and then reports mean, median, p95, minimum, and maximum
latency for each stage:

```bash
src-tauri/sidecars/inference/scripts/run-latency-benchmark.sh --runs 10
```

The tester keeps one real NDJSON sidecar child alive. Its inference table
separates sidecar round-trip time, backend time, sidecar protocol/process
overhead, the concurrent completion batch, and normalization/ranking. A second
table breaks every completion into request construction, TCP connection,
request write, model server wait/time-to-first-byte, response read, HTTP
parsing, OpenAI-response decoding, and Quip-output decoding. Completion stages
are distributions over individual concurrent requests, so they do not add
linearly to inference time.

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

Compare local models by running the launcher once per model. A server started
by the script is stopped after each run:

```bash
QUIP_SERVER_MODEL=Qwen/Qwen3.5-2B \
  src-tauri/sidecars/inference/scripts/run-latency-benchmark.sh --runs 20

QUIP_SERVER_MODEL=other-org/other-model \
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

## Streaming, cancellation, and early exit

`LiveConfig.streaming` (`QUIP_STREAMING=true`, or `--streaming` on the
latency tester) switches each of the `completion_count` concurrent
completions from one buffered response to an incrementally-decoded SSE
stream (`stream: true` against `/v1/chat/completions`, handling both
`Transfer-Encoding: chunked` and plain `Connection: close` framing — the
server's actual choice is unconfirmed against a real mistral.rs instance,
which is why both are implemented and covered by mock-server tests in
`live.rs`). The live adapter launchers enable streaming by default. Set
`QUIP_STREAMING=false` to retain the complete n:5 path for an A/B comparison.

With streaming on, `run_streaming_batch` reads completions in the order they
actually finish rather than the order they were dispatched, and once
`early_exit_agreement` of them (`QUIP_EARLY_EXIT_AGREEMENT`, default 3, must
be between 1 and `completion_count`) land on the same suggestion, the rest
are cancelled — their sockets are shut down from the coordinator thread,
which unblocks their blocked `read`s (`CancelHandle` in `live.rs`). A
cancelled completion is not a failure: `normalize_model_outputs` only fails
the batch on a genuine transport/parse error, distinct from an intentional
cancellation.

A/B the two against each other with the latency tester:

```bash
target/debug/quip-latency-tester --address 127.0.0.1:1234 --runs 20 --json > batched.json
target/debug/quip-latency-tester --address 127.0.0.1:1234 --runs 20 --streaming --json > streaming.json
```

Streaming's benefit depends on whether early-exit agreement typically lands
before all five completions finish. The three-model comparison command measures
that optimized path on the current Mac.

`model_input` (the JSON body of the user chat message) also orders
`context_snippets`/`personal_patterns` before `text`: those two fields stay
constant for a whole composition session while `text` (the sliding-window
draft) changes on every burst, so this ordering maximizes the literal byte
prefix consecutive sliding-window requests share — the lever a server-side
prefix cache would key on. Whether it measurably helps still needs the
5/10/15-word latency matrix above, run on both Macs, before it's trusted as
more than a plausible mechanism.

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
prediction requests. Select the model with `QUIP_GLOBAL_MODEL_PRESET=0.8b`,
`2b`, or `4b`; set `QUIP_DEMO_WARMUP_RUNS=0` only when intentionally measuring
a cold launch.
