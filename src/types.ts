// Mirrors the serialised Rust types in src-tauri/src/{snapshot,settings}.rs.

export type Level = "normal" | "warning" | "critical";

export interface LimitView {
  kind: string;
  label: string;
  percent: number;
  /** RFC 3339 */
  resets_at: string | null;
  level: Level;
}

export interface Snapshot {
  limits: LimitView[];
  /** Unix epoch, milliseconds */
  updated_at_ms: number | null;
  problem: { kind: string; message: string } | null;
}

export type TrayLimit = "most_constrained" | "session" | "weekly";
export type HeatmapMetric = "cost" | "tokens";
export type Currency = "USD" | "EUR" | "GBP" | "DKK" | "SEK" | "NOK";
export type SectionName = "limits" | "value" | "heatmap";

export interface SectionSetting {
  section: SectionName;
  enabled: boolean;
}

export interface Settings {
  refresh_minutes: number;
  tray_limit: TrayLimit;
  show_tray_percent: boolean;
  notify_warning: boolean;
  warning_threshold: number;
  notify_reset: boolean;
  currency: Currency;
  exchange_rate: number;
  heatmap_metric: HeatmapMetric;
  plan_price_usd: number | null;
  claude_config_dir: string | null;
  sections: SectionSetting[];
}

// Mirrors `claude_usage_core::scanner::ScanReport` and the app's `ScanSnapshot`
// (src-tauri/src/scan.rs).

export interface Totals {
  cost: number;
  input: number;
  output: number;
  cache_write: number;
  cache_read: number;
  messages: number;
}

export interface DayStat {
  /** YYYY-MM-DD, local time */
  date: string;
  cost: number;
  tokens: number;
  messages: number;
}

export interface HourStat {
  cost: number;
  tokens: number;
}

export interface ScanReport {
  generated_at_ms: number;
  days: number;
  totals: Totals;
  /** Cost per model family, most expensive first. */
  families: { name: string; cost: number }[];
  daily: DayStat[];
  /** 168 entries, Monday first: index = weekday * 24 + hour. */
  hours: HourStat[];
  active_days: number;
  streak: number;
  files_scanned: number;
  warnings: string[];
}

export interface PlanInfo {
  name: string;
  /** Monthly price in USD. */
  price_usd: number;
}

export interface ScanSnapshot {
  report: ScanReport | null;
  plan: PlanInfo | null;
  /** Why there is no report, if there is none. */
  problem: string | null;
}
