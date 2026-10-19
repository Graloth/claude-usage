import { invoke } from "@tauri-apps/api/core";

import { element, h } from "./dom";
import type {
  Currency,
  HeatmapMetric,
  SectionName,
  SectionSetting,
  Settings,
  TrayLimit,
} from "./types";

const SECTION_LABELS: Record<SectionName, string> = {
  limits: "Limits",
  value: "Value",
  heatmap: "Activity heatmap",
};

const form = element<HTMLFormElement>("settings-view");
const configDir = element<HTMLInputElement>("s-config-dir");
const planPrice = element<HTMLInputElement>("s-plan-price");
const refresh = element<HTMLInputElement>("s-refresh");
const trayLimit = element<HTMLSelectElement>("s-tray-limit");
const trayPercent = element<HTMLInputElement>("s-tray-percent");
const sectionList = element<HTMLUListElement>("s-sections");
const currency = element<HTMLSelectElement>("s-currency");
const rate = element<HTMLInputElement>("s-rate");
const rateHint = element("s-rate-hint");
const metric = element<HTMLSelectElement>("s-metric");
const notifyWarning = element<HTMLInputElement>("s-notify-warning");
const threshold = element<HTMLInputElement>("s-threshold");
const notifyReset = element<HTMLInputElement>("s-notify-reset");

let sections: SectionSetting[] = [];

function populate(settings: Settings): void {
  configDir.value = settings.claude_config_dir ?? "";
  planPrice.value = settings.plan_price_usd === null ? "" : String(settings.plan_price_usd);
  refresh.value = String(settings.refresh_minutes);
  trayLimit.value = settings.tray_limit;
  trayPercent.checked = settings.show_tray_percent;
  currency.value = settings.currency;
  rate.value = String(settings.exchange_rate);
  metric.value = settings.heatmap_metric;
  notifyWarning.checked = settings.notify_warning;
  threshold.value = String(settings.warning_threshold);
  notifyReset.checked = settings.notify_reset;
  sections = settings.sections.map((entry) => ({ ...entry }));
  renderSections();
  renderRateHint(settings);
}

/** A non-dollar currency at the default rate of 1 is almost certainly unset. */
function renderRateHint(settings: Settings): void {
  rateHint.textContent =
    settings.currency !== "USD" && settings.exchange_rate === 1
      ? `Set how many ${settings.currency} one US dollar buys; costs are in USD until you do.`
      : "";
}

function collect(): Settings {
  return {
    claude_config_dir: configDir.value.trim() || null,
    plan_price_usd: planPrice.value === "" ? null : Number(planPrice.value),
    refresh_minutes: Number(refresh.value) || 5,
    tray_limit: trayLimit.value as TrayLimit,
    show_tray_percent: trayPercent.checked,
    currency: currency.value as Currency,
    exchange_rate: Number(rate.value) || 1,
    heatmap_metric: metric.value as HeatmapMetric,
    notify_warning: notifyWarning.checked,
    warning_threshold: Number(threshold.value) || 80,
    notify_reset: notifyReset.checked,
    sections: sections.map((entry) => ({ ...entry })),
  };
}

function renderSections(): void {
  sectionList.replaceChildren(
    ...sections.map((entry, index) => {
      const toggle = h("input");
      toggle.type = "checkbox";
      toggle.checked = entry.enabled;
      toggle.addEventListener("change", () => {
        entry.enabled = toggle.checked;
      });

      const move = (label: string, delta: number): HTMLButtonElement => {
        const button = h("button", "move", label);
        button.type = "button";
        button.disabled = index + delta < 0 || index + delta >= sections.length;
        button.setAttribute("aria-label", `Move ${SECTION_LABELS[entry.section]} ${delta < 0 ? "up" : "down"}`);
        button.addEventListener("click", () => {
          const [moved] = sections.splice(index, 1);
          sections.splice(index + delta, 0, moved);
          renderSections();
          form.dispatchEvent(new Event("change", { bubbles: true }));
        });
        return button;
      };

      return h(
        "li",
        "section-row",
        h("label", "check", toggle, SECTION_LABELS[entry.section]),
        h("span", "move-buttons", move("↑", -1), move("↓", 1)),
      );
    }),
  );
}

/**
 * Wires the settings form: it saves on every change and shows what the Rust
 * side actually stored (values are clamped there).
 *
 * @param onSaved called with the stored settings after each save
 */
export async function initSettings(onSaved: (settings: Settings) => void): Promise<Settings> {
  const initial = await invoke<Settings>("get_settings");
  populate(initial);

  form.addEventListener("submit", (event) => event.preventDefault());
  form.addEventListener("change", () => {
    void invoke<Settings>("save_settings", { settings: collect() })
      .then((saved) => {
        populate(saved);
        onSaved(saved);
      })
      .catch((error: unknown) => {
        rateHint.textContent = `Could not save settings: ${String(error)}`;
      });
  });
  return initial;
}
