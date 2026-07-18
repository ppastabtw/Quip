// Demo harness (Workstream 4): the typing playground (stand-in for any macOS
// textbox until Workstream 3 lands), deterministic corpus comparison, sidecar
// health and schema-validity counters, and scripted capture_result drivers.

import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  api,
  byId,
  el,
  events,
  type AppSettings,
  type ComparisonReport,
  type ComparisonSide,
  type Metrics,
} from "./ipc";
import type { PredictionResult, Rect, SidecarHealth, Trigger } from "./contracts";

const healthStatusEl = byId<HTMLSpanElement>("health_status");
const healthLoadedEl = byId<HTMLSpanElement>("health_loaded");
const metricsEl = byId<HTMLSpanElement>("metrics");
const failureEl = byId<HTMLInputElement>("simulate_failure");
const casesEl = byId<HTMLDivElement>("cases");
const comparisonEl = byId<HTMLDivElement>("comparison");
const lastStateEl = byId<HTMLSpanElement>("last_state");
const lastCommitEl = byId<HTMLParagraphElement>("last_commit");
const playgroundEl = byId<HTMLTextAreaElement>("playground");

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

const measureCanvas = document.createElement("canvas").getContext("2d")!;

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
  const draft = playgroundEl.value.slice(burstStart, burstEnd).trim();
  if (draft.length === 0) return;
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
  const draft = playgroundEl.value.slice(burstStart, caret);
  const last = draft.at(-1) ?? "";
  // Sentence boundary ends the session: a newline, or whitespace after a
  // terminator (the burst before it was already predicted on).
  if (last === "\n" || (/\s/.test(last) && /[.!?]$/.test(draft.trimEnd()))) {
    endSession();
    return;
  }
  if (draft.trim().length === 0) return;
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
    void api.selectCandidate(Number(e.key) - 1);
  } else if (e.key === "ArrowLeft") {
    e.preventDefault();
    void api.moveSelection(-1);
  } else if (e.key === "ArrowRight") {
    e.preventDefault();
    void api.moveSelection(1);
  } else if (e.key === "Tab") {
    e.preventDefault();
    void api.selectCandidate(selectedIndex);
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

byId<HTMLButtonElement>("fire_secure").addEventListener("click", () => {
  void api.injectCapture({ status: "unavailable", reason: "secure_field" });
});

// ---- event wiring ----

void events.onMetrics(renderMetrics);
void events.onSettings((next) => {
  settings = next;
});
void events.onSnapshot((snapshot) => {
  lastStateEl.textContent = `composition: ${snapshot.phase}`;
  if (snapshot.phase === "predicting") return; // bar keeps current candidates
  suggesting =
    snapshot.phase === "suggesting" && snapshot.burst_id === activeBurstId;
  selectedIndex = snapshot.phase === "suggesting" ? snapshot.selected : 0;
});
void events.onCommitted((outcome) => {
  lastCommitEl.textContent = `last commit → ${outcome.destination_id}: "${outcome.text}"`;
  if (outcome.destination_id === "destination_playground") {
    applyReplacement(outcome.text);
  }
});

void (async () => {
  settings = await api.getSettings();
  const cases = await api.listCorpus();
  for (const demoCase of cases) {
    const button = el("button", undefined, demoCase.title);
    button.title = demoCase.description;
    button.addEventListener("click", async () => {
      renderComparison(await api.runComparison(demoCase.case_id));
      await refresh();
    });
    casesEl.append(button);
  }
  await refresh();
  window.setInterval(() => void refresh(), 3000);
})();
