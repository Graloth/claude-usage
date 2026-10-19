import { h } from "./dom";
import { formatMoney, formatTokens } from "./format";
import type { ScanSnapshot, Settings } from "./types";

const FAMILY_COLORS: Record<string, string> = {
  Fable: "#d97757",
  Opus: "#e6b428",
  Sonnet: "#6aa6e8",
  Haiku: "#7bc48a",
  Other: "#8a8a96",
};

function familyColor(name: string): string {
  return FAMILY_COLORS[name] ?? FAMILY_COLORS.Other;
}

/** What the last 30 days would have cost at API list prices, against the plan. */
export function renderValue(scan: ScanSnapshot, settings: Settings): HTMLElement | null {
  const report = scan.report;
  if (!report) {
    return scan.problem ? h("p", "muted", scan.problem) : null;
  }

  const money = (usd: number): string =>
    formatMoney(usd, settings.currency, settings.exchange_rate);
  const { totals } = report;

  const section = h("section", "value");
  section.append(h("h2", "section-title", `Value · last ${report.days} days`));

  const planPrice = settings.plan_price_usd ?? scan.plan?.price_usd ?? null;
  const planName = settings.plan_price_usd === null ? scan.plan?.name : "your plan";

  const headline = h("div", "value-headline");
  headline.append(
    h("span", "value-amount", money(totals.cost)),
    h("span", "muted", " at API prices"),
  );
  section.append(headline);

  if (planPrice !== null && planPrice > 0) {
    const ratio = totals.cost / planPrice;
    section.append(
      h(
        "p",
        "muted value-plan",
        `${planName ?? "Plan"} costs ${money(planPrice)} a month: ${ratio.toFixed(1)}× its price in API value.`,
      ),
    );
  }

  const total = report.families.reduce((sum, family) => sum + family.cost, 0);
  if (total > 0) {
    const bar = h("div", "stack-bar");
    const legend = h("ul", "legend");
    for (const family of report.families) {
      if (family.cost <= 0) continue;
      const slice = h("div", "slice");
      slice.style.setProperty("--w", String((family.cost / total) * 100));
      slice.style.setProperty("--c", familyColor(family.name));
      slice.title = `${family.name}: ${money(family.cost)}`;
      bar.append(slice);

      const swatch = h("span", "swatch");
      swatch.style.setProperty("--c", familyColor(family.name));
      legend.append(
        h("li", "", swatch, h("span", "", family.name), h("span", "muted", money(family.cost))),
      );
    }
    section.append(bar, legend);
  }

  section.append(
    h(
      "p",
      "muted tokens",
      `${formatTokens(totals.input)} in · ${formatTokens(totals.output)} out · ` +
        `${formatTokens(totals.cache_write)} cache write · ${formatTokens(totals.cache_read)} cache read`,
    ),
  );
  return section;
}
