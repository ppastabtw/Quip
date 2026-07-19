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
      /** Zero through five unique model replacements, best first. */
      candidates: string[];
      recommended: number;
      /** Highlighted candidate; arrows move it, Tab accepts it. */
      selected: number;
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
  /** Burst window in words; 5 until a retrained model raises it. */
  window_words: number;
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

/** One word-slot proposal from the engine's session edit accumulator, in
 * session word coordinates. */
export interface Mark {
  start_word: number;
  /** Draft words replaced (0 = insertion before start_word). */
  word_len: number;
  original: string;
  replacement: string;
  agreements: number;
  /** Hardened: stable enough to show and apply. */
  stable: boolean;
}

export interface DebugEventView {
  ts_ms: number;
  event: string;
  summary: string;
  payload: Record<string, unknown>;
}

export const api = {
  captureActiveDestination: (trigger: "idle" | "punctuation" | "return" | "shortcut") =>
    invoke<void>("capture_active_destination", { trigger }),
  injectCapture: (result: CaptureResult, barless?: boolean) =>
    invoke<void>("inject_capture", { result, barless }),
  selectCandidate: (index: number) => invoke<CommitOutcome>("select_candidate", { index }),
  moveSelection: (delta: number) => invoke<void>("move_selection", { delta }),
  dismissSuggestions: () => invoke<void>("dismiss_suggestions"),
  endSession: () => invoke<void>("end_composition_session"),
  retractOffer: (burstId: string) => invoke<void>("retract_offer", { burstId }),
  getMarks: () => invoke<Mark[]>("get_marks"),
  applyMarks: () => invoke<Mark[]>("apply_marks"),
  clearMarks: () => invoke<void>("clear_marks"),
  getCompositionState: () => invoke<Snapshot>("get_composition_state"),
  getSettings: () => invoke<AppSettings>("get_settings"),
  updateSettings: (settings: AppSettings) => invoke<void>("update_settings", { settings }),
  listProfiles: () => invoke<string[]>("list_profiles"),
  getPatterns: (profileId: string) =>
    invoke<PatternView[]>("get_patterns", { profileId }),
  resetProfile: (profileId: string) => invoke<void>("reset_profile", { profileId }),
  getHealth: () => invoke<SidecarHealth>("get_health"),
  getMetrics: () => invoke<Metrics>("get_metrics"),
  getDebugEvents: (limit: number) => invoke<DebugEventView[]>("get_debug_events", { limit }),
  recordDebugEvent: (event: string, summary: string, payload: Record<string, unknown>) =>
    invoke<void>("record_debug_event", { event, summary, payload }),
  setSimulateFailure: (on: boolean) => invoke<void>("set_simulate_failure", { on }),
  listCorpus: () => invoke<DemoCase[]>("list_corpus"),
  runComparison: (caseId: string) =>
    invoke<ComparisonReport>("run_comparison", { caseId }),
};

/** A burst's prediction settled (offered, skipped, or stale): the capture
 * side's serial request slot is free again. */
export interface SettledEvent {
  burst_id: string;
  offered: boolean;
}

export const events = {
  onSnapshot: (handler: (snapshot: Snapshot) => void) =>
    listen<Snapshot>("composition://state", (e) => handler(e.payload)),
  onSettled: (handler: (settled: SettledEvent) => void) =>
    listen<SettledEvent>("composition://settled", (e) => handler(e.payload)),
  onMarks: (handler: (marks: Mark[]) => void) =>
    listen<Mark[]>("composition://marks", (e) => handler(e.payload)),
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
