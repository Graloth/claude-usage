import { h } from "./dom";
import { formatReset } from "./format";
import type { LimitView, Snapshot } from "./types";

function limitRow(limit: LimitView): HTMLLIElement {
  const fill = h("div", `fill ${limit.level}`);
  // CSSOM writes are allowed under the strict CSP; inline style attributes are not.
  fill.style.setProperty("--pct", String(Math.min(100, Math.max(0, limit.percent))));

  return h(
    "li",
    "",
    h(
      "div",
      "row",
      h("span", "", limit.label),
      h("span", "percent", `${Math.round(limit.percent)}%`),
    ),
    h("div", "bar", fill),
    h("div", "reset muted", formatReset(limit.resets_at)),
  );
}

/** The limits section: one bar per limit, greyed out while the data is stale. */
export function renderLimits(snapshot: Snapshot): HTMLElement {
  const stale = snapshot.problem !== null;
  if (snapshot.limits.length === 0) {
    return h("p", "muted", stale ? "" : "Loading…");
  }
  const list = h("ul", `limits${stale ? " stale" : ""}`);
  list.append(...snapshot.limits.map(limitRow));
  return list;
}
