// Typed IPC surface between the webviews and the Rust engine.
// Contract shapes mirror crates/quip-contracts; engine shapes mirror
// src-tauri/src/composition (Snapshot) and friends.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  Backend,
  CaptureResult,
  ErrorInfo,
  ModelVariant,
  PredictionRequest,
  PredictionResult,
  Rect,
  SidecarHealth,
} from "./contracts";

export type Snapshot =
  | { phase: "idle" }
  | { phase: "predicting"; burst_id: string; draft: string; model_variant: ModelVariant }
  | {
      phase: "suggesting";
      burst_id: string;
      draft: string;
      /** One to three model replacements; empty only in the error state. */
      candidates: string[];
      recommended: number;
      caret: Rect;
      model_variant: ModelVariant;
      backend: Backend | null;
      latency_ms: number | null;
      error: ErrorInfo | null;
    }
  | { phase: "applied"; burst_id: string; destination_id: string; text: string }
  | { phase: "unavailable"; reason: string };

export interface AppSettings {
  enabled: boolean;
  window_context: boolean;
  learning_paused: boolean;
  active_profile: string;
  backend_mode: "fixture" | "live";
  model_variant: ModelVariant;
}

export interface Metrics {
  requests: number;
  ok: number;
  errors: number;
  schema_invalid: number;
  last_latency_ms: number | null;
  avg_latency_ms: number | null;
}

export interface PatternView {
  shorthand: string;
  expansion: string;
  count: number;
}

export interface SideSpec {
  label: string;
  model_variant: ModelVariant;
  profile_id: string;
  use_context: boolean;
}

export interface DemoCase {
  case_id: string;
  title: string;
  description: string;
  draft: string;
  context_snippets: { app_name: string; window_title: string; visible_text: string }[];
  left: SideSpec;
  right: SideSpec;
}

export interface ComparisonSide {
  spec: SideSpec;
  request: PredictionRequest;
  result: PredictionResult;
}

export interface ComparisonReport {
  case_id: string;
  title: string;
  description: string;
  draft: string;
  left: ComparisonSide;
  right: ComparisonSide;
}

export interface CommitOutcome {
  destination_id: string;
  text: string;
}

export const api = {
  injectCapture: (result: CaptureResult) => invoke<void>("inject_capture", { result }),
  selectCandidate: (index: number) => invoke<CommitOutcome>("select_candidate", { index }),
  dismissSuggestions: () => invoke<void>("dismiss_suggestions"),
  getCompositionState: () => invoke<Snapshot>("get_composition_state"),
  getSettings: () => invoke<AppSettings>("get_settings"),
  updateSettings: (settings: AppSettings) => invoke<void>("update_settings", { settings }),
  listProfiles: () => invoke<string[]>("list_profiles"),
  getPatterns: (profileId: string) =>
    invoke<PatternView[]>("get_patterns", { profileId }),
  resetProfile: (profileId: string) => invoke<void>("reset_profile", { profileId }),
  getHealth: () => invoke<SidecarHealth>("get_health"),
  getMetrics: () => invoke<Metrics>("get_metrics"),
  setSimulateFailure: (on: boolean) => invoke<void>("set_simulate_failure", { on }),
  listCorpus: () => invoke<DemoCase[]>("list_corpus"),
  runComparison: (caseId: string) =>
    invoke<ComparisonReport>("run_comparison", { caseId }),
};

export const events = {
  onSnapshot: (handler: (snapshot: Snapshot) => void) =>
    listen<Snapshot>("composition://state", (e) => handler(e.payload)),
  onCommitted: (handler: (outcome: CommitOutcome) => void) =>
    listen<CommitOutcome>("composition://committed", (e) => handler(e.payload)),
  onSettings: (handler: (settings: AppSettings) => void) =>
    listen<AppSettings>("settings://changed", (e) => handler(e.payload)),
  onMetrics: (handler: (metrics: Metrics) => void) =>
    listen<Metrics>("metrics://changed", (e) => handler(e.payload)),
};

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

export function byId<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error(`missing element #${id}`);
  return node as T;
}
