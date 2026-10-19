//! What the app currently knows about the user's limits, and why it may be
//! out of date. Pure data and rules: no Tauri, no IO, so it is unit-tested.

use claude_usage_core::{
    credentials::CredentialsError,
    usage::{Limit, UsageError},
};
use serde::Serialize;

use crate::settings::TrayLimit;

/// Percent of a limit at which the ring turns yellow / red.
pub const WARNING_PERCENT: f64 = 75.0;
pub const CRITICAL_PERCENT: f64 = 90.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Normal,
    Warning,
    Critical,
}

impl Level {
    pub fn of(percent: f64) -> Self {
        if percent >= CRITICAL_PERCENT {
            Self::Critical
        } else if percent >= WARNING_PERCENT {
            Self::Warning
        } else {
            Self::Normal
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LimitView {
    pub kind: String,
    pub label: String,
    pub percent: f64,
    /// RFC 3339; formatted for the user's locale by the frontend.
    pub resets_at: Option<String>,
    pub level: Level,
}

impl LimitView {
    pub fn from_limit(limit: &Limit) -> Self {
        Self {
            kind: limit.kind.clone(),
            label: label_for(limit),
            percent: limit.percent,
            resets_at: limit.resets_at.clone(),
            level: Level::of(limit.percent),
        }
    }
}

fn label_for(limit: &Limit) -> String {
    match (limit.kind.as_str(), limit.scope_label()) {
        ("session", _) => "Current session".into(),
        ("weekly_all", _) => "Weekly · all models".into(),
        ("weekly_opus", _) => "Weekly · Opus".into(),
        ("weekly_sonnet", _) => "Weekly · Sonnet".into(),
        (kind, Some(scope)) => {
            let head = kind.split('_').next().unwrap_or(kind);
            format!("{} · {scope}", capitalise(head))
        }
        (kind, None) => capitalise(&kind.replace('_', " ")),
    }
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemKind {
    NoLogin,
    Expired,
    Rejected,
    RateLimited,
    Offline,
    Other,
}

/// Why the last refresh did not produce fresh data.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Problem {
    pub kind: ProblemKind,
    /// Ready to show to the user.
    pub message: String,
}

impl Problem {
    fn new(kind: ProblemKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn expired() -> Self {
        Self::new(
            ProblemKind::Expired,
            "Claude Code's sign-in has expired. Use Claude Code (run `claude`) to renew it.",
        )
    }

    pub fn from_credentials(error: &CredentialsError) -> Self {
        match error {
            CredentialsError::NotFound { .. } => Self::new(
                ProblemKind::NoLogin,
                "Not signed in to Claude Code. Run `claude` to sign in.",
            ),
            CredentialsError::Unreadable { .. } => Self::new(ProblemKind::Other, error.to_string()),
        }
    }

    pub fn from_usage(error: &UsageError) -> Self {
        match error {
            UsageError::Unauthorized => Self::new(
                ProblemKind::Rejected,
                "Claude rejected the stored sign-in. Run `claude` to renew it.",
            ),
            UsageError::RateLimited { .. } => Self::new(
                ProblemKind::RateLimited,
                "Rate limited by Claude. Retrying automatically.",
            ),
            UsageError::Transport(_) => Self::new(
                ProblemKind::Offline,
                "Can't reach Claude. Retrying automatically.",
            ),
            UsageError::Status(_) | UsageError::Parse(_) => {
                Self::new(ProblemKind::Other, error.to_string())
            }
        }
    }
}

/// Last known limits plus the reason they may be stale. A failed refresh keeps
/// the old limits so the UI can show them greyed out instead of going blank.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub limits: Vec<LimitView>,
    /// When `limits` was fetched, Unix epoch milliseconds.
    pub updated_at_ms: Option<i64>,
    pub problem: Option<Problem>,
}

impl Snapshot {
    pub fn record_success(&mut self, limits: &[Limit], now_ms: i64) {
        self.limits = limits.iter().map(LimitView::from_limit).collect();
        self.updated_at_ms = Some(now_ms);
        self.problem = None;
    }

    pub fn record_problem(&mut self, problem: Problem) {
        self.problem = Some(problem);
    }

    pub fn is_stale(&self) -> bool {
        self.problem.is_some()
    }

    /// The most constrained limit.
    pub fn headline(&self) -> Option<&LimitView> {
        self.limits
            .iter()
            .max_by(|a, b| a.percent.total_cmp(&b.percent))
    }

    /// The limit the tray ring shows, per the user's choice. Falls back to the
    /// most constrained one when the chosen limit is not in the data.
    pub fn tray_limit(&self, which: TrayLimit) -> Option<&LimitView> {
        let chosen = match which {
            TrayLimit::MostConstrained => None,
            TrayLimit::Session => self.limits.iter().find(|l| l.kind == "session"),
            TrayLimit::Weekly => self
                .limits
                .iter()
                .find(|l| l.kind == "weekly_all")
                .or_else(|| self.limits.iter().find(|l| l.kind.starts_with("weekly"))),
        };
        chosen.or_else(|| self.headline())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(kind: &str, percent: f64, scope: Option<&str>) -> Limit {
        serde_json::from_value(serde_json::json!({
            "kind": kind,
            "percent": percent,
            "resets_at": "2026-10-19T11:30:00+00:00",
            "scope": scope.map(|s| serde_json::json!({"model": {"display_name": s}})),
        }))
        .unwrap()
    }

    #[test]
    fn level_thresholds() {
        assert_eq!(Level::of(0.0), Level::Normal);
        assert_eq!(Level::of(74.9), Level::Normal);
        assert_eq!(Level::of(75.0), Level::Warning);
        assert_eq!(Level::of(89.9), Level::Warning);
        assert_eq!(Level::of(90.0), Level::Critical);
        assert_eq!(Level::of(100.0), Level::Critical);
    }

    #[test]
    fn labels() {
        let label = |kind, scope| LimitView::from_limit(&limit(kind, 1.0, scope)).label;
        assert_eq!(label("session", None), "Current session");
        assert_eq!(label("weekly_all", None), "Weekly · all models");
        assert_eq!(label("weekly_scoped", Some("Fable")), "Weekly · Fable");
        assert_eq!(label("monthly_credit_pool", None), "Monthly credit pool");
    }

    #[test]
    fn headline_is_the_most_constrained_limit() {
        let mut snapshot = Snapshot::default();
        assert!(snapshot.headline().is_none());
        snapshot.record_success(
            &[
                limit("session", 51.0, None),
                limit("weekly_all", 90.0, None),
                limit("weekly_scoped", 17.0, Some("Fable")),
            ],
            1,
        );
        assert_eq!(snapshot.headline().unwrap().kind, "weekly_all");
    }

    #[test]
    fn tray_limit_follows_the_choice_and_falls_back() {
        let mut snapshot = Snapshot::default();
        snapshot.record_success(
            &[
                limit("session", 51.0, None),
                limit("weekly_all", 90.0, None),
            ],
            1,
        );
        let kind = |which| snapshot.tray_limit(which).unwrap().kind.clone();
        assert_eq!(kind(TrayLimit::MostConstrained), "weekly_all");
        assert_eq!(kind(TrayLimit::Session), "session");
        assert_eq!(kind(TrayLimit::Weekly), "weekly_all");

        let mut only_session = Snapshot::default();
        only_session.record_success(&[limit("session", 5.0, None)], 1);
        assert_eq!(
            only_session.tray_limit(TrayLimit::Weekly).unwrap().kind,
            "session"
        );
    }

    #[test]
    fn a_problem_keeps_the_last_known_limits() {
        let mut snapshot = Snapshot::default();
        snapshot.record_success(&[limit("session", 51.0, None)], 42);
        assert!(!snapshot.is_stale());

        snapshot.record_problem(Problem::expired());
        assert!(snapshot.is_stale());
        assert_eq!(snapshot.limits.len(), 1);
        assert_eq!(snapshot.updated_at_ms, Some(42));

        snapshot.record_success(&[limit("session", 60.0, None)], 100);
        assert!(!snapshot.is_stale());
        assert_eq!(snapshot.headline().unwrap().percent, 60.0);
    }

    #[test]
    fn problems_map_to_user_facing_kinds() {
        let kind = |e: UsageError| Problem::from_usage(&e).kind;
        assert_eq!(kind(UsageError::Unauthorized), ProblemKind::Rejected);
        assert_eq!(
            kind(UsageError::RateLimited { retry_after: None }),
            ProblemKind::RateLimited
        );
        assert_eq!(
            kind(UsageError::Transport("dns".into())),
            ProblemKind::Offline
        );
        assert_eq!(kind(UsageError::Status(500)), ProblemKind::Other);

        let not_found = CredentialsError::NotFound { tried: vec![] };
        assert_eq!(
            Problem::from_credentials(&not_found).kind,
            ProblemKind::NoLogin
        );
    }
}
