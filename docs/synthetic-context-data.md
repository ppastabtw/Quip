# Synthetic context training data

Quip's synthetic-context pipeline creates a separate training supplement that
teaches the correction model when context should change an answer and, equally
importantly, when it should be ignored. The checked-in default generates 200
candidates and selects the best 100 passing examples.

## Flow

```text
deterministic balanced slots and contrast groups
  -> Backboard generator (structured batches)
  -> deterministic schema/privacy/length/group validation
  -> independent Backboard judge (structured scores)
  -> score thresholds plus exact/normalized/near deduplication
  -> best 100 under behavior/category balance
  -> training-ready train.jsonl
```

The active model replies with plain corrected text. Canonical training rows
still store the gold target as an object:

```json
{
  "input": {
    "text": "meet there tmrw",
    "context_snippets": [
      {
        "app_name": "Notes",
        "window_title": "Trip planning",
        "visible_text": "Tomorrow: meet at Union Station."
      }
    ]
  },
  "output": {"suggestion": "meet at Union Station tomorrow"},
  "metadata": {"...": "auditable provenance and judge scores"}
}
```

This structure loads through the existing Freesolo environment. The original
MASSIVE train/eval/test files are never modified.

## Three semantic outcomes

1. Normal correction fixes only what the draft itself justifies, such as
   `tmrw` to `tomorrow`.
2. Justified contextual clarification uses direct evidence to resolve an
   otherwise vague, phonetic, misspelled, or ambiguous draft. For example,
   clear trip context can resolve `there` to `Union Station`.
3. Unjustified context injection copies a nearby detail that is irrelevant,
   stale, contradicted, or one of several plausible choices. Those candidates
   must be rejected. Negative examples deliberately teach the model to ignore
   such context and trust explicit user updates.

## Configuration

The default configuration is
`training/flash/configs/synthetic-context-v1.toml`. It controls:

- final target count and 2x candidate pool;
- generator/judge batch sizes, temperature, token caps, concurrency, retry
  attempts, request rate, and timeout;
- deterministic seed, one-to-five-word draft bounds, context bounds, and
  near-duplicate threshold;
- context behavior and semantic category weights;
- judge score and dataset-value thresholds;
- contrast-group share.

Context behavior defaults are 45% useful, 17.5% irrelevant, 12.5% ambiguous,
12.5% conflicting/stale, and 12.5% no-context controls. The final selector
ranks by the minimum judge dimension, total judge score, dataset-value score,
and context-grounding score within exact behavior/category balance constraints,
then favors underrepresented writing styles and error mechanisms available in
the accepted pool. Complete passing
contrast groups are selected first. If a judge rejects one or more siblings,
individually valid variants may fill the remaining rows rather than discarding
good examples solely because a sibling failed. This prevents a high-scoring but
behaviorally narrow pool from dominating the final 100.

Generator and judge prompts are versioned separately under
`training/flash/synthetic_data/prompts/`. The generator sees an exact slot
matrix rather than inventing its own distribution. Contrast group members are
kept in one generation batch so they share a coherent base situation.

## Credentials and model selection

Set the existing repository credential only in the process environment or the
ignored root `.env` file:

```bash
export BACKBOARD_API_KEY="..."
```

Never put the key in a command, config, prompt, log, output artifact, or commit.
Backboard model selection uses the provider/model slug shown by its model
catalog. Generator and judge may use different models:

```bash
--generator-model provider/generator-model \
--judge-model provider/judge-model
```

Before sending requests, the pipeline checks that both models are present in
Backboard's JSON-output-capable catalog. Backboard memory and web search are
disabled. Usage tokens, catalog prices when available, estimated cost,
latency, retries, and failures are written to the run summary without exposing
the credential.

## Commands

Run these from `training/flash`. A plan makes no network request and does not
need a credential:

```bash
.venv/bin/python scripts/generate_synthetic_data.py plan
```

Run a small paid-API-free workflow smoke test:

```bash
.venv/bin/python scripts/generate_synthetic_data.py run \
  --mock \
  --count 10 \
  --run-id mock-smoke
```

Mock records validate the workflow only; their rationale marks them as mock
fixtures, and they are not model-quality evidence.

Run the complete 200-candidate to 100-row Backboard job:

```bash
.venv/bin/python scripts/generate_synthetic_data.py run \
  --run-id context-v1 \
  --generator-model provider/generator-model \
  --judge-model provider/judge-model
```

Limit a test to selected categories by repeating `--category`:

```bash
.venv/bin/python scripts/generate_synthetic_data.py run \
  --mock --count 12 --run-id entity-smoke \
  --category entity_spelling \
  --category phonetic_resolution
```

When generating a dataset that will be combined with an earlier run, pass the
earlier training JSONL as a diversity reference:

```bash
.venv/bin/python scripts/generate_synthetic_data.py run \
  --count 200 --run-id context-v2 \
  --candidate-count 300 \
  --generator-model openrouter/z-ai/glm-5.2 \
  --judge-model openrouter/openai/gpt-5.6-luna \
  --avoid-dataset ../../artifacts/synthetic/context-v1/train.jsonl
```

The generator receives prior coverage counts plus a rotating sample of earlier
draft/target pairs and is told to broaden rather than lightly mutate them. The
deduplicator also seeds itself with every reference row, so exact, normalized,
and near matches against the earlier dataset are rejected with explicit
`reference_*_duplicate` reasons. Reference file paths and hashes are included
in the immutable run manifest.

`--candidate-count` is an operational cap separate from the final `--count`.
It can safely reduce a checkpointed run: the scheduler retains the original
deterministic slot plan, skips completed slot IDs, and requests only enough
remaining slots to reach the new pool size. The cap is recorded in state and
summary artifacts.

Stages can be resumed independently. Reuse the same run ID, output directory,
configuration, count, category filters, and model overrides:

```bash
.venv/bin/python scripts/generate_synthetic_data.py generate ...
.venv/bin/python scripts/generate_synthetic_data.py judge ...
.venv/bin/python scripts/generate_synthetic_data.py build-dataset ...
```

Raw candidates and completed judgments are indexed on reload, so already
finished slots and judge calls are skipped. A configuration fingerprint in
`manifest.json` prevents accidental resume with incompatible settings.

## Run artifacts

Default output is `artifacts/synthetic/<run-id>/`, which is ignored by Git.

| File | Purpose |
| --- | --- |
| `manifest.json` | immutable run/config/prompt identity |
| `state.json` | atomic resume checkpoint and counts |
| `raw_responses.jsonl` | generator/judge responses, usage, latency, and parse failures |
| `raw_candidates.jsonl` | parsed generator candidates with full provenance |
| `local_validation.jsonl` | pre-judge deterministic pass/failure reasons |
| `judge_results.jsonl` | strict dimension scores and judge provenance |
| `judge_failures.jsonl` | exhausted judge-call failures, if any |
| `accepted_examples.jsonl` | passing, deduplicated audit records |
| `rejected_examples.jsonl` | rejected records with stage and reasons |
| `train.jsonl` | final 100 training-ready rows |
| `summary.json` | dataset SHA-256 plus acceptance, failure, cost, token, category, behavior, app, domain, error, and style statistics |

Inspect `rejected_examples.jsonl` before using a dataset. A missing judge result,
unsupported context change, low score, malformed group, or duplicate can never
enter `train.jsonl`.
