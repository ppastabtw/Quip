# Person 2 plan: local inference, adapters, and packaging

## Mission

Person 2 owns Quip's local inference runtime. The workstream is complete when
the application can send a `PredictionRequest` to a reliable local backend and
receive a schema-valid `PredictionResult`, while the health UI receives an
accurate `SidecarHealth` report.

The implementation must preserve Quip's locality contract: drafts, context,
personal patterns, model output, adapters, and personal training records stay
on the Mac. Model binaries, adapters, personal records, secrets, and generated
logs must not be committed.

`docs/SPEC.md` governs product behavior. `docs/phase-0.schema.json` and
`docs/phase-0-contracts.md` govern the provisional v0 boundary.

## Ownership boundaries

Person 2 owns:

- the local inference sidecar and its process lifecycle;
- deterministic fixture and live inference backends;
- Qwen loading, Metal acceleration, and quantization;
- model prompt rendering, guided JSON decoding, and result validation;
- global and per-user adapter loading and composition;
- local model and adapter artifact discovery;
- latency, memory, error, and sidecar-health reporting;
- Tauri sidecar packaging; and
- the feasibility proof for local per-user training and its fallback.

Person 2 does not own:

- global adapter training and held-out evaluation, which belong to Person 1;
- Accessibility capture and window-context collection, which belong to Person
  3; or
- request construction, personal-pattern storage, exact-draft display, and the
  composition UI, which belong to Person 4.

Person 4 constructs the shared `PredictionRequest`. Person 2 converts that
request into model messages, runs inference, validates the model output, and
returns the shared `PredictionResult`.

## Delivery strategy

Maintain two working lanes throughout development:

1. **Demo-safe lane:** deterministic fixture inference that is always usable.
2. **Live-model lane:** Qwen, Metal, adapter, and personalization experiments.

The fixture lane is the first integration target and remains the rollback path
after live inference is added. A failed model experiment must never prevent the
rest of the application from using fixture mode.

## Milestones

| Milestone | Deliverable | Exit condition |
| --- | --- | --- |
| 1. Contract handshake | Fixture backend and health responses | All shared prediction and health fixtures pass through a producer and consumer |
| 2. Sidecar service | Local process with predict and health operations | Person 4 can call the sidecar without a model installed |
| 3. Base inference | Qwen3.5-2B with 4-bit Metal inference | One live request returns a valid result with measured latency |
| 4. Global adapter | Freesolo adapter supplied by Person 1 | Base and global variants run on the same input |
| 5. Personalization | Personal patterns and optional user adapter | Two profiles produce meaningfully different output |
| 6. Packaging | Tauri launches and supervises the sidecar | A packaged build works on both target Macs |
| 7. Hardening | Benchmarks, explicit failures, and fallback | The end-to-end demo survives missing artifacts and sidecar restart |

## Milestone 1: contract-backed fixture inference

Implement the boundary defined by:

- `crates/quip-contracts`;
- `docs/phase-0.schema.json`;
- `docs/phase-0-contracts.md`; and
- `docs/fixtures/phase-0-examples.json`.

Required behavior:

- accept the exact `PredictionRequest` type;
- return the exact `PredictionResult` type;
- echo the incoming `request_id` and `model_variant`;
- call `PredictionResult::validate()` before returning a result;
- support every shared prediction fixture;
- report ready and degraded `SidecarHealth` fixtures;
- reproduce the `adapter_not_loaded` failure; and
- never add the exact draft as a candidate.

Fixture lookup should match the semantic request fields while allowing the
caller to generate a fresh `request_id`. The fixture result must be rewritten
to echo that incoming identifier.

The first pull request should contain fixture inference, contract tests, and
health reporting without depending on `mistral.rs`.

## Milestone 2: sidecar skeleton

Turn `src-tauri/sidecars/inference/` into an executable Rust package. A likely
structure is:

```text
src-tauri/sidecars/inference/
├── Cargo.toml
├── src/
│   ├── main.rs
│   ├── api.rs
│   ├── artifacts.rs
│   ├── health.rs
│   ├── prompt.rs
│   └── backend/
│       ├── mod.rs
│       ├── fixture.rs
│       └── live.rs
└── tests/
    ├── fixture_contract.rs
    └── health.rs
```

The sidecar exposes two internal operations:

- prediction: `PredictionRequest -> PredictionResult`;
- health: `() -> SidecarHealth`.

Transport details remain internal to the workstreams. If a loopback server is
used, it must bind only to `127.0.0.1`. Request bodies must be bounded and raw
drafts, context snippets, personal patterns, and generated text must not be
logged.

Coordinate the handshake with the app-side client in
`src-tauri/src/inference/`. Person 4 owns request construction and application
state; Person 2 owns the live transport, runtime lifecycle, and response
validation on the producer side.

## Milestone 3: base-model proof

Prove the model path independently before coupling it to application events:

1. Load Qwen3.5-2B locally.
2. Use 4-bit quantization and Metal acceleration.
3. Run the model in non-thinking mode.
4. Constrain generation to the model-output schema.
5. Generate one best replacement candidate by default.
6. Parse and validate the generated object.
7. Verify that prefix caching and guided JSON decoding work together.
8. Record cold-start time, warm latency, memory use, and schema failures.

The model generates only one of these shapes:

```json
{ "action": "keep", "candidates": [] }
```

```json
{ "action": "replace", "candidates": ["full-input replacement"] }
```

The sidecar, not the model, supplies `request_id`, `model_variant`,
`backend: "live"`, `latency_ms`, and explicit error information.

Although the shared contract permits one to three candidates, the normal
runtime policy is to request one best candidate. This reduces output tokens
without changing the Phase 0 schema. Lazy generation of additional candidates
is deferred until the one-candidate flow is measured and integrated.

Order model input with the stable system and output-contract prefix first,
followed by personal patterns, bounded window context, and the draft. Benchmark
prefix caching disabled, at the runtime default, and at one tuned setting. The
selected configuration must preserve guided-JSON correctness; cache reuse is
not accepted if it increases schema errors or returns stale content.

Do not test Qwen3.5-4B until the 2B path is stable. Adopt 4B only if a measured
quality improvement justifies its latency and memory on both target machines.

## Milestone 4: global Freesolo adapter

Person 1 must hand off:

- the base model identifier and immutable revision;
- the adapter repository and immutable revision;
- LoRA rank and target modules;
- tokenizer and chat-template assumptions;
- relevant held-out examples and category-level results; and
- an artifact checksum.

Person 2 then proves that:

- the global adapter loads on Metal;
- `base` and `global` variants run against the same request;
- the global variant improves a real shorthand or phonetic example;
- protected content is not unnecessarily changed;
- the downloaded artifact matches its recorded revision and checksum; and
- a missing or incompatible adapter produces degraded health instead of a
  crash.

Benchmark the global adapter in both unmerged and merged configurations. Treat
merging as a normal performance option rather than only a failure fallback.
Select the configuration using correctness, warm latency, memory, startup time,
and operational reliability.

Benchmark the live base-versus-global comparison sequentially and concurrently
on the target hardware. Record total comparison time, time to first result,
peak unified memory, and thermal behavior. Concurrency is adopted only if it
improves the demo without destabilizing Metal inference or duplicating too much
model state.

If `mistral.rs` cannot load the exported adapter, replace the internal local
runtime without changing the Phase 0 request, result, or health contracts.

## Milestone 5: personalization

Implement personalization in this order:

1. Inject the request's `personal_patterns` into the model prompt.
2. Resolve profile-specific artifact locations.
3. Load a pre-trained user adapter when one exists.
4. Benchmark stacked and merged global-plus-user configurations.
5. Prototype local user-adapter training only after inference is stable.

Use this fallback ladder:

- choose a merged global model plus one user adapter when it is faster or more
  reliable, even if stacking is technically functional;
- if local training is too slow or unstable, ship pre-trained local user
  adapters;
- if user adapters remain unavailable, use the local pattern dictionary; and
- never allow personalization failure to block base or global inference.

The judged path only needs two local profiles to produce different candidates.
Automatic idle-time retraining is a stretch goal.

## Milestone 6: Tauri packaging

Person 2 supplies a buildable sidecar binary and an internal artifact manifest.
Person 4 wires the lifecycle into the Tauri application.

Required packaging work:

- add the sidecar to Tauri's `bundle.externalBin` configuration;
- produce an Apple Silicon binary with the required target-triple suffix;
- grant only the minimum sidecar-spawn permission;
- start the sidecar during application initialization;
- wait for health before enabling live inference;
- restart it after an unexpected exit;
- terminate it when Quip exits; and
- keep actual model and adapter files outside Git.

Development artifacts may live under `artifacts/models/`. A packaged build
should use a Quip-specific Application Support directory. Artifact paths,
runtime tuning values, and adapter metadata remain internal rather than being
added to the shared Phase 0 schema.

## Performance benchmark matrix

Run the following matrix after base inference works and repeat the relevant
rows after the global adapter arrives:

| Variable | Cases |
| --- | --- |
| Result type | `keep`, `replace` |
| Candidate count | 1 default, 3 comparison-only |
| Prompt content | draft only, bounded context, personal patterns |
| Prefix cache | disabled, runtime default, tuned |
| Model variant | base, global |
| Adapter layout | stacked, merged |
| Demo comparison | sequential, concurrent |
| Hardware | M3 Pro, M4 Air |

For every case record prefill time, decode tokens per second, total latency,
time to first token or first result, output-token count, peak unified memory,
schema validity, and any thermal degradation. Performance estimates are not
accepted as completion evidence; the decision log must contain measurements
from the actual Quip prompt and artifacts.

## Decision gates and fallbacks

### Gate 1: Qwen3.5-2B on Metal

- **Pass:** continue with the selected `mistral.rs` integration.
- **Fail:** switch to another replaceable local runtime behind the same
  sidecar contract.

### Gate 2: global adapter loading

- **Pass:** proceed to base-versus-global comparison.
- **Fail:** test a compatible export or local runtime; do not change the app
  boundary.

### Gate 3: global and user adapter composition

- **Stacked wins:** keep the global and user adapters separate by profile.
- **Merged wins:** merge the global adapter into the base and load one user
  adapter, even if stacking also works.
- **Both fail:** use personal patterns or a pre-trained compatible artifact
  without blocking base or global inference.

### Gate 4: local per-user training

- **Pass:** refresh the user adapter during safe idle periods.
- **Fail:** ship pre-trained user adapters plus the local pattern dictionary.

### Gate 5: Qwen3.5-4B

- **Pass:** adopt it only if measured quality gains outweigh added latency and
  memory.
- **Fail:** keep Qwen3.5-2B as the judged model.

## Integration checkpoints

1. **Contract handoff:** Person 4's client successfully uses fixture mode and
   reads health.
2. **Live handoff:** base inference replaces fixture inference without changing
   UI or orchestration logic.
3. **Adapter handoff:** Person 1's exported global adapter runs locally against
   the shared demo inputs, with merged-versus-stacked and
   sequential-versus-concurrent measurements recorded.
4. **Personalization handoff:** Person 4's profile and pattern state produce two
   measurably different local results.
5. **Demo handoff:** base/global comparison, protected text, context,
   personalization, latency, health, and fixture fallback work from the Tauri
   demo harness.

## Definition of done

Person 2's workstream is complete when:

- every prediction fixture passes through an actual producer and consumer;
- base Qwen runs locally and remains usable offline after installation;
- the global adapter runs against the same request as the base model;
- every result satisfies the shared candidate-count invariants;
- live replacement inference returns one candidate by default;
- prefix caching has been measured with guided JSON enabled;
- context snippets and personal patterns reach the model correctly;
- two local profiles can produce different results;
- merged and stacked adapter configurations have been compared using real
  measurements;
- sequential and concurrent base-versus-global execution have been compared on
  the target hardware;
- missing artifacts return explicit errors and accurate degraded health;
- Tauri can start, query, restart, and stop the sidecar;
- latency and memory results are recorded for the M3 Pro and M4 Air;
- fixture mode remains a one-step rollback; and
- no model binaries, adapters, personal data, prompts, secrets, or generated
  logs are committed.
