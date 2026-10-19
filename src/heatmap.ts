import { h } from "./dom";
import { formatMoney, formatTokens } from "./format";
import type { DayStat, HeatmapMetric, ScanReport, Settings } from "./types";

type Mode = "days" | "hours";

const MODE_KEY = "heatmap-mode";
const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

function loadMode(): Mode {
  try {
    return localStorage.getItem(MODE_KEY) === "hours" ? "hours" : "days";
  } catch {
    return "days"; // storage can be unavailable; the heatmap must still render
  }
}

function saveMode(mode: Mode): void {
  try {
    localStorage.setItem(MODE_KEY, mode);
  } catch {
    // Not remembering the choice is fine.
  }
}

/** 0 for none, 1 to 4 relative to the busiest cell. */
function level(value: number, max: number): number {
  if (value <= 0 || max <= 0) return 0;
  return Math.min(4, Math.ceil((value / max) * 4));
}

function cell(lvl: number, title: string): HTMLDivElement {
  const el = h("div", `cell l${lvl}`);
  el.title = title;
  return el;
}

function describe(value: { cost: number; tokens: number }, metric: HeatmapMetric, settings: Settings): string {
  return metric === "cost"
    ? formatMoney(value.cost, settings.currency, settings.exchange_rate)
    : `${formatTokens(value.tokens)} tokens`;
}

function metricOf(value: { cost: number; tokens: number }, metric: HeatmapMetric): number {
  return metric === "cost" ? value.cost : value.tokens;
}

/** Local weekday of a YYYY-MM-DD string, Monday = 0. */
function weekdayOf(date: string): number {
  const [year, month, day] = date.split("-").map(Number);
  return (new Date(year, month - 1, day).getDay() + 6) % 7;
}

function dayGrid(report: ReportView, settings: Settings): HTMLElement {
  const metric = settings.heatmap_metric;
  const max = Math.max(0, ...report.daily.map((day) => metricOf(day, metric)));

  // Weeks as columns, Monday at the top; pad the first week up to its weekday.
  const grid = h("div", "grid days");
  const lead = report.daily.length > 0 ? weekdayOf(report.daily[0].date) : 0;
  for (let i = 0; i < lead; i++) grid.append(h("div", "cell blank"));
  for (const day of report.daily) {
    grid.append(
      cell(level(metricOf(day, metric), max), `${day.date}: ${describe(day, metric, settings)}, ${day.messages} messages`),
    );
  }
  return grid;
}

function hourGrid(report: ReportView, settings: Settings): HTMLElement {
  const metric = settings.heatmap_metric;
  const max = Math.max(0, ...report.hours.map((hour) => metricOf(hour, metric)));

  const grid = h("div", "grid hours");
  report.hours.forEach((hour, index) => {
    const weekday = Math.floor(index / 24);
    const clock = String(index % 24).padStart(2, "0");
    grid.append(
      cell(level(metricOf(hour, metric), max), `${WEEKDAYS[weekday]} ${clock}:00: ${describe(hour, metric, settings)}`),
    );
  });
  return grid;
}

type ReportView = Pick<ScanReport, "daily" | "hours" | "active_days" | "streak" | "days">;

function stats(report: ReportView, settings: Settings): HTMLElement {
  const metric = settings.heatmap_metric;
  const busiest: DayStat | undefined = [...report.daily].sort(
    (a, b) => metricOf(b, metric) - metricOf(a, metric),
  )[0];
  const total = report.daily.reduce((sum, day) => sum + metricOf(day, metric), 0);
  const average = report.days > 0 ? total / report.days : 0;
  const show = (value: number): string =>
    metric === "cost"
      ? formatMoney(value, settings.currency, settings.exchange_rate)
      : formatTokens(value);

  const item = (label: string, value: string, wide = false): HTMLElement =>
    h("div", `stat${wide ? " wide" : ""}`, h("span", "muted", label), h("span", "", value));

  return h(
    "div",
    "stats",
    item("Busiest day", busiest && metricOf(busiest, metric) > 0 ? `${busiest.date.slice(5)} · ${show(metricOf(busiest, metric))}` : "–", true),
    item("Active days", `${report.active_days} of ${report.days}`),
    item("Daily average", show(average)),
    item("Streak", `${report.streak} ${report.streak === 1 ? "day" : "days"}`),
  );
}

/** Activity heatmap with a day / hour-of-week switch. */
export function renderHeatmap(
  report: ScanReport | null,
  settings: Settings,
  rerender: () => void,
): HTMLElement | null {
  if (!report) return null;
  const mode = loadMode();

  const tab = (value: Mode, label: string): HTMLButtonElement => {
    const button = h("button", `tab${mode === value ? " active" : ""}`, label);
    button.type = "button";
    button.addEventListener("click", () => {
      saveMode(value);
      rerender();
    });
    return button;
  };

  const section = h("section", "heatmap");
  section.append(
    h("div", "heatmap-head", h("h2", "section-title", "Activity"), h("div", "tabs", tab("days", "By day"), tab("hours", "By hour"))),
    mode === "days" ? dayGrid(report, settings) : hourGrid(report, settings),
    stats(report, settings),
  );
  return section;
}
