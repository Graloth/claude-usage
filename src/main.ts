import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { element } from "./dom";
import { formatAge } from "./format";
import { renderHeatmap } from "./heatmap";
import { renderLimits } from "./limits";
import { initSettings } from "./settings";
import type { ScanSnapshot, SectionName, Settings, Snapshot } from "./types";
import { renderValue } from "./value";

const USAGE_EVENT = "usage-updated";
const SCAN_EVENT = "scan-updated";

const stack = element("stack");
const title = element("title");
const banner = element("banner");
const sections = element("sections");
const updated = element("updated");
const mainView = element("main-view");
const settingsView = element("settings-view");
const settingsToggle = element("settings-toggle");

let snapshot: Snapshot | null = null;
let scan: ScanSnapshot | null = null;
let settings: Settings | null = null;

/** How each section draws itself; `null` means there is nothing to show yet. */
const renderers: Record<SectionName, () => HTMLElement | null> = {
  limits: () => (snapshot ? renderLimits(snapshot) : null),
  value: () => (scan && settings ? renderValue(scan, settings) : null),
  heatmap: () => (scan && settings ? renderHeatmap(scan.report, settings, render) : null),
};

function render(): void {
  if (!snapshot || !settings) return;

  banner.hidden = snapshot.problem === null;
  banner.textContent = snapshot.problem
    ? snapshot.limits.length > 0
      ? `Showing last known values. ${snapshot.problem.message}`
      : snapshot.problem.message
    : "";

  const parts = settings.sections
    .filter((entry) => entry.enabled)
    .map((entry) => renderers[entry.section]())
    .filter((el): el is HTMLElement => el !== null);
  sections.replaceChildren(...parts);
  updated.textContent = formatAge(snapshot.updated_at_ms);
}

function showSettings(show: boolean): void {
  settingsView.hidden = !show;
  mainView.hidden = show;
  title.textContent = show ? "Settings" : "Claude usage";
  settingsToggle.textContent = show ? "←" : "⚙";
  settingsToggle.title = show ? "Back" : "Settings";
}

// `hidden` can also be "until-found"; only a plain `true` means the form is closed.
settingsToggle.addEventListener("click", () => showSettings(settingsView.hidden === true));

window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (!settingsView.hidden) showSettings(false);
  else void getCurrentWindow().close();
});

element("refresh").addEventListener("click", () => {
  void invoke("refresh_now");
});

// Keep "Updated 3 min ago" honest while the popup stays open.
setInterval(() => {
  if (snapshot) updated.textContent = formatAge(snapshot.updated_at_ms);
}, 30_000);

// Tell the Rust side how tall the content is so it can size the window, and
// show it: the window stays hidden until this first report. Observing only
// after the first render avoids showing an empty "Loading…" frame.
const PANEL_BORDER = 2;
let fitFrame = 0;
function fitWindow(): void {
  cancelAnimationFrame(fitFrame);
  fitFrame = requestAnimationFrame(() => {
    void invoke("fit_popup", { height: stack.offsetHeight + PANEL_BORDER });
  });
}

async function main(): Promise<void> {
  void listen<Snapshot>(USAGE_EVENT, (event) => {
    snapshot = event.payload;
    render();
  });
  void listen<ScanSnapshot>(SCAN_EVENT, (event) => {
    scan = event.payload;
    render();
  });
  settings = await initSettings((saved) => {
    settings = saved;
    render();
  });
  [snapshot, scan] = await Promise.all([
    invoke<Snapshot>("get_snapshot"),
    invoke<ScanSnapshot>("get_scan"),
  ]);
  render();
  new ResizeObserver(fitWindow).observe(stack);
}

void main();
