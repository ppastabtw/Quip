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
import type { Mark } from "./ipc";
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
// grows and the bar refreshes in place. All inference is local, so calls are
// free; the constraints are the sidecar's serial throughput (one request at
// a time, each costing a full inference) and how the bar feels. A cadence
// strategy decides which keystrokes are worth a serial slot.
//
// The window size in words comes from settings (`window_words`, 5 until a
// retrained model raises it); the char backstop scales with it. The model
// never sees more than one window of words.
const windowWords = () => settings?.window_words ?? 5;
const windowChars = () => windowWords() * 16;

// Long enough that ordinary typing rhythm never trips it: only a deliberate
// stop mid-burst fetches early.
const PAUSE_MS = 600;

// Keyboard-ownership rule: the bar auto-expires after this many words are
// typed through it, capping how long digits 1–5 stay claimed. The offer's
// evidence lives on in the word slots, so an expired bar can still resurface
// as a quiet mark.
const BAR_EXPIRY_WORDS = 3;

interface KeyInfo {
  /** The last typed character completed a word (whitespace after a non-space). */
  wordBoundary: boolean;
  /** The last typed character is a sentence terminator. */
  sentencePunct: boolean;
  /** Rolling median of this typist's recent inter-key gaps. */
  medianGapMs: number;
}

interface CadenceStrategy {
  label: string;
  hint: string;
  /** Refire the moment a result lands if the draft moved meanwhile (the
   * sidecar never idles while dirty, and never queues more than one). */
  pipelined: boolean;
  /** Sliding-window cadence: fire the trailing window at word boundaries,
   * feed the edit accumulator only (marks, no bars), no chunk freezing. */
  barless?: boolean;
  /** "now" fires immediately, a number arms the pause timer, "never" leaves
   * only the draft-window char backstop. */
  onKey(key: KeyInfo): "now" | number | "never";
}

const STRATEGIES: Record<string, CadenceStrategy> = {
  chunk_pause: {
    label: "Pause, punctuation, or 5 words (current)",
    hint: "The five-word chunk and punctuation fire immediately; a 600 ms pause catches unfinished bursts. Typing never waits on inference.",
    pipelined: true,
    onKey: (key) => (key.sentencePunct ? "now" : PAUSE_MS),
  },
  sliding_word: {
    label: "Sliding window: marks, not bars",
    hint: "Every completed word fires the trailing five-word window; corrections that stay stable across passes harden into dotted underlines. ⌘⏎ applies them, Esc reverts, and a sentence end offers the whole corrected sentence in one bar.",
    pipelined: true,
    barless: true,
    onKey: (key) => (key.sentencePunct || key.wordBoundary ? "now" : 500),
  },
  pipeline_greedy: {
    label: "Pipelined: every keystroke",
    hint: "No timers: fetch immediately, and refetch the newest draft the moment the previous result lands.",
    pipelined: true,
    onKey: () => "now",
  },
  pipeline_word: {
    label: "Pipelined: word boundaries",
    hint: "Fetch on completed words so every serial slot sees whole words; 500 ms fallback mid-word.",
    pipelined: true,
    onKey: (key) => (key.sentencePunct || key.wordBoundary ? "now" : 500),
  },
  rhythm: {
    label: "Rhythm-adaptive pause",
    hint: "Pause threshold tracks your typing rhythm (2.2× median key gap); punctuation is immediate.",
    pipelined: false,
    onKey: (key) =>
      key.sentencePunct ? "now" : Math.min(700, Math.max(180, 2.2 * key.medianGapMs)),
  },
  sentence_only: {
    label: "Boundaries only",
    hint: "Fetch only at . ! ? — plus the universal five-word chunk and 80-character backstop.",
    pipelined: false,
    onKey: (key) => (key.sentencePunct ? "now" : "never"),
  },
};

/** A frozen span of text awaiting (or undergoing) inference. */
interface ChunkRange {
  start: number;
  end: number;
}

let settings: AppSettings | undefined;
let burstStart = 0;
/** Char offset where the current composition session began; word slots and
 * marks are indexed by words counted from here. */
let sessionStart = 0;
/** The engine's current word-slot proposals (marks event mirror). */
let sessionMarks: Mark[] = [];
/** The whole-session char range snapshotted when endSession fires, so a
 * consolidated sentence offer knows what it would replace. */
let consolidationRange: ChunkRange | undefined;
/** Words typed through the shown offer; at BAR_EXPIRY_WORDS the bar
 * auto-dismisses so digits stop being claimed. */
let typedThroughWords = 0;
/** The exact range each fired burst's candidates would replace, keyed by
 * burst id and pinned at request time: typing continues past it, so a
 * selection must replace exactly the words the model saw. Entries live until
 * the burst resolves (applied, dismissed, skipped, or invalidated). */
const firedRanges = new Map<string, ChunkRange>();
/** Chunks completed while inference was busy; fired oldest-first as the
 * serial slot frees up (15 words typed = three 5-word batches, one at a
 * time, while typing continues). */
const pendingChunks: ChunkRange[] = [];
/** Mirror of the engine's shown offer (the oldest unresolved batch):
 * underlined in the playground and the target of the selection keys. The
 * engine queues later batches behind it, so it survives their inference and
 * their results. */
let shownChunk:
  | (ChunkRange & { candidates: string[]; selected: number; burstId: string })
  | undefined;
let burstSeq = 0;
let activeBurstId: string | undefined;
let idleTimer: number | undefined;
let debugRows: DebugEventView[] = [];

let strategy: CadenceStrategy = STRATEGIES.chunk_pause;
let inflight = false;
let dirty = false;
let lastKeyAt = 0;
const keyGaps: number[] = [];
const stats = { keystrokes: 0, fired: 0, shown: 0, ttbLast: 0, ttbSum: 0 };

const strategyStatsEl = byId<HTMLParagraphElement>("strategy_stats");
const strategyHintEl = byId<HTMLSpanElement>("strategy_hint");
const strategyEl = byId<HTMLSelectElement>("strategy");

function medianGapMs(): number {
  if (keyGaps.length === 0) return 250;
  const sorted = [...keyGaps].sort((a, b) => a - b);
  return sorted[Math.floor(sorted.length / 2)];
}

function renderStrategyStats() {
  const avg = stats.shown === 0 ? "–" : `${Math.round(stats.ttbSum / stats.shown)} ms`;
  const last = stats.shown === 0 ? "–" : `${stats.ttbLast} ms`;
  const hardened = sessionMarks.filter((m) => m.stable).length;
  strategyStatsEl.textContent =
    `keystrokes ${stats.keystrokes} · requests ${stats.fired} · bars ${stats.shown}` +
    ` · time-to-bar last ${last} / avg ${avg}` +
    ` · median key gap ${Math.round(medianGapMs())} ms` +
    ` · marks ${hardened}/${sessionMarks.length} hardened`;
}

/** Absolute char ranges of the session's words (whitespace-separated tokens
 * from sessionStart up to `upTo`). Index k here is session word index k. */
function sessionWordRanges(upTo: number): ChunkRange[] {
  const slice = playgroundEl.value.slice(sessionStart, upTo);
  const ranges: ChunkRange[] = [];
  for (const match of slice.matchAll(/\S+/g)) {
    ranges.push({
      start: sessionStart + match.index,
      end: sessionStart + match.index + match[0].length,
    });
  }
  return ranges;
}

/** Char range covering session words [startWord, startWord + wordLen), or
 * undefined when the text no longer has those words. */
function wordCharRange(startWord: number, wordLen: number): ChunkRange | undefined {
  const words = sessionWordRanges(playgroundEl.value.length);
  const last = startWord + Math.max(wordLen, 1) - 1;
  if (startWord >= words.length || last >= words.length) return undefined;
  return { start: words[startWord].start, end: words[last].end };
}

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
  sessionStart = caret;
  activeBurstId = undefined;
  inflight = false;
  dirty = false;
  pendingChunks.length = 0;
  firedRanges.clear();
  shownChunk = undefined;
  consolidationRange = undefined;
  typedThroughWords = 0;
  void api.endSession();
  renderChunkIndicator();
  recordTimelineEvent("playground_reset", reason, {
    reason,
    caret,
    value_chars: playgroundEl.value.length,
  });
}

const backdropEl = byId<HTMLDivElement>("playground_backdrop");

// The backdrop mirrors the textarea's text with transparent glyphs; only
// annotation spans draw. Two kinds: the solid underline under the words the
// visible candidates would replace, and dotted underlines under hardened
// marks (hover shows original → replacement).
function renderChunkIndicator() {
  const value = playgroundEl.value;
  interface Span extends ChunkRange {
    cls: string;
    title?: string;
  }
  const spans: Span[] = [];
  for (const mark of sessionMarks) {
    if (!mark.stable || mark.word_len === 0) continue;
    const range = wordCharRange(mark.start_word, mark.word_len);
    if (range) {
      spans.push({ ...range, cls: "mark-underline", title: `${mark.original} → ${mark.replacement}` });
    }
  }
  if (shownChunk) {
    spans.push({ start: shownChunk.start, end: shownChunk.end, cls: "chunk-underline" });
  }
  spans.sort((a, b) => a.start - b.start);
  const nodes: Node[] = [];
  let cursor = 0;
  for (const span of spans) {
    if (span.start < cursor) continue; // overlaps are rare; first wins
    nodes.push(document.createTextNode(value.slice(cursor, span.start)));
    const node = el("span", span.cls, value.slice(span.start, span.end));
    if (span.title) node.title = span.title;
    nodes.push(node);
    cursor = span.end;
  }
  nodes.push(document.createTextNode(value.slice(cursor)));
  backdropEl.replaceChildren(...nodes);
  backdropEl.scrollTop = playgroundEl.scrollTop;
  backdropEl.scrollLeft = playgroundEl.scrollLeft;
}

playgroundEl.addEventListener("scroll", () => {
  backdropEl.scrollTop = playgroundEl.scrollTop;
  backdropEl.scrollLeft = playgroundEl.scrollLeft;
});

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

async function fireTrigger(trigger: Trigger, chunk?: ChunkRange) {
  window.clearTimeout(idleTimer);
  const value = playgroundEl.value;
  let start = chunk ? chunk.start : burstStart;
  let end = chunk ? chunk.end : playgroundEl.selectionStart;
  if (value.length === 0 || end < start || start > value.length) {
    resetPlayground("cleared_or_caret_before_burst", end);
    return;
  }
  let wordOffset: number;
  if (strategy.barless && !chunk) {
    // Sliding cadence: the window is the trailing ≤ window_words words at
    // the caret, so consecutive passes overlap and per-word agreement can
    // accumulate.
    const words = sessionWordRanges(playgroundEl.selectionStart);
    if (words.length === 0) {
      inflight = false;
      return;
    }
    const windowLen = Math.min(words.length, windowWords());
    start = words[words.length - windowLen].start;
    end = words[words.length - 1].end;
    wordOffset = words.length - windowLen;
  } else {
    // Trailing whitespace stays outside the replaced range so an accepted
    // candidate keeps its separator from the words typed after it.
    while (end > start && /\s/.test(value[end - 1])) end -= 1;
    wordOffset = sessionWordRanges(start).length;
  }
  const draft = value.slice(start, end).trim();
  if (draft.length === 0) {
    inflight = false;
    return;
  }
  recordTimelineEvent("playground_input", `draft_chars=${draft.length}`, {
    source: "playground",
    trigger,
    draft_chars: draft.length,
  });
  activeBurstId = `pg_${++burstSeq}`;
  firedRanges.set(activeBurstId, { start, end });
  void api.injectCapture(
    {
      status: "ready",
      burst_id: activeBurstId,
      destination_id: "destination_playground",
      profile_id: settings?.active_profile ?? "profile_default",
      draft,
      trigger,
      caret: await caretScreenRect(),
      word_offset: wordOffset,
    },
    strategy.barless === true,
  );
}

// Spends a serial slot, or earmarks one: pipelined strategies never queue
// behind an in-flight request — they mark the draft dirty and refire with the
// newest text the moment the result lands. Non-pipelined strategies fire
// unconditionally (overlapping requests supersede; stale results are dropped
// engine-side), which is exactly the head-of-line cost the demo lets you feel.
function requestPrediction(trigger: Trigger) {
  window.clearTimeout(idleTimer);
  if (strategy.pipelined && inflight) {
    dirty = true;
    return;
  }
  inflight = true;
  dirty = false;
  stats.fired += 1;
  renderStrategyStats();
  void fireTrigger(trigger);
}

// Completing the fifth word freezes the chunk and starts a fresh burst at the
// caret immediately: inference on the chunk overlaps typing the next words,
// so the typist never waits. If the serial slot is busy the chunk queues and
// fires the moment the previous result lands.
function chunkBurst(caret: number) {
  window.clearTimeout(idleTimer);
  const chunk: ChunkRange = { start: burstStart, end: caret };
  burstStart = caret;
  if (inflight) {
    pendingChunks.push(chunk);
    dirty = false;
    return;
  }
  inflight = true;
  stats.fired += 1;
  renderStrategyStats();
  void fireTrigger("idle", chunk);
}

// Ends the composition session at the current caret: the engine records the
// visible offer as a stable dismissal (a keep label), drops queued offers and
// the in-flight burst, and the next keystroke starts a fresh burst. If
// hardened marks were never applied, the engine answers with one
// consolidated whole-sentence offer; the range it would replace is
// snapshotted here, before the session moves on.
function endSession() {
  window.clearTimeout(idleTimer);
  dirty = false;
  // A newline is a hard composition boundary. Release the frontend's serial
  // slot even when a request is still in flight; the engine drops its result
  // as stale.
  inflight = false;
  activeBurstId = undefined;
  pendingChunks.length = 0;
  firedRanges.clear();
  shownChunk = undefined;
  const words = sessionWordRanges(playgroundEl.selectionStart);
  consolidationRange =
    words.length > 0
      ? { start: words[0].start, end: words[words.length - 1].end }
      : undefined;
  renderChunkIndicator();
  void api.endSession();
  burstStart = playgroundEl.selectionStart;
  sessionStart = burstStart;
}

playgroundEl.addEventListener("input", () => {
  const now = performance.now();
  if (lastKeyAt > 0) {
    const gap = now - lastKeyAt;
    // Gaps past 1.2 s are thinking pauses, not typing rhythm.
    if (gap < 1200) {
      keyGaps.push(gap);
      if (keyGaps.length > 15) keyGaps.shift();
    }
  }
  lastKeyAt = now;
  stats.keystrokes += 1;

  const caret = playgroundEl.selectionStart;
  // Clearing the textarea, replacing a selection, or editing before the
  // tracked burst invalidates the old offsets. Start a fresh burst at the
  // current edit instead of slicing from an unreachable prior position.
  if (caret < burstStart || playgroundEl.value.length < burstStart) {
    resetPlayground("cleared_or_caret_before_burst", Math.max(0, caret - 1));
    return;
  }
  // Editing at or before a fired range invalidates its candidates, whether
  // the batch is on display, queued, or still inferring: the words the model
  // saw are gone.
  for (const [burstId, range] of firedRanges) {
    if (caret <= range.end) {
      firedRanges.delete(burstId);
      if (burstId === activeBurstId) {
        activeBurstId = undefined;
        inflight = false;
      }
      if (shownChunk?.burstId === burstId) shownChunk = undefined;
      void api.retractOffer(burstId);
    }
  }
  renderChunkIndicator();
  const draft = playgroundEl.value.slice(burstStart, caret);
  const last = draft.at(-1) ?? "";
  // Sentence boundary ends the session: a newline, or whitespace after a
  // terminator (the burst before it was already predicted on).
  if (last === "\n" || (/\s/.test(last) && /[.!?]$/.test(draft.trimEnd()))) {
    endSession();
    renderStrategyStats();
    return;
  }
  if (draft.trim().length === 0) {
    resetPlayground("empty_draft", caret);
    renderStrategyStats();
    return;
  }
  const key: KeyInfo = {
    wordBoundary: /\s/.test(last) && draft.length > 1 && !/\s/.test(draft.at(-2) ?? " "),
    sentencePunct: ".!?".includes(last),
    medianGapMs: medianGapMs(),
  };
  // Keyboard ownership: a bar that has been typed through for several words
  // stops claiming digits — it dismisses itself (a stable dismissal; its
  // evidence lives on as word-slot marks). Surfaces change only at word
  // boundaries, never mid-word.
  if (key.wordBoundary && shownChunk) {
    typedThroughWords += 1;
    if (typedThroughWords >= BAR_EXPIRY_WORDS) {
      typedThroughWords = 0;
      firedRanges.delete(shownChunk.burstId);
      shownChunk = undefined;
      renderChunkIndicator();
      void api.dismissSuggestions();
    }
  }
  // The window-words chunk boundary freezes a chunk under every bar-mode
  // strategy — the model never sees an oversized draft. The sliding cadence
  // has no frozen chunks: its window construction bounds itself.
  if (
    !strategy.barless &&
    /\s/.test(last) &&
    draft.trim().split(/\s+/).filter(Boolean).length >= windowWords()
  ) {
    chunkBurst(caret);
    renderStrategyStats();
    return;
  }
  // The draft window is a strategy-independent backstop: past the char cap
  // the burst must be predicted on before it grows unwieldy.
  if (draft.length >= windowChars()) {
    requestPrediction("idle");
    return;
  }
  const action = strategy.onKey(key);
  if (action === "now") {
    requestPrediction(key.sentencePunct ? "punctuation" : "idle");
  } else if (action !== "never") {
    window.clearTimeout(idleTimer);
    idleTimer = window.setTimeout(() => requestPrediction("idle"), action);
  }
  renderStrategyStats();
});

// Applies every hardened mark. Marks come back in pre-apply session word
// coordinates, so they replay right to left — earlier (left) char ranges
// stay valid while later ones are replaced.
async function applyAllMarks() {
  const applied = await api.applyMarks();
  for (const mark of [...applied].sort((a, b) => b.start_word - a.start_word)) {
    if (mark.word_len === 0) {
      const words = sessionWordRanges(playgroundEl.value.length);
      const at = words[mark.start_word]?.start;
      if (at !== undefined) applyRangeReplacement(at, at, `${mark.replacement} `);
      continue;
    }
    const range = wordCharRange(mark.start_word, mark.word_len);
    if (range) applyRangeReplacement(range.start, range.end, mark.replacement);
  }
}

// Keyboard ownership (single-owner invariant): the caret-anchored bar, when
// visible, always owns the unmodified keys — digits pick, arrows move the
// highlight, Tab accepts (Space stays a real character in English, so Tab
// plays Space's role), Escape keeps the literal text. The engine's
// suggesting state is always the offer on display (later batches queue
// behind it), so those keys route through the engine. Marks never claim
// plain keys: they are chorded (⌘⏎ applies all) or take Escape only when no
// bar is up. Any other key types through untouched.
playgroundEl.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
    if (sessionMarks.some((m) => m.stable)) {
      e.preventDefault();
      void applyAllMarks();
    }
    return;
  }
  const offered = shownChunk;
  if (!offered) {
    if (e.key === "Escape" && sessionMarks.some((m) => m.stable)) {
      e.preventDefault();
      void api.clearMarks();
    }
    return;
  }
  const accept = (index: number) => {
    if (index >= offered.candidates.length) return;
    e.preventDefault();
    void selectCandidate(index);
  };
  if (e.key >= "1" && e.key <= "5") {
    accept(Number(e.key) - 1);
  } else if (e.key === "ArrowLeft" && offered.candidates.length > 0) {
    e.preventDefault();
    void api.moveSelection(-1);
  } else if (e.key === "ArrowRight" && offered.candidates.length > 0) {
    e.preventDefault();
    void api.moveSelection(1);
  } else if (e.key === "Tab") {
    accept(offered.selected);
  } else if (e.key === "Escape") {
    e.preventDefault();
    firedRanges.delete(offered.burstId);
    shownChunk = undefined;
    renderChunkIndicator();
    void api.dismissSuggestions();
  }
});

function applyRangeReplacement(start: number, end: number, text: string) {
  const value = playgroundEl.value;
  const prevCaret = playgroundEl.selectionStart;
  const delta = text.length - (end - start);
  playgroundEl.value = value.slice(0, start) + text + value.slice(end);
  // The typist may already be words past the replaced batch: keep their caret
  // where they are typing (shifted by the edit) instead of yanking it back.
  const caret = prevCaret >= end ? prevCaret + delta : start + text.length;
  playgroundEl.setSelectionRange(caret, caret);
  if (burstStart >= end) {
    burstStart += delta;
  } else {
    burstStart = start + text.length;
  }
  // The in-flight batch and any queued ones sit after the replaced range;
  // their pinned offsets shift with the edit.
  for (const range of firedRanges.values()) {
    if (range.start >= end) {
      range.start += delta;
      range.end += delta;
    }
  }
  for (const chunk of pendingChunks) {
    if (chunk.start >= end) {
      chunk.start += delta;
      chunk.end += delta;
    }
  }
  shownChunk = undefined;
  renderChunkIndicator();
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
// Mirrors the engine's word-slot proposals: hardened ones draw as dotted
// underlines in the backdrop, and unlock ⌘⏎ (apply all) / Escape (revert).
void events.onMarks((marks) => {
  sessionMarks = marks;
  renderChunkIndicator();
  renderStrategyStats();
});
void events.onSnapshot((snapshot) => {
  lastStateEl.textContent =
    snapshot.phase === "unavailable"
      ? `composition: unavailable (${snapshot.reason})`
      : `composition: ${snapshot.phase}`;
  if (snapshot.phase === "suggesting" && snapshot.backend) {
    lastStateEl.textContent += ` (${snapshot.backend} · ${snapshot.latency_ms} ms)`;
  }
  appendTimelineEvent("snapshot", snapshotSummary(snapshot), { phase: snapshot.phase });
  renderInlineCandidates(snapshot);
  if (snapshot.phase === "predicting") return; // bar keeps current candidates
  if (snapshot.phase === "applied") {
    // A candidate was committed: replace exactly the words the model saw.
    const range = firedRanges.get(snapshot.burst_id);
    firedRanges.delete(snapshot.burst_id);
    if (range) applyRangeReplacement(range.start, range.end, snapshot.text);
    return;
  }
  if (snapshot.phase === "suggesting") {
    // The engine shows the oldest unresolved offer; mirror it and underline
    // exactly the words its candidates would replace (the typist may be
    // several words past them by now). Bursts fired outside the playground
    // (scripted capture buttons) have no range and get no underline. The
    // sentence-boundary consolidation offer is synthesized engine-side with
    // no matching fireTrigger call, so it has no firedRanges entry yet;
    // adopt the range snapshotted at endSession() and record it under the
    // engine's burst id so the "applied" branch above finds it the same way
    // as any other offer.
    let range = firedRanges.get(snapshot.burst_id);
    if (!range && snapshot.burst_id.startsWith("consolidated_") && consolidationRange) {
      range = consolidationRange;
      firedRanges.set(snapshot.burst_id, range);
    }
    shownChunk = range
      ? {
          ...range,
          candidates: snapshot.candidates,
          selected: snapshot.selected,
          burstId: snapshot.burst_id,
        }
      : undefined;
  } else {
    shownChunk = undefined;
  }
  renderChunkIndicator();
});
// A settled prediction frees the serial slot: fire the next queued batch (or
// refire a dirty draft) the moment the engine can take it.
void events.onSettled(({ burst_id, offered }) => {
  if (offered) {
    stats.shown += 1;
    stats.ttbLast = Math.max(0, Math.round(performance.now() - lastKeyAt));
    stats.ttbSum += stats.ttbLast;
  } else {
    // Skipped or stale: nothing will ever resolve this range.
    firedRanges.delete(burst_id);
  }
  renderStrategyStats();
  if (burst_id !== activeBurstId) return; // a retracted straggler, not the slot
  inflight = false;
  const next = pendingChunks.shift();
  if (next) {
    inflight = true;
    stats.fired += 1;
    renderStrategyStats();
    void fireTrigger("idle", next);
  } else if (dirty) {
    dirty = false;
    requestPrediction("idle");
  }
});
void events.onCommitted((outcome) => {
  lastCommitEl.textContent = `last commit → ${outcome.destination_id}: "${outcome.text}"`;
  // The actual text replacement happens in onSnapshot's "applied" branch
  // (keyed by firedRanges/burst id, which correctly supports queued and
  // consolidated offers); this is observability only.
  appendTimelineEvent("commit_succeeded", `${outcome.destination_id} chars=${outcome.text.length}`, {
    destination_id: outcome.destination_id,
    committed_chars: outcome.text.length,
    success: true,
  });
});

for (const [id, spec] of Object.entries(STRATEGIES)) {
  const option = el("option", undefined, spec.label);
  option.value = id;
  strategyEl.append(option);
}
strategyHintEl.textContent = strategy.hint;
renderStrategyStats();
strategyEl.addEventListener("change", () => {
  strategy = STRATEGIES[strategyEl.value];
  strategyHintEl.textContent = strategy.hint;
  window.clearTimeout(idleTimer);
  dirty = false;
  keyGaps.length = 0;
  Object.assign(stats, { keystrokes: 0, fired: 0, shown: 0, ttbLast: 0, ttbSum: 0 });
  renderStrategyStats();
  playgroundEl.focus();
});

void (async () => {
  settings = await api.getSettings();
  renderBackendMode(settings);
  sessionMarks = await api.getMarks();
  renderChunkIndicator();
  renderStrategyStats();
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
