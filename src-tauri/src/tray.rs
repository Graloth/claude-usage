//! Maps the current [`Snapshot`] onto the tray icon, title and tooltip.

use tauri::{AppHandle, image::Image};

use crate::{
    ring::{self, Rgb},
    snapshot::{Level, Snapshot},
};

pub const TRAY_ID: &str = "main";

const ORANGE: Rgb = Rgb(217, 119, 87);
const YELLOW: Rgb = Rgb(230, 180, 40);
const RED: Rgb = Rgb(220, 60, 60);
/// Stale or unknown: the ring is greyed out.
const GREY: Rgb = Rgb(140, 140, 140);

/// Everything the tray shows, decided without touching Tauri.
#[derive(Debug, PartialEq)]
pub struct TrayView {
    pub fraction: f32,
    pub color: Rgb,
    /// Text beside the icon where the platform supports it (macOS, some Linux
    /// desktops).
    pub title: Option<String>,
    pub tooltip: String,
}

pub fn view(snapshot: &Snapshot) -> TrayView {
    let Some(headline) = snapshot.headline() else {
        return TrayView {
            fraction: 0.0,
            color: GREY,
            title: None,
            tooltip: snapshot.problem.as_ref().map_or_else(
                || "Claude usage: loading…".to_owned(),
                |problem| format!("Claude usage: {}", problem.message),
            ),
        };
    };

    let color = match (snapshot.is_stale(), headline.level) {
        (true, _) => GREY,
        (false, Level::Normal) => ORANGE,
        (false, Level::Warning) => YELLOW,
        (false, Level::Critical) => RED,
    };
    let mut tooltip = format!("{}: {:.0}%", headline.label, headline.percent);
    if let Some(problem) = &snapshot.problem {
        tooltip.push_str(&format!("\nLast known value. {}", problem.message));
    }
    TrayView {
        fraction: (headline.percent / 100.0) as f32,
        color,
        title: Some(format!("{:.0}%", headline.percent)),
        tooltip,
    }
}

pub fn icon(view: &TrayView) -> Image<'static> {
    Image::new_owned(
        ring::render(view.fraction, view.color),
        ring::SIZE,
        ring::SIZE,
    )
}

/// Updates the tray to match `snapshot`. Failures are logged, not fatal: a
/// stale tray is better than a crashed poller.
pub fn apply(app: &AppHandle, snapshot: &Snapshot) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let view = view(snapshot);
    let results = [
        tray.set_icon(Some(icon(&view))),
        tray.set_tooltip(Some(&view.tooltip)),
        tray.set_title(view.title.as_deref()),
    ];
    for error in results.into_iter().filter_map(Result::err) {
        eprintln!("claude-usage: could not update tray: {error}");
    }
}

#[cfg(test)]
mod tests {
    use claude_usage_core::usage::Limit;

    use super::*;
    use crate::snapshot::Problem;

    fn limit(percent: f64) -> Limit {
        serde_json::from_value(serde_json::json!({"kind": "session", "percent": percent})).unwrap()
    }

    #[test]
    fn fresh_data_is_coloured_by_level() {
        let colour_at = |percent| {
            let mut snapshot = Snapshot::default();
            snapshot.record_success(&[limit(percent)], 0);
            view(&snapshot).color
        };
        assert_eq!(colour_at(10.0), ORANGE);
        assert_eq!(colour_at(80.0), YELLOW);
        assert_eq!(colour_at(95.0), RED);
    }

    #[test]
    fn stale_data_keeps_the_value_but_greys_the_ring() {
        let mut snapshot = Snapshot::default();
        snapshot.record_success(&[limit(95.0)], 0);
        snapshot.record_problem(Problem::expired());

        let view = view(&snapshot);
        assert_eq!(view.color, GREY);
        assert_eq!(view.title.as_deref(), Some("95%"));
        assert!((view.fraction - 0.95).abs() < 1e-6);
        assert!(view.tooltip.contains("Last known value"));
        assert!(view.tooltip.contains("renew"));
    }

    #[test]
    fn no_data_shows_an_empty_grey_ring_with_the_reason() {
        let mut snapshot = Snapshot::default();
        assert!(view(&snapshot).tooltip.contains("loading"));

        snapshot.record_problem(Problem::expired());
        let view = view(&snapshot);
        assert_eq!(view.color, GREY);
        assert_eq!(view.title, None);
        assert_eq!(view.fraction, 0.0);
        assert!(view.tooltip.contains("expired"));
    }
}
