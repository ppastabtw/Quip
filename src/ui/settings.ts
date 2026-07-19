// Settings window (Workstream 4): reads and writes the persisted engine
// settings, and exposes the inspectable per-profile pattern dictionary.

import { api, byId, el, events, type AppSettings } from "./ipc";
import type { ModelVariant } from "./contracts";

const enabledEl = byId<HTMLInputElement>("enabled");
const pausedEl = byId<HTMLInputElement>("learning_paused");
const profileEl = byId<HTMLSelectElement>("active_profile");
const variantEl = byId<HTMLSelectElement>("model_variant");
const backendEl = byId<HTMLSelectElement>("backend_mode");
const resetEl = byId<HTMLButtonElement>("reset_profile");
const patternsTable = byId<HTMLTableElement>("patterns");
const patternsEmpty = byId<HTMLParagraphElement>("patterns_empty");

let settings: AppSettings | undefined;

function apply(next: AppSettings) {
  settings = next;
  enabledEl.checked = next.enabled;
  pausedEl.checked = next.learning_paused;
  profileEl.value = next.active_profile;
  variantEl.value = next.model_variant;
  backendEl.value = next.backend_mode;
  void renderPatterns(next.active_profile);
}

async function renderPatterns(profileId: string) {
  const patterns = await api.getPatterns(profileId);
  const body = patternsTable.tBodies[0];
  body.replaceChildren();
  patternsTable.hidden = patterns.length === 0;
  patternsEmpty.hidden = patterns.length > 0;
  for (const pattern of patterns) {
    const row = el("tr");
    row.append(
      el("td", "mono", pattern.shorthand),
      el("td", "mono", pattern.expansion),
      el("td", undefined, String(pattern.count)),
    );
    body.append(row);
  }
}

function push() {
  if (!settings) return;
  const next: AppSettings = {
    ...settings,
    enabled: enabledEl.checked,
    window_context: true,
    learning_paused: pausedEl.checked,
    active_profile: profileEl.value,
    model_variant: variantEl.value as ModelVariant,
    backend_mode: backendEl.value as AppSettings["backend_mode"],
  };
  settings = next;
  void api.updateSettings(next).then(() => renderPatterns(next.active_profile));
}

for (const control of [enabledEl, pausedEl, profileEl, variantEl, backendEl]) {
  control.addEventListener("change", push);
}

resetEl.addEventListener("click", () => {
  const profileId = profileEl.value;
  void api.resetProfile(profileId).then(() => renderPatterns(profileId));
});

void events.onSettings(apply);

void (async () => {
  const profiles = await api.listProfiles();
  profileEl.replaceChildren(
    ...profiles.map((p) => {
      const option = el("option", undefined, p);
      option.value = p;
      return option;
    }),
  );
  apply(await api.getSettings());
})();
