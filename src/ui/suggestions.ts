// Candidate bar (Workstream 4). IME model: a small non-focusable window the
// Rust side anchors above the caret. Renders numbered candidate chips (or an
// explicit error chip) from engine snapshots. Clicks select; number keys and
// Escape are handled by whoever owns the keyboard (the playground now, the
// Workstream 3 event tap later) — this window never has key focus.

import { api, byId, el, events, type Snapshot } from "./ipc";

const barEl = byId<HTMLDivElement>("bar");

function render(snapshot: Snapshot) {
  // IME behavior: while the next prediction computes, keep showing the
  // current candidates instead of flickering empty.
  if (snapshot.phase === "predicting") return;
  barEl.replaceChildren();
  if (snapshot.phase !== "suggesting") return;

  if (snapshot.error) {
    const chip = el("span", "chip error-chip", `⚠ ${snapshot.error.code}`);
    chip.title = snapshot.error.message;
    barEl.append(chip);
    return;
  }

  snapshot.candidates.forEach((candidate, index) => {
    const chip = el("span", "chip");
    if (index === snapshot.selected) chip.classList.add("recommended");
    chip.append(el("b", undefined, String(index + 1)), el("span", undefined, candidate));
    chip.addEventListener("click", () => void api.selectCandidate(index));
    barEl.append(chip);
  });
}

void events.onSnapshot(render);
void api.getCompositionState().then(render);
