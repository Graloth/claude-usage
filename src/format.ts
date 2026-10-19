import type { Currency } from "./types";

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
export function formatReset(iso: string | null): string {
  if (!iso) return "";
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  const withinDay = at.getTime() - Date.now() < 24 * 3600 * 1000;
  return `resets ${(withinDay ? clock : dayAndClock).format(at)}`;
}

export function formatAge(ms: number | null): string {
  if (ms === null) return "";
  const minutes = Math.floor((Date.now() - ms) / 60_000);
  if (minutes < 1) return "Updated just now";
  if (minutes < 60) return `Updated ${minutes} min ago`;
  return `Updated ${Math.floor(minutes / 60)} h ${minutes % 60} min ago`;
}

/** Money in the user's currency; `usd` is converted with the user's rate. */
export function formatMoney(usd: number, currency: Currency, rate: number): string {
  const value = usd * rate;
  return new Intl.NumberFormat(undefined, {
    style: "currency",
    currency,
    // "$" rather than "US$" in non-US locales.
    currencyDisplay: "narrowSymbol",
    // Cents matter for small amounts, not for hundreds.
    maximumFractionDigits: Math.abs(value) >= 100 ? 0 : 2,
  }).format(value);
}

/** 1234 -> "1.2k", 5_600_000 -> "5.6M" */
export function formatTokens(count: number): string {
  if (count >= 1e9) return `${(count / 1e9).toFixed(1)}B`;
  if (count >= 1e6) return `${(count / 1e6).toFixed(1)}M`;
  if (count >= 1e3) return `${(count / 1e3).toFixed(1)}k`;
  return String(Math.round(count));
}
