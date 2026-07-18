// Demo harness (Workstream 4): the typing playground, manual focused-capture
// integration, deterministic corpus comparison, sidecar health and
// schema-validity counters, and scripted capture_result drivers.

import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  api,
  byId,
  el,
  events,
  type AppSettings,
  type ComparisonReport,
  type ComparisonSide,
  type DebugEventView,
  type Metrics,
  type Snapshot,
} from "./ipc";
import type { PredictionResult, Rect, SidecarHealth, Trigger } from "./contracts";

const healthStatusEl = byId<HTMLSpanElement>("health_status");
const backendModeEl = byId<HTMLSpanElement>("backend_mode");
const healthLoadedEl = byId<HTMLSpanElement>("health_loaded");
const metricsEl = byId<HTMLSpanElement>("metrics");
const failureEl = byId<HTMLInputElement>("simulate_failure");
const casesEl = byId<HTMLDivElement>("cases");
const comparisonEl = byId<HTMLDivElement>("comparison");
const lastStateEl = byId<HTMLSpanElement>("last_state");
const lastCommitEl = byId<HTMLParagraphElement>("last_commit");
const playgroundEl = byId<HTMLTextAreaElement>("playground");
const debugTimelineEl = byId<HTMLDivElement>("debug_timeline");
const inlineCandidatesEl = byId<HTMLDivElement>("inline_candidates");

// ---- playground: burst tracking, triggers, caret geometry ----

// IME model (macOS Pinyin): predictions run continuously while the burst
// grows, debounced only enough to avoid churn; the bar refreshes in place.
const LIVE_DEBOUNCE_MS = 150;
const DRAFT_WINDOW_CHARS = 80;

let settings: AppSettings | undefined;
let burstStart = 0;
let burstEnd = 0;
let burstSeq = 0;
let activeBurstId: string | undefined;
let suggesting = false;
let selectedIndex = 0;
let idleTimer: number | undefined;
let debugRows: DebugEventView[] = [];

const measureCanvas = document.createElement("canvas").getContext("2d")!;

async function selectCandidate(index: number) {
  try {
    await api.selectCandidate(index);
  } catch (error) {
    const summary = `commit failed: ${String(error)}`;
    lastStateEl.textContent = summary;
    lastCommitEl.textContent = "";
    recordTimelineEvent("commit_failed", summary, {
      selected_index: index,
      success: false,
      error: String(error),
    });
  }
}

function appendTimelineEvent(
  event: string,
  summary: string,
  payload: Record<string, unknown> = {},
) {
  debugRows.push({
    ts_ms: Date.now(),
    event,
    summary,
    payload,
  });
  if (debugRows.length > 50) debugRows = debugRows.slice(-50);
  renderTimeline();
}

function recordTimelineEvent(
  event: string,
  summary: string,
  payload: Record<string, unknown> = {},
) {
  appendTimelineEvent(event, summary, payload);
  void api.recordDebugEvent(event, summary, payload).catch(() => {});
}

function mergeDebugEvents(eventsFromSink: DebugEventView[]) {
  const seen = new Set(debugRows.map((row) => `${row.ts_ms}:${row.event}:${row.summary}`));
  for (const event of eventsFromSink) {
    const key = `${event.ts_ms}:${event.event}:${event.summary}`;
    if (!seen.has(key)) {
      debugRows.push(event);
      seen.add(key);
    }
  }
  debugRows.sort((a, b) => a.ts_ms - b.ts_ms);
  if (debugRows.length > 50) debugRows = debugRows.slice(-50);
  renderTimeline();
}

function renderTimeline() {
  debugTimelineEl.replaceChildren();
  if (debugRows.length === 0) {
    debugTimelineEl.append(el("div", "placeholder", "No debug events"));
    return;
  }
  for (const event of debugRows.slice().reverse()) {
    const row = el("div", "debug-row");
    row.append(
      el("span", "debug-time", new Date(event.ts_ms).toLocaleTimeString()),
      el("span", "debug-summary", `${event.event}: ${event.summary}`),
    );
    debugTimelineEl.append(row);
  }
}

function snapshotSummary(snapshot: Snapshot): string {
  switch (snapshot.phase) {
    case "predicting":
      return `${snapshot.burst_id} draft_chars=${snapshot.draft.length}`;
    case "suggesting":
      return (
        `${snapshot.candidates.length} candidates` +
        (snapshot.backend ? ` ${snapshot.backend}` : "") +
        (snapshot.latency_ms === null ? "" : ` ${snapshot.latency_ms}ms`) +
        ` caret=${Math.round(snapshot.caret.x)},${Math.round(snapshot.caret.y)}`
      );
    case "unavailable":
      return snapshot.reason;
    case "applied":
      return `${snapshot.destination_id} chars=${snapshot.text.length}`;
    case "idle":
      return "hidden/idle";
  }
}

function renderInlineCandidates(snapshot: Snapshot) {
  inlineCandidatesEl.replaceChildren();
  if (snapshot.phase === "unavailable") {
    inlineCandidatesEl.classList.remove("placeholder");
    inlineCandidatesEl.append(
      el("div", "inline-candidate error-text", `unavailable: ${snapshot.reason}`),
    );
    return;
  }
  if (snapshot.phase !== "suggesting") {
    inlineCandidatesEl.classList.add("placeholder");
    inlineCandidatesEl.textContent = "No candidates";
    return;
  }
  inlineCandidatesEl.classList.remove("placeholder");
  if (snapshot.error) {
    inlineCandidatesEl.append(el("div", "inline-candidate", `error: ${snapshot.error.code}`));
    return;
  }
  if (snapshot.candidates.length === 0) {
    inlineCandidatesEl.classList.add("placeholder");
    inlineCandidatesEl.textContent = "No candidates";
    return;
  }
  snapshot.candidates.forEach((candidate, index) => {
    inlineCandidatesEl.append(el("div", "inline-candidate", `${index + 1}. ${candidate}`));
  });
}

function resetPlayground(reason: string, caret: number) {
  window.clearTimeout(idleTimer);
  burstStart = caret;
  burstEnd = caret;
  activeBurstId = undefined;
  suggesting = false;
  selectedIndex = 0;
  recordTimelineEvent("playground_reset", reason, {
    reason,
    caret,
    value_chars: playgroundEl.value.length,
  });
}

function caretClientPoint(): { x: number; y: number } {
  const style = getComputedStyle(playgroundEl);
  measureCanvas.font = `${style.fontSize} ${style.fontFamily}`;
  const charWidth = measureCanvas.measureText("M").width;
  const lineHeight = parseFloat(style.lineHeight);
  const before = playgroundEl.value.slice(0, playgroundEl.selectionStart);
  const lastBreak = before.lastIndexOf("\n");
  const row = before.split("\n").length - 1;
  const col = before.length - lastBreak - 1;
  const rect = playgroundEl.getBoundingClientRect();
  return {
    x: rect.left + parseFloat(style.paddingLeft) + col * charWidth - playgroundEl.scrollLeft,
    y: rect.top + parseFloat(style.paddingTop) + row * lineHeight - playgroundEl.scrollTop,
  };
}

async function caretScreenRect(): Promise<Rect> {
  const appWindow = getCurrentWindow();
  const [position, scale] = await Promise.all([
    appWindow.innerPosition(),
    appWindow.scaleFactor(),
  ]);
  const point = caretClientPoint();
  const style = getComputedStyle(playgroundEl);
  return {
    x: position.x / scale + point.x,
    y: position.y / scale + point.y,
    width: 2,
    height: parseFloat(style.lineHeight),
  };
}

async function fireTrigger(trigger: Trigger) {
  window.clearTimeout(idleTimer);
  burstEnd = playgroundEl.selectionStart;
  if (playgroundEl.value.length === 0 || burstEnd < burstStart) {
    resetPlayground("cleared_or_caret_before_burst", burstEnd);
    return;
  }
  const draft = playgroundEl.value.slice(burstStart, burstEnd).trim();
  if (draft.length === 0) return;
  recordTimelineEvent("playground_input", `draft_chars=${draft.length}`, {
    source: "playground",
    trigger,
    draft_chars: draft.length,
  });
  activeBurstId = `pg_${++burstSeq}`;
  void api.injectCapture({
    status: "ready",
    burst_id: activeBurstId,
    destination_id: "destination_playground",
    profile_id: settings?.active_profile ?? "profile_default",
    draft,
    trigger,
    caret: await caretScreenRect(),
  });
}

// Ends the composition session at the current caret: visible suggestions
/// become a stable dismissal (a keep label), and the next keystroke starts a
/// fresh burst.
function endSession() {
  window.clearTimeout(idleTimer);
  if (suggesting) void api.dismissSuggestions();
  burstStart = playgroundEl.selectionStart;
  burstEnd = burstStart;
}

playgroundEl.addEventListener("input", () => {
  const caret = playgroundEl.selectionStart;
  if (playgroundEl.value.length === 0 || caret < burstStart) {
    resetPlayground("cleared_or_caret_before_burst", caret);
    return;
  }
  const draft = playgroundEl.value.slice(burstStart, caret);
  const last = draft.at(-1) ?? "";
  // Sentence boundary ends the session: a newline, or whitespace after a
  // terminator (the burst before it was already predicted on).
  if (last === "\n" || (/\s/.test(last) && /[.!?]$/.test(draft.trimEnd()))) {
    endSession();
    return;
  }
  if (draft.trim().length === 0) {
    resetPlayground("empty_draft", caret);
    return;
  }
  // Continuous prediction while the burst grows, like an IME: punctuation
  // and the draft window fire immediately, everything else on a short
  // debounce. Stale results are dropped engine-side.
  if (".!?".includes(last)) {
    void fireTrigger("punctuation");
  } else if (draft.length >= DRAFT_WINDOW_CHARS) {
    void fireTrigger("idle");
  } else {
    window.clearTimeout(idleTimer);
    idleTimer = window.setTimeout(() => void fireTrigger("idle"), LIVE_DEBOUNCE_MS);
  }
});

playgroundEl.addEventListener("keydown", (e) => {
  if (!suggesting) return;
  // Pinyin-style keys while candidates are visible: digits pick, arrow keys
  // move the highlight, Tab accepts the highlighted candidate (Space stays a
  // real character in English, so Tab plays Space's role), Escape keeps the
  // literal text. Any other key types through and the bar simply refreshes
  // with the growing burst.
  if (e.key >= "1" && e.key <= "5") {
    e.preventDefault();
    void selectCandidate(Number(e.key) - 1);
  } else if (e.key === "ArrowLeft") {
    e.preventDefault();
    void api.moveSelection(-1);
  } else if (e.key === "ArrowRight") {
    e.preventDefault();
    void api.moveSelection(1);
  } else if (e.key === "Tab") {
    e.preventDefault();
    void selectCandidate(selectedIndex);
  } else if (e.key === "Escape") {
    e.preventDefault();
    endSession();
  }
});

function applyReplacement(text: string) {
  const value = playgroundEl.value;
  playgroundEl.value = value.slice(0, burstStart) + text + value.slice(burstEnd);
  const caret = burstStart + text.length;
  playgroundEl.setSelectionRange(caret, caret);
  burstStart = caret;
  burstEnd = caret;
  playgroundEl.focus();
}

// ---- health, metrics, corpus comparison ----

// Read-only confirmation of which backend is actually answering — the
// live/base combination is normally forced by run-live-app.sh's env vars
// rather than picked here, so this just proves the demo is really hitting
// Qwen instead of quietly falling back to fixture data.
function renderBackendMode(next: AppSettings) {
  backendModeEl.textContent = `${next.backend_mode} · ${next.model_variant}`;
  backendModeEl.className = "pill " + (next.backend_mode === "live" ? "ok" : "muted");
}

function renderHealth(health: SidecarHealth) {
  healthStatusEl.textContent = health.status;
  healthStatusEl.className =
    "pill " +
    (health.status === "ready" ? "ok" : health.status === "degraded" ? "warn" : "bad");
  const loaded = Object.entries(health.loaded)
    .map(([name, on]) => `${name}: ${on ? "✓" : "no"}`)
    .join("  ");
  const err = health.error ? `  ·  ${health.error.code}` : "";
  healthLoadedEl.textContent = `fixture: ${health.fixture_available ? "✓" : "no"}  ${loaded}${err}`;
}

function renderMetrics(metrics: Metrics) {
  const avg = metrics.avg_latency_ms === null ? "none" : `${Math.round(metrics.avg_latency_ms)} ms`;
  metricsEl.textContent =
    `requests ${metrics.requests} · ok ${metrics.ok} · errors ${metrics.errors}` +
    ` · schema-invalid ${metrics.schema_invalid} · avg latency ${avg}`;
}

async function refresh() {
  renderHealth(await api.getHealth());
  renderMetrics(await api.getMetrics());
}

function renderResult(side: ComparisonSide): HTMLElement {
  const box = el("div", "card result-side");
  box.append(el("strong", undefined, side.spec.label));
  box.append(
    el(
      "div",
      "muted",
      `${side.spec.model_variant} · ${side.spec.profile_id}` +
        (side.spec.use_context ? " · window context" : ""),
    ),
  );
  const result: PredictionResult = side.result;
  if (result.status === "ok") {
    const latency = ` · ${result.latency_ms} ms`;
    box.append(el("div", "result-meta", `${result.candidates.length} candidates${latency}`));
    if (result.candidates.length === 0) {
      box.append(el("div", "mono", "(keeps the typed text; no bar is shown)"));
    } else {
      const list = el("ul");
      for (const candidate of result.candidates) {
        list.append(el("li", "mono", candidate));
      }
      box.append(list);
    }
  } else {
    const err = el("div", "result-meta", `error: ${result.error.code}: ${result.error.message}`);
    err.style.color = "var(--bad)";
    box.append(err);
  }
  return box;
}

function renderComparison(report: ComparisonReport) {
  comparisonEl.replaceChildren();
  comparisonEl.append(el("p", undefined, report.description));
  const draft = el("p", "mono");
  draft.textContent = `draft: ${report.draft}`;
  draft.style.margin = "6px 0";
  comparisonEl.append(draft);
  const grid = el("div", "grid2");
  grid.append(renderResult(report.left), renderResult(report.right));
  comparisonEl.append(grid);
}

failureEl.addEventListener("change", async () => {
  await api.setSimulateFailure(failureEl.checked);
  await refresh();
});

byId<HTMLButtonElement>("fire_textedit").addEventListener("click", () => {
  void api.injectCapture({
    status: "ready",
    burst_id: `burst_textedit_${Date.now()}`,
    destination_id: "destination_textedit",
    profile_id: "profile_default",
    draft: "cnt cm tmrw",
    trigger: "idle",
    caret: { x: 512, y: 384, width: 2, height: 18 },
  });
});

byId<HTMLButtonElement>("capture_focused").addEventListener("click", () => {
  activeBurstId = undefined;
  void api.captureActiveDestination("shortcut");
});

byId<HTMLButtonElement>("fire_secure").addEventListener("click", () => {
  void api.injectCapture({ status: "unavailable", reason: "secure_field" });
});

// ---- event wiring ----

void events.onMetrics(renderMetrics);
void events.onSettings((next) => {
  settings = next;
  renderBackendMode(next);
});
void events.onSnapshot((snapshot) => {
  lastStateEl.textContent =
    snapshot.phase === "unavailable"
      ? `composition: unavailable (${snapshot.reason})`
      : `composition: ${snapshot.phase}`;
  appendTimelineEvent("snapshot", snapshotSummary(snapshot), { phase: snapshot.phase });
  renderInlineCandidates(snapshot);
  if (snapshot.phase === "predicting") return; // bar keeps current candidates
  suggesting =
    snapshot.phase === "suggesting" && snapshot.burst_id === activeBurstId;
  selectedIndex = snapshot.phase === "suggesting" ? snapshot.selected : 0;
  if (snapshot.phase === "suggesting" && snapshot.backend) {
    lastStateEl.textContent += ` (${snapshot.backend} · ${snapshot.latency_ms} ms)`;
  }
});
void events.onCommitted((outcome) => {
  lastCommitEl.textContent = `last commit → ${outcome.destination_id}: "${outcome.text}"`;
  appendTimelineEvent("commit_succeeded", `${outcome.destination_id} chars=${outcome.text.length}`, {
    destination_id: outcome.destination_id,
    committed_chars: outcome.text.length,
    success: true,
  });
  if (outcome.destination_id === "destination_playground") {
    applyReplacement(outcome.text);
  }
});

void (async () => {
  settings = await api.getSettings();
  renderBackendMode(settings);
  const cases = await api.listCorpus();
  for (const demoCase of cases) {
    const button = el("button", undefined, demoCase.title);
    button.title = demoCase.description;
    button.addEventListener("click", async () => {
      recordTimelineEvent("comparison_requested", demoCase.case_id, {
        case_id: demoCase.case_id,
      });
      try {
        const report = await api.runComparison(demoCase.case_id);
        renderComparison(report);
        recordTimelineEvent("comparison_result", `${report.case_id}: comparison rendered`, {
          case_id: report.case_id,
        });
        await refresh();
      } catch (error) {
        const summary = `comparison failed: ${String(error)}`;
        recordTimelineEvent("comparison_failed", summary, {
          case_id: demoCase.case_id,
          error: String(error),
        });
        lastStateEl.textContent = summary;
      }
    });
    casesEl.append(button);
  }
  await refresh();
  renderTimeline();
  void api.getDebugEvents(50).then(mergeDebugEvents).catch(() => {});
  window.setInterval(() => {
    void api.getDebugEvents(50).then(mergeDebugEvents).catch(() => {});
  }, 2000);
  window.setInterval(() => void refresh(), 3000);
})();
