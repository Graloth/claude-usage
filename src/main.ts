import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

// Mirrors `Snapshot` in src-tauri/src/snapshot.rs.
type Level = "normal" | "warning" | "critical";

interface LimitView {
  kind: string;
  label: string;
  percent: number;
  resets_at: string | null;
  level: Level;
}

interface Snapshot {
  limits: LimitView[];
  updated_at_ms: number | null;
  problem: { kind: string; message: string } | null;
}

const UPDATED_EVENT = "usage-updated";

function element<T extends HTMLElement>(id: string): T {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing #${id}`);
  return el as T;
}

const banner = element("banner");
const empty = element("empty");
const limitsList = element<HTMLUListElement>("limits");
const updated = element("updated");

let current: Snapshot | null = null;

const clock = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
});
const dayAndClock = new Intl.DateTimeFormat(undefined, {
  weekday: "short",
  hour: "2-digit",
  minute: "2-digit",
});

/** "resets 11:30" within a day, "resets Thu 08:00" later. */
function formatReset(iso: string | null): string {
  if (!iso) return "";
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  const withinDay = at.getTime() - Date.now() < 24 * 3600 * 1000;
  return `resets ${(withinDay ? clock : dayAndClock).format(at)}`;
}

function formatAge(ms: number | null): string {
  if (ms === null) return "";
  const minutes = Math.floor((Date.now() - ms) / 60_000);
  if (minutes < 1) return "Updated just now";
  if (minutes < 60) return `Updated ${minutes} min ago`;
  return `Updated ${Math.floor(minutes / 60)} h ${minutes % 60} min ago`;
}

function limitRow(limit: LimitView): HTMLLIElement {
  const label = document.createElement("span");
  label.textContent = limit.label;
  const percent = document.createElement("span");
  percent.className = "percent";
  percent.textContent = `${Math.round(limit.percent)}%`;

  const top = document.createElement("div");
  top.className = "row";
  top.append(label, percent);

  const fill = document.createElement("div");
  fill.className = `fill ${limit.level}`;
  // CSSOM writes are allowed under the strict CSP; inline style attributes are not.
  fill.style.setProperty("--pct", String(Math.min(100, Math.max(0, limit.percent))));
  const bar = document.createElement("div");
  bar.className = "bar";
  bar.append(fill);

  const reset = document.createElement("div");
  reset.className = "reset muted";
  reset.textContent = formatReset(limit.resets_at);

  const row = document.createElement("li");
  row.append(top, bar, reset);
  return row;
}

function render(snapshot: Snapshot): void {
  current = snapshot;
  const stale = snapshot.problem !== null;

  banner.hidden = !stale;
  banner.textContent = snapshot.problem
    ? snapshot.limits.length > 0
      ? `Showing last known values. ${snapshot.problem.message}`
      : snapshot.problem.message
    : "";

  empty.hidden = snapshot.limits.length > 0 || stale;
  limitsList.classList.toggle("stale", stale);
  limitsList.replaceChildren(...snapshot.limits.map(limitRow));
  updated.textContent = formatAge(snapshot.updated_at_ms);
}

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    void getCurrentWindow().close();
  }
});

element("refresh").addEventListener("click", () => {
  void invoke("refresh_now");
});

// Keep "Updated 3 min ago" honest while the popup stays open.
setInterval(() => {
  if (current) updated.textContent = formatAge(current.updated_at_ms);
}, 30_000);

void listen<Snapshot>(UPDATED_EVENT, (event) => render(event.payload));
void invoke<Snapshot>("get_snapshot").then(render);
