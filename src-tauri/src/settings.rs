//! User preferences, stored as JSON in the app's config directory.
//!
//! Loading never fails: a missing or unreadable file means defaults, and every
//! value is clamped into its valid range so a hand-edited file cannot break the
//! poller (for example a zero refresh interval).

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

/// Least time between refreshes. The usage endpoint rate-limits hard.
pub const MIN_REFRESH_MINUTES: u32 = 2;
pub const MAX_REFRESH_MINUTES: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayLimit {
    MostConstrained,
    Session,
    Weekly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeatmapMetric {
    Cost,
    Tokens,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Currency {
    Usd,
    Eur,
    Gbp,
    Dkk,
    Sek,
    Nok,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Limits,
    Value,
    Heatmap,
}

impl Section {
    const ALL: [Self; 3] = [Self::Limits, Self::Value, Self::Heatmap];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionSetting {
    pub section: Section,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub refresh_minutes: u32,
    pub tray_limit: TrayLimit,
    pub show_tray_percent: bool,
    pub notify_warning: bool,
    /// Percent of a limit at which to warn.
    pub warning_threshold: u8,
    pub notify_reset: bool,
    pub currency: Currency,
    /// How many `currency` units one US dollar buys. Rates change, so the user
    /// supplies it.
    pub exchange_rate: f64,
    pub heatmap_metric: HeatmapMetric,
    /// Monthly plan price in USD, overriding the detected plan (annual or
    /// negotiated billing).
    pub plan_price_usd: Option<f64>,
    /// Comma-separated Claude config directories, overriding autodetection.
    pub claude_config_dir: Option<String>,
    /// Popup sections, in display order.
    pub sections: Vec<SectionSetting>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            refresh_minutes: 5,
            tray_limit: TrayLimit::MostConstrained,
            show_tray_percent: true,
            notify_warning: true,
            warning_threshold: 80,
            notify_reset: true,
            currency: Currency::Usd,
            exchange_rate: 1.0,
            heatmap_metric: HeatmapMetric::Cost,
            plan_price_usd: None,
            claude_config_dir: None,
            sections: Section::ALL
                .map(|section| SectionSetting {
                    section,
                    enabled: true,
                })
                .to_vec(),
        }
    }
}

impl Settings {
    /// Forces every value into its valid range.
    pub fn sanitised(mut self) -> Self {
        self.refresh_minutes = self
            .refresh_minutes
            .clamp(MIN_REFRESH_MINUTES, MAX_REFRESH_MINUTES);
        self.warning_threshold = self.warning_threshold.clamp(1, 100);
        if !(self.exchange_rate.is_finite() && self.exchange_rate > 0.0) {
            self.exchange_rate = 1.0;
        }
        self.plan_price_usd = self
            .plan_price_usd
            .filter(|price| price.is_finite() && *price >= 0.0);
        self.claude_config_dir = self
            .claude_config_dir
            .map(|dir| dir.trim().to_owned())
            .filter(|dir| !dir.is_empty());

        // Keep the first entry of each section, then add any that are missing
        // (a newer version may have introduced one) in the default order.
        let mut sections: Vec<SectionSetting> = Vec::new();
        for entry in self.sections.drain(..) {
            if sections.iter().all(|kept| kept.section != entry.section) {
                sections.push(entry);
            }
        }
        for section in Section::ALL {
            if sections.iter().all(|kept| kept.section != section) {
                sections.push(SectionSetting {
                    section,
                    enabled: true,
                });
            }
        }
        self.sections = sections;
        self
    }

    pub fn section_enabled(&self, section: Section) -> bool {
        self.sections
            .iter()
            .any(|entry| entry.section == section && entry.enabled)
    }
}

/// Loads settings from `path`; anything wrong yields defaults.
pub fn load(path: &Path) -> Settings {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Settings>(&raw).ok())
        .unwrap_or_default()
        .sanitised()
}

/// Writes settings atomically so a crash cannot leave a half-written file.
pub fn save(path: &Path, settings: &Settings) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    fs::rename(&tmp, path)
}

/// Settings shared across threads, persisted on every change.
pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsStore {
    pub fn load(path: PathBuf) -> Self {
        let current = Mutex::new(load(&path));
        Self { path, current }
    }

    pub fn get(&self) -> Settings {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Validates, stores and persists `settings`; returns what was stored.
    pub fn set(&self, settings: Settings) -> io::Result<Settings> {
        let settings = settings.sanitised();
        save(&self.path, &settings)?;
        *self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = settings.clone();
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "claude-usage-settings-{test}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir.join("settings.json")
    }

    #[test]
    fn missing_or_corrupt_file_gives_defaults() {
        let path = scratch("missing");
        assert_eq!(load(&path), Settings::default());

        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not json").unwrap();
        assert_eq!(load(&path), Settings::default());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn partial_file_keeps_defaults_for_the_rest() {
        let path = scratch("partial");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"currency":"DKK","exchange_rate":7.0}"#).unwrap();
        let settings = load(&path);
        assert_eq!(settings.currency, Currency::Dkk);
        assert_eq!(settings.exchange_rate, 7.0);
        assert_eq!(settings.refresh_minutes, 5);
        assert_eq!(settings.sections.len(), 3);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let settings = Settings {
            refresh_minutes: 0,
            warning_threshold: 250,
            exchange_rate: f64::NAN,
            plan_price_usd: Some(-5.0),
            claude_config_dir: Some("   ".into()),
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(settings.refresh_minutes, MIN_REFRESH_MINUTES);
        assert_eq!(settings.warning_threshold, 100);
        assert_eq!(settings.exchange_rate, 1.0);
        assert_eq!(settings.plan_price_usd, None);
        assert_eq!(settings.claude_config_dir, None);

        let slow = Settings {
            refresh_minutes: 10_000,
            ..Settings::default()
        }
        .sanitised();
        assert_eq!(slow.refresh_minutes, MAX_REFRESH_MINUTES);
    }

    #[test]
    fn sections_are_deduplicated_and_completed() {
        let settings = Settings {
            sections: vec![
                SectionSetting {
                    section: Section::Heatmap,
                    enabled: true,
                },
                SectionSetting {
                    section: Section::Heatmap,
                    enabled: false,
                },
                SectionSetting {
                    section: Section::Limits,
                    enabled: false,
                },
            ],
            ..Settings::default()
        }
        .sanitised();
        let order: Vec<_> = settings.sections.iter().map(|s| s.section).collect();
        assert_eq!(order, [Section::Heatmap, Section::Limits, Section::Value]);
        assert!(settings.section_enabled(Section::Heatmap));
        assert!(!settings.section_enabled(Section::Limits));
        assert!(settings.section_enabled(Section::Value));
    }

    #[test]
    fn store_persists_and_reloads() {
        let path = scratch("store");
        let store = SettingsStore::load(path.clone());
        let stored = store
            .set(Settings {
                currency: Currency::Eur,
                refresh_minutes: 1,
                ..Settings::default()
            })
            .unwrap();
        assert_eq!(stored.refresh_minutes, MIN_REFRESH_MINUTES);

        let reloaded = SettingsStore::load(path.clone());
        assert_eq!(reloaded.get().currency, Currency::Eur);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn currency_serialises_as_iso_code() {
        assert_eq!(serde_json::to_string(&Currency::Dkk).unwrap(), "\"DKK\"");
    }
}
