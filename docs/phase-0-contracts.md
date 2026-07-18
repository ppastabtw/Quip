# Phase 0 contracts

Source: `docs/SPEC.md`, `docs/technical-plan.md`, `docs/fixtures/phase-0-examples.json`

## TLDR

Phase 0 locks the shared boundary names so four people can build in parallel. Done means this doc and the fixture JSON are accepted by the team.

Person 4 owns final contract calls because they integrate everyone. Persons 1 to 3 own blocking objections for their workstreams.

## Ownership

| Person   | Owns objections for                | Blocks only if                                                              |
| -------- | ---------------------------------- | --------------------------------------------------------------------------- |
| Person 1 | Flash training and evaluation      | A field breaks training rows, JSON gold outputs, or held-out evaluation.    |
| Person 2 | Local inference sidecar            | A field breaks runtime API, fixture mode, adapter health, or errors.        |
| Person 3 | Accessibility capture and commit   | A field misrepresents capture, restore, secure fields, or window context.   |
| Person 4 | UI, learning, and demo integration | A field breaks candidate rendering, local examples, fixtures, or health UI. |

Tie-break rule: if it does not block a workstream, keep the contract stable.

## Vocabulary

| Term                     | Means                                                           |
| ------------------------ | --------------------------------------------------------------- |
| `composition_burst`      | Bounded user text captured before prediction.                   |
| `exact_draft_option`     | The unchanged user text option the UI always shows.             |
| `prediction_request`     | Request sent to local inference.                                |
| `prediction_result`      | Schema-validated model/runtime response.                        |
| `destination_snapshot`   | Preserved editable target state.                                |
| `window_context_snippet` | Bounded visible text from another window.                       |
| `candidate_state`        | UI state for exact draft, model options, selection, and commit. |
| `personal_example`       | Compact local personalization record.                           |
| `sidecar_health`         | Inference runtime readiness.                                    |

Avoid overloaded names like `context`, `payload`, `output`, `options`, `status`, and `feedback` unless paired with a domain noun.

## Enums

| Field                     | Values                                                                                             |
| ------------------------- | -------------------------------------------------------------------------------------------------- |
| `source_mode`             | `compose_before_commit`, `existing_text`                                                           |
| `trigger`                 | `idle`, `punctuation`, `return`, `shortcut`                                                        |
| `runtime_mode`            | `fixture`, `base_qwen`, `trained_adapter`, `personal_adapter`                                      |
| `action`                  | `keep`, `replace`                                                                                  |
| `commit_capability`       | `accessibility_insert`, `accessibility_replace`, `paste_fallback`, `unsupported`                   |
| `insertion_point_kind`    | `text_marker`, `range`, `unknown`                                                                  |
| `personal_example.source` | `confirmed_candidate`, `dismissed_stable_suggestion`, `post_commit_correction`, `repeated_pattern` |
| `candidate_state.status`  | `idle`, `capturing`, `predicting`, `ready`, `committing`, `unavailable`, `cancelled`               |

## Shapes

These are human-readable object shapes, not a TypeScript decision. The app can implement them in Rust, JavaScript, TypeScript, Python, or JSON Schema later.

### `composition_burst`

```ts
{
  burst_id: string;
  profile_id: string;
  source_mode: "compose_before_commit" | "existing_text";
  text: string;
  trigger: "idle" | "punctuation" | "return" | "shortcut";
  idle_ms: number | null;
  char_window_limit: number;
}
```

Rules: `text` is bounded draft/selection text. `char_window_limit` starts at `80`. `idle_ms` is only set for `idle`.

### `destination_snapshot`

```ts
{
  snapshot_id: string;
  app_name: string;
  bundle_id: string;
  element_role: string;
  selection_text: string;
  insertion_point_kind: "text_marker" | "range" | "unknown";
  commit_capability: "accessibility_insert" |
    "accessibility_replace" |
    "paste_fallback" |
    "unsupported";
  clipboard_restore_required: boolean;
  secure_field: boolean;
}
```

Rules: if `secure_field` is `true`, do not predict. If `commit_capability` is `unsupported`, do not commit. Clipboard restore is only for paste fallback.

### `window_context_snippet`

```ts
{
  snippet_id: string;
  app_name: string;
  window_title: string;
  visible_text: string;
  rank_reason: string;
  max_chars: number;
}
```

Rules: accessible text only. Bounded. No secure fields or excluded apps.

### `prediction_request`

```ts
{
  request_id: string;
  profile_id: string;
  runtime_mode: "fixture" | "base_qwen" | "trained_adapter" | "personal_adapter";
  composition_burst: {
    text: string;
    char_window_limit: number;
  };
  window_context_snippets: window_context_snippet[];
  personal_patterns: Array<{
    shorthand: string;
    expansion: string;
  }>;
}
```

Rules: send bounded text, not the whole destination. Context snippets and personal patterns can be empty.

### `prediction_result`

```ts
{
  request_id: string;
  runtime_mode: "fixture" | "base_qwen" | "trained_adapter" | "personal_adapter";
  schema_valid: boolean;
  action: "keep" | "replace";
  candidates: string[];
  latency_ms: number;
}
```

Rules: `keep` has zero candidates. `replace` has one to three full-input replacement candidates. The app adds `exact_draft_option`; the model does not return it.

### `candidate_state`

```ts
{
  burst_id: string;
  status: "idle" |
    "capturing" |
    "predicting" |
    "ready" |
    "committing" |
    "unavailable" |
    "cancelled";
  exact_draft_option: {
    option_id: "option_exact";
    text: string;
  }
  model_options: Array<{
    option_id: string;
    text: string;
    source: "fixture" | "base_qwen" | "trained_adapter" | "personal_adapter";
  }>;
  selected_option_id: string | null;
  commit_enabled: boolean;
}
```

Rules: `exact_draft_option` is always present after capture. `model_options` can be empty.

### `personal_example`

```ts
{
  example_id: string;
  profile_id: string;
  source: "confirmed_candidate" |
    "dismissed_stable_suggestion" |
    "post_commit_correction" |
    "repeated_pattern";
  input_text: string;
  confirmed_text: string;
  created_at: string;
}
```

Rules: store useful labeled events, not every keystroke. Use ISO-8601 with offset for now.

### `sidecar_health`

```ts
{
  sidecar_ready: boolean;
  base_model_loaded: boolean;
  global_adapter_loaded: boolean;
  user_adapter_loaded: boolean;
  fixture_mode: boolean;
  model_family: string;
}
```

Rules: must be visible before demo. `fixture_mode` means examples can come from the fixture JSON.

## Fixture scenarios

`docs/fixtures/phase-0-examples.json` is the source of truth.

| Scenario                   | Proves                                         |
| -------------------------- | ---------------------------------------------- |
| `shorthand_replace`        | `cnt cm tmrw` becomes `Can't come tomorrow.`   |
| `typo_replace`             | `instaed` becomes `instead`.                   |
| `protected_keep`           | `usr/bin` and `q3_finl_v2.pdf` stay unchanged. |
| `context_replace`          | Window context can resolve ambiguous text.     |
| `secure_field_unavailable` | Secure fields do not predict or commit.        |
| `personalized_replace`     | Profile patterns can change candidates.        |

## Smoke checks

Before workstreams split:

- Person 1 can map fixtures into Flash rows with `input`, optional `output`, and optional `metadata`.
- Person 2 can serve `prediction_request`, `prediction_result`, and `sidecar_health` in fixture mode.
- Person 3 can emit `composition_burst`, `destination_snapshot`, and `window_context_snippet` for TextEdit and one browser.
- Person 4 can render `candidate_state`, including exact draft, empty model options, unavailable state, and personal examples.

Before integration:

- Render `shorthand_replace` with no live inference.
- Render `protected_keep` with exact draft and no model options.
- Render `secure_field_unavailable` with commit disabled.
- Show sidecar health in fixture mode.
- Walk through commit state without touching destination text.

## Change process

- New field: add one fixture example.
- Renamed field: all affected owners confirm.
- Merely imperfect field: keep it.
- Blocking field: Person 4 decides after hearing the owner objection.
