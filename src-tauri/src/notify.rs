//! Decides when to notify about a limit. Pure state machine: the poller feeds
//! it each fresh snapshot and shows whatever comes back.
//!
//! Rules, ported from the GNOME extension:
//! - Warn once per limit window when usage reaches the threshold (and again
//!   after the limit's reset moves it to a new window).
//! - Say so when a limit that was at or above the threshold has reset.
//! - Never notify on the first observation of a limit, so starting the app
//!   with a nearly full limit does not nag.

use std::collections::HashMap;

use crate::{settings::Settings, snapshot::LimitView};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Default)]
pub struct NotifyState {
    /// Last seen (percent, resets_at) per limit.
    previous: HashMap<String, (f64, Option<String>)>,
    /// The window (`resets_at`) already warned about, per limit.
    warned: HashMap<String, Option<String>>,
}

impl NotifyState {
    /// Notifications to show for `limits`, which must come from a fresh fetch.
    pub fn observe(&mut self, limits: &[LimitView], settings: &Settings) -> Vec<Notification> {
        let threshold = f64::from(settings.warning_threshold);
        let mut out = Vec::new();

        for limit in limits {
            // `label` is unique per limit; `kind` is shared by all scoped weeklies.
            let key = limit.label.clone();
            let previous = self.previous.get(&key);

            if settings.notify_warning
                && limit.percent >= threshold
                && self.warned.get(&key) != Some(&limit.resets_at)
            {
                self.warned.insert(key.clone(), limit.resets_at.clone());
                if previous.is_some() {
                    out.push(Notification {
                        title: format!("{}: {:.0}%", limit.label, limit.percent),
                        body: if limit.percent >= 100.0 {
                            "Limit reached.".to_owned()
                        } else {
                            format!("{:.0}% used.", limit.percent)
                        },
                    });
                }
            }

            if settings.notify_reset
                && let Some((was, was_resets_at)) = previous
                && *was >= threshold
                && limit.percent < *was
                && *was_resets_at != limit.resets_at
            {
                out.push(Notification {
                    title: format!("{} has reset", limit.label),
                    body: format!("You're at {:.0}% again.", limit.percent),
                });
            }

            self.previous
                .insert(key, (limit.percent, limit.resets_at.clone()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Level;

    fn limit(label: &str, percent: f64, resets_at: &str) -> LimitView {
        LimitView {
            kind: "session".into(),
            label: label.into(),
            percent,
            resets_at: Some(resets_at.into()),
            level: Level::of(percent),
        }
    }

    fn titles(notes: &[Notification]) -> Vec<&str> {
        notes.iter().map(|n| n.title.as_str()).collect()
    }

    #[test]
    fn first_observation_never_notifies() {
        let mut state = NotifyState::default();
        let notes = state.observe(&[limit("Session", 95.0, "t1")], &Settings::default());
        assert!(notes.is_empty());
    }

    #[test]
    fn warns_once_per_window_when_crossing_the_threshold() {
        let mut state = NotifyState::default();
        let settings = Settings::default(); // threshold 80
        state.observe(&[limit("Session", 50.0, "t1")], &settings);

        let notes = state.observe(&[limit("Session", 85.0, "t1")], &settings);
        assert_eq!(titles(&notes), ["Session: 85%"]);
        assert_eq!(notes[0].body, "85% used.");

        // Still above the threshold in the same window: no repeat.
        assert!(
            state
                .observe(&[limit("Session", 90.0, "t1")], &settings)
                .is_empty()
        );
    }

    #[test]
    fn reaching_one_hundred_says_limit_reached() {
        let mut state = NotifyState::default();
        let settings = Settings::default();
        state.observe(&[limit("Session", 10.0, "t1")], &settings);
        let notes = state.observe(&[limit("Session", 100.0, "t1")], &settings);
        assert_eq!(notes[0].body, "Limit reached.");
    }

    #[test]
    fn announces_a_reset_and_warns_again_in_the_next_window() {
        let mut state = NotifyState::default();
        let settings = Settings::default();
        state.observe(&[limit("Session", 50.0, "t1")], &settings);
        state.observe(&[limit("Session", 95.0, "t1")], &settings);

        let notes = state.observe(&[limit("Session", 3.0, "t2")], &settings);
        assert_eq!(titles(&notes), ["Session has reset"]);

        // A new window: crossing the threshold again warns again.
        let notes = state.observe(&[limit("Session", 82.0, "t2")], &settings);
        assert_eq!(titles(&notes), ["Session: 82%"]);
    }

    #[test]
    fn low_usage_dropping_is_not_a_reset_announcement() {
        let mut state = NotifyState::default();
        let settings = Settings::default();
        state.observe(&[limit("Session", 40.0, "t1")], &settings);
        assert!(
            state
                .observe(&[limit("Session", 2.0, "t2")], &settings)
                .is_empty()
        );
    }

    #[test]
    fn switches_turn_each_kind_off() {
        let mut state = NotifyState::default();
        let settings = Settings {
            notify_warning: false,
            notify_reset: false,
            ..Settings::default()
        };
        state.observe(&[limit("Session", 50.0, "t1")], &settings);
        assert!(
            state
                .observe(&[limit("Session", 99.0, "t1")], &settings)
                .is_empty()
        );
        assert!(
            state
                .observe(&[limit("Session", 1.0, "t2")], &settings)
                .is_empty()
        );
    }

    #[test]
    fn limits_sharing_a_kind_are_tracked_separately() {
        let mut state = NotifyState::default();
        let settings = Settings::default();
        state.observe(
            &[
                limit("Weekly · Fable", 10.0, "t1"),
                limit("Weekly · Opus", 10.0, "t1"),
            ],
            &settings,
        );
        let notes = state.observe(
            &[
                limit("Weekly · Fable", 90.0, "t1"),
                limit("Weekly · Opus", 10.0, "t1"),
            ],
            &settings,
        );
        assert_eq!(titles(&notes), ["Weekly · Fable: 90%"]);
    }
}
