// TypeScript mirrors of the Phase 0 wire contracts.
// `docs/phase-0.schema.json` is authoritative; keep in sync with the Rust
// types in `crates/quip-contracts`.

export type ModelVariant = "base" | "global" | "global_plus_personal";

export type Backend = "fixture" | "live";

export interface ErrorInfo {
  code: string;
  message: string;
  retryable: boolean;
}

export interface ContextSnippet {
  app_name: string;
  window_title: string;
  visible_text: string;
}

export interface PersonalPattern {
  shorthand: string;
  expansion: string;
}

export interface PredictionRequest {
  request_id: string;
  profile_id: string;
  model_variant: ModelVariant;
  draft: string;
  context_snippets: ContextSnippet[];
  personal_patterns: PersonalPattern[];
}

export type PredictionResult =
  | {
      status: "ok";
      request_id: string;
      model_variant: ModelVariant;
      backend: Backend;
      /** Zero through five unique full-input replacements, best first. */
      candidates: string[];
      latency_ms: number;
    }
  | {
      status: "error";
      request_id: string;
      model_variant: ModelVariant;
      error: ErrorInfo;
    };

export type Trigger = "idle" | "punctuation" | "return" | "shortcut";

/** Logical screen coordinates, origin top-left. */
export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type CaptureResult =
  | {
      status: "ready";
      burst_id: string;
      /** Opaque: return it on commit without interpreting it. */
      destination_id: string;
      profile_id: string;
      draft: string;
      trigger: Trigger;
      /** Caret rectangle; the suggestion bar is anchored above it. */
      caret: Rect;
    }
  | {
      status: "unavailable";
      reason: string;
    };

export type HealthStatus = "ready" | "degraded" | "unavailable";

export interface SidecarHealth {
  status: HealthStatus;
  fixture_available: boolean;
  loaded: {
    base: boolean;
    global_adapter: boolean;
    user_adapter: boolean;
  };
  error?: ErrorInfo;
}
