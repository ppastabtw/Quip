// Composition box (Workstream 4). A pure renderer of engine snapshots:
// typing is the pre-Workstream-3 capture surface, every state change comes
// back through composition://state events, and commits only happen through
// explicit confirmation (Enter / click).

import { api, byId, el, events, type Snapshot } from "./ipc";
import type { Trigger } from "./contracts";

const IDLE_TRIGGER_MS = 800; // spec: 700–900 ms idle window, tune later
const DRAFT_WINDOW_CHARS = 80;

const draftEl = byId<HTMLTextAreaElement>("draft");
const statusEl = byId<HTMLDivElement>("status");
const optionsEl = byId<HTMLOListElement>("options");
const variantEl = byId<HTMLSpanElement>("variant");
const flashEl = byId<HTMLDivElement>("commit-flash");

let snapshot: Snapshot = { phase: "idle" };
let selected = 0;
let idleTimer: number | undefined;
let lastSubmitted = "";

function editable(): boolean {
  return snapshot.phase === "idle" || snapshot.phase === "unavailable";
}

function submit(trigger: Trigger) {
  window.clearTimeout(idleTimer);
  const draft = draftEl.value.trim();
  if (!editable() || draft.length === 0 || draft === lastSubmitted) return;
  lastSubmitted = draft;
  void api.submitBurst(draft, trigger);
}

function scheduleIdle() {
  window.clearTimeout(idleTimer);
  idleTimer = window.setTimeout(() => submit("idle"), IDLE_TRIGGER_MS);
}

draftEl.addEventListener("input", () => {
  if (!editable()) return;
  lastSubmitted = "";
  const text = draftEl.value;
  if (text.trim().length === 0) return;
  const last = text.at(-1) ?? "";
  if (".!?".includes(last)) {
    submit("punctuation");
  } else if (text.length >= DRAFT_WINDOW_CHARS) {
    submit("idle");
  } else {
    scheduleIdle();
  }
});

draftEl.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && editable()) {
    e.preventDefault();
    submit("return");
  }
});

document.addEventListener("keydown", (e) => {
  if (snapshot.phase !== "presenting") return;
  const count = snapshot.options.length;
  if (e.key === "ArrowDown") {
    e.preventDefault();
    selected = (selected + 1) % count;
    renderOptions();
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    selected = (selected + count - 1) % count;
    renderOptions();
  } else if (e.key === "Enter") {
    e.preventDefault();
    void api.confirmOption(selected);
  } else if (e.key === "Escape") {
    e.preventDefault();
    void api.cancelComposition();
  }
});

function renderOptions() {
  optionsEl.replaceChildren();
  if (snapshot.phase !== "presenting") return;
  snapshot.options.forEach((option, index) => {
    const li = el("li", "mono");
    if (index === selected) li.classList.add("selected");
    if (snapshot.phase === "presenting" && index === snapshot.recommended) {
      li.classList.add("recommended");
    }
    const tag = el(
      "span",
      "tag",
      option.kind === "exact" ? "draft" : `option ${index}`,
    );
    li.append(tag, el("span", undefined, option.text));
    li.addEventListener("click", () => void api.confirmOption(index));
    li.addEventListener("mousemove", () => {
      if (selected !== index) {
        selected = index;
        renderOptions();
      }
    });
    optionsEl.append(li);
  });
}

function render() {
  statusEl.classList.remove("error");
  switch (snapshot.phase) {
    case "idle":
      variantEl.textContent = "";
      statusEl.textContent = "";
      draftEl.readOnly = false;
      renderOptions();
      break;
    case "predicting":
      variantEl.textContent = snapshot.model_variant;
      if (draftEl.value.trim() !== snapshot.draft) draftEl.value = snapshot.draft;
      draftEl.readOnly = true;
      statusEl.textContent = "Predicting…";
      renderOptions();
      break;
    case "presenting": {
      variantEl.textContent = snapshot.model_variant;
      draftEl.readOnly = true;
      selected = snapshot.recommended;
      if (snapshot.error) {
        statusEl.classList.add("error");
        statusEl.textContent = `Prediction unavailable (${snapshot.error.code}) — the exact draft still commits`;
      } else {
        const latency =
          snapshot.latency_ms === null ? "" : ` · ${snapshot.latency_ms} ms`;
        const keep = snapshot.options.length === 1 ? " · model says keep" : "";
        statusEl.textContent = `↑↓ choose, Enter commits, Esc cancels${latency}${keep}`;
      }
      renderOptions();
      break;
    }
    case "committed":
      draftEl.value = "";
      lastSubmitted = "";
      flashEl.textContent = `Committed to ${snapshot.destination_id}: ${snapshot.text}`;
      flashEl.style.display = "block";
      window.setTimeout(() => (flashEl.style.display = "none"), 1600);
      break;
    case "unavailable":
      variantEl.textContent = "";
      draftEl.readOnly = false;
      statusEl.classList.add("error");
      statusEl.textContent = `Quip unavailable: ${snapshot.reason}`;
      renderOptions();
      break;
  }
}

void events.onSnapshot((next) => {
  snapshot = next;
  render();
});

void api.getCompositionState().then((current) => {
  snapshot = current;
  render();
  draftEl.focus();
});
