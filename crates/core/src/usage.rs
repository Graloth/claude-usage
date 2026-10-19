//! Client for the claude.ai OAuth usage endpoint.
//!
//! The endpoint is undocumented and rate-limits aggressively, so callers
//! should poll sparingly and honour [`UsageError::RateLimited`].

use std::time::Duration;

use serde::Deserialize;

use crate::credentials::OAuthCredentials;

const API_BASE: &str = "https://api.anthropic.com/api/oauth";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    /// 401/403: the token is expired or revoked. Claude Code renews it while
    /// it runs; this client never refreshes.
    #[error(
        "the Claude Code login was rejected (token expired or revoked); run `claude` to renew it"
    )]
    Unauthorized,
    #[error("rate limited by the usage endpoint{}", retry_hint(.retry_after))]
    RateLimited { retry_after: Option<Duration> },
    #[error("usage endpoint returned HTTP {0}")]
    Status(u16),
    #[error("could not reach the usage endpoint: {0}")]
    Transport(String),
    #[error("unexpected usage response: {0}")]
    Parse(String),
}

fn retry_hint(retry_after: &Option<Duration>) -> String {
    retry_after.map_or_else(String::new, |d| format!(" (retry in {}s)", d.as_secs()))
}

/// One usage limit, normalised from either response shape.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Limit {
    /// `session`, `weekly_all`, `weekly_scoped`, ...
    pub kind: String,
    pub group: Option<String>,
    /// 0 to 100, can exceed 100 only through rounding.
    pub percent: f64,
    /// `normal`, `warning`, ... as reported by the server.
    pub severity: Option<String>,
    /// RFC 3339 timestamp.
    pub resets_at: Option<String>,
    pub scope: Option<Scope>,
    #[serde(default)]
    pub is_active: bool,
}

impl Limit {
    /// Human label for the scope, such as the model a weekly limit applies to.
    pub fn scope_label(&self) -> Option<&str> {
        match self.scope.as_ref()? {
            Scope::Text(text) => Some(text),
            Scope::Detailed { model, .. } => model.as_ref()?.display_name.as_deref(),
        }
    }
}

/// Older servers send the scope as a plain string, newer ones as an object.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Scope {
    Text(String),
    Detailed {
        model: Option<ModelScope>,
        surface: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelScope {
    pub id: Option<String>,
    pub display_name: Option<String>,
}

/// The pre-`limits` response shape, still returned alongside the new one.
#[derive(Debug, Clone, Deserialize)]
struct Window {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

/// Raw response: only what the app uses, everything else is ignored.
#[derive(Debug, Deserialize)]
pub struct Usage {
    #[serde(default)]
    limits: Vec<Limit>,
    five_hour: Option<Window>,
    seven_day: Option<Window>,
    seven_day_opus: Option<Window>,
    seven_day_sonnet: Option<Window>,
}

impl Usage {
    /// The limits to show: the `limits` array when present, otherwise the
    /// legacy per-window fields mapped to the same kinds.
    pub fn limits(&self) -> Vec<Limit> {
        if !self.limits.is_empty() {
            return self.limits.clone();
        }
        [
            ("session", "session", &self.five_hour),
            ("weekly_all", "weekly", &self.seven_day),
            ("weekly_opus", "weekly", &self.seven_day_opus),
            ("weekly_sonnet", "weekly", &self.seven_day_sonnet),
        ]
        .into_iter()
        .filter_map(|(kind, group, window)| {
            let window = window.as_ref()?;
            Some(Limit {
                kind: kind.to_owned(),
                group: Some(group.to_owned()),
                percent: window.utilization.unwrap_or(0.0),
                severity: None,
                resets_at: window.resets_at.clone(),
                scope: None,
                is_active: false,
            })
        })
        .collect()
    }
}

/// The `/profile` response, reduced to what plan detection needs. Names,
/// emails and ids are deliberately not modelled, so they are never read.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Profile {
    pub account: Option<ProfileAccount>,
    pub organization: Option<ProfileOrganization>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ProfileAccount {
    pub has_claude_max: Option<bool>,
    pub has_claude_pro: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ProfileOrganization {
    /// For example `claude_team`.
    pub organization_type: Option<String>,
    /// For example `default_claude_max_5x`.
    pub rate_limit_tier: Option<String>,
    /// For example `team_tier_1`.
    pub seat_tier: Option<String>,
}

pub struct Client {
    agent: ureq::Agent,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            // Handle statuses ourselves to tell 401 and 429 apart.
            .http_status_as_error(false)
            .user_agent(concat!("claude-usage/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { agent }
    }

    /// Blocking; call from a worker thread, not the UI thread.
    pub fn fetch_usage(&self, creds: &OAuthCredentials) -> Result<Usage, UsageError> {
        self.get("usage", creds)
    }

    /// The account and organisation behind the login, for plan detection.
    /// Blocking, like [`Client::fetch_usage`].
    pub fn fetch_profile(&self, creds: &OAuthCredentials) -> Result<Profile, UsageError> {
        self.get("profile", creds)
    }

    fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        creds: &OAuthCredentials,
    ) -> Result<T, UsageError> {
        let mut response = self
            .agent
            .get(format!("{API_BASE}/{path}"))
            .header("Authorization", format!("Bearer {}", creds.access_token()))
            .header("anthropic-beta", OAUTH_BETA)
            .header("Accept", "application/json")
            .call()
            .map_err(|e| UsageError::Transport(e.to_string()))?;

        match response.status().as_u16() {
            200 => response
                .body_mut()
                .read_json()
                .map_err(|e| UsageError::Parse(e.to_string())),
            401 | 403 => Err(UsageError::Unauthorized),
            429 => Err(UsageError::RateLimited {
                retry_after: response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse().ok())
                    .map(Duration::from_secs),
            }),
            other => Err(UsageError::Status(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from a real response (2026-10-19): the new `limits` array with
    // an object scope, next to the legacy windows and many unknown fields.
    const CURRENT: &str = r#"{
        "five_hour": {"utilization": 50.0, "resets_at": "2026-10-19T11:30:00+00:00"},
        "seven_day": {"utilization": 22.0, "resets_at": "2026-10-22T06:00:00+00:00"},
        "seven_day_opus": null,
        "tangelo": null,
        "extra_usage": {"is_enabled": false},
        "limits": [
            {"kind": "session", "group": "session", "percent": 50, "severity": "normal",
             "resets_at": "2026-10-19T11:30:00+00:00", "scope": null, "is_active": true},
            {"kind": "weekly_scoped", "group": "weekly", "percent": 17, "severity": "normal",
             "resets_at": "2026-10-22T06:00:00+00:00",
             "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null},
             "is_active": false}
        ]
    }"#;

    #[test]
    fn prefers_limits_array_and_reads_object_scope() {
        let usage: Usage = serde_json::from_str(CURRENT).unwrap();
        let limits = usage.limits();
        assert_eq!(limits.len(), 2);
        assert_eq!(limits[0].kind, "session");
        assert_eq!(limits[0].percent, 50.0);
        assert!(limits[0].is_active);
        assert_eq!(limits[0].scope_label(), None);
        assert_eq!(limits[1].scope_label(), Some("Fable"));
    }

    #[test]
    fn accepts_string_scope() {
        let usage: Usage = serde_json::from_str(
            r#"{"limits":[{"kind":"weekly_scoped","percent":3.5,"scope":"opus"}]}"#,
        )
        .unwrap();
        assert_eq!(usage.limits()[0].scope_label(), Some("opus"));
        assert_eq!(usage.limits()[0].percent, 3.5);
    }

    #[test]
    fn falls_back_to_legacy_windows() {
        let usage: Usage = serde_json::from_str(
            r#"{"five_hour":{"utilization":12.0,"resets_at":"2026-01-01T00:00:00+00:00"},
                "seven_day":{"utilization":40.0,"resets_at":null},
                "seven_day_sonnet":null}"#,
        )
        .unwrap();
        let limits = usage.limits();
        assert_eq!(
            limits.iter().map(|l| l.kind.as_str()).collect::<Vec<_>>(),
            ["session", "weekly_all"]
        );
        assert_eq!(limits[1].percent, 40.0);
        assert_eq!(limits[1].resets_at, None);
    }

    // Shape of a real `/profile` response (2026-10-19) with every identifying
    // field removed or replaced.
    const PROFILE: &str = r#"{
        "account": {
            "uuid": "00000000-0000-0000-0000-000000000000",
            "full_name": "Example Person",
            "display_name": "Example",
            "email": "person@example.com",
            "has_claude_max": false,
            "has_claude_pro": false,
            "created_at": "2025-01-01T00:00:00Z"
        },
        "organization": {
            "uuid": "00000000-0000-0000-0000-000000000001",
            "name": "Example Org",
            "organization_type": "claude_team",
            "billing_type": "stripe_subscription",
            "rate_limit_tier": "default_claude_max_5x",
            "seat_tier": "team_tier_1",
            "has_extra_usage_enabled": false,
            "subscription_status": "active",
            "claude_code_trial_ends_at": null
        },
        "application": {"uuid": "x", "name": "Claude Code", "slug": "claude-code"},
        "enabled_plugins": []
    }"#;

    #[test]
    fn profile_reads_plan_fields_and_ignores_the_rest() {
        let profile: Profile = serde_json::from_str(PROFILE).unwrap();
        let org = profile.organization.unwrap();
        assert_eq!(org.organization_type.as_deref(), Some("claude_team"));
        assert_eq!(
            org.rate_limit_tier.as_deref(),
            Some("default_claude_max_5x")
        );
        assert_eq!(org.seat_tier.as_deref(), Some("team_tier_1"));
        assert_eq!(profile.account.unwrap().has_claude_max, Some(false));
    }

    #[test]
    fn empty_profile_is_valid() {
        assert_eq!(
            serde_json::from_str::<Profile>("{}").unwrap(),
            Profile::default()
        );
    }

    #[test]
    fn empty_response_yields_no_limits() {
        let usage: Usage = serde_json::from_str("{}").unwrap();
        assert!(usage.limits().is_empty());
    }

    #[test]
    fn rate_limit_message_includes_retry_hint() {
        let e = UsageError::RateLimited {
            retry_after: Some(Duration::from_secs(90)),
        };
        assert_eq!(
            e.to_string(),
            "rate limited by the usage endpoint (retry in 90s)"
        );
        assert_eq!(
            UsageError::RateLimited { retry_after: None }.to_string(),
            "rate limited by the usage endpoint"
        );
    }
}
