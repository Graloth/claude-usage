//! Claude subscription plans: what they cost and which one a login is on.
//!
//! Prices follow <https://claude.com/pricing> as fetched on 2026-10-19 and are
//! per seat per month on *monthly* billing. Annual billing is cheaper for Pro
//! and Team; see [`Plan::annual_billed_monthly_usd`]. Anthropic changes these,
//! so the app lets users override the price.

use serde::Serialize;

use crate::{credentials::OAuthCredentials, usage::Profile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Plan {
    #[serde(rename = "pro")]
    Pro,
    #[serde(rename = "max5")]
    Max5x,
    #[serde(rename = "max20")]
    Max20x,
    #[serde(rename = "team-standard")]
    TeamStandard,
    #[serde(rename = "team-premium")]
    TeamPremium,
    #[serde(rename = "enterprise")]
    Enterprise,
    #[serde(rename = "custom")]
    Custom,
}

impl Plan {
    pub const ALL: [Plan; 7] = [
        Plan::Pro,
        Plan::Max5x,
        Plan::Max20x,
        Plan::TeamStandard,
        Plan::TeamPremium,
        Plan::Enterprise,
        Plan::Custom,
    ];

    /// Stable identifier, also the serialised form (matches the original
    /// extension's settings values).
    pub fn key(self) -> &'static str {
        match self {
            Plan::Pro => "pro",
            Plan::Max5x => "max5",
            Plan::Max20x => "max20",
            Plan::TeamStandard => "team-standard",
            Plan::TeamPremium => "team-premium",
            Plan::Enterprise => "enterprise",
            Plan::Custom => "custom",
        }
    }

    pub fn from_key(key: &str) -> Option<Plan> {
        Plan::ALL.into_iter().find(|plan| plan.key() == key)
    }

    pub fn name(self) -> &'static str {
        match self {
            Plan::Pro => "Pro",
            Plan::Max5x => "Max 5×",
            Plan::Max20x => "Max 20×",
            Plan::TeamStandard => "Team",
            Plan::TeamPremium => "Team Premium",
            Plan::Enterprise => "Enterprise",
            Plan::Custom => "Custom",
        }
    }

    /// USD per seat per month on monthly billing. Zero where there is no
    /// meaningful flat price: Enterprise seats ($20 on annual terms) exclude
    /// usage billed at API rates, so a subscription-versus-API comparison does
    /// not apply, and Custom is user-defined.
    pub fn monthly_usd(self) -> f64 {
        match self {
            Plan::Pro => 20.0,
            Plan::Max5x => 100.0,
            Plan::Max20x => 200.0,
            Plan::TeamStandard => 25.0,
            Plan::TeamPremium => 125.0,
            Plan::Enterprise | Plan::Custom => 0.0,
        }
    }

    /// Effective USD per month when billed annually, for plans that offer it.
    pub fn annual_billed_monthly_usd(self) -> Option<f64> {
        match self {
            Plan::Pro => Some(17.0),
            Plan::TeamStandard => Some(20.0),
            Plan::TeamPremium => Some(100.0),
            _ => None,
        }
    }
}

/// Works out the plan from the OAuth profile and/or the stored credentials.
///
/// The organisation in the profile wins over the credentials, and team and
/// enterprise win over the rate-limit tier: a Team seat reports
/// `default_claude_max_5x` as its tier.
pub fn detect_plan(profile: Option<&Profile>, creds: &OAuthCredentials) -> Option<Plan> {
    let org = profile.and_then(|p| p.organization.as_ref());
    let account = profile.and_then(|p| p.account.as_ref());

    let tier = org
        .and_then(|o| o.rate_limit_tier.as_deref())
        .or(creds.rate_limit_tier())
        .unwrap_or_default()
        .to_lowercase();
    let kind = org
        .and_then(|o| o.organization_type.as_deref())
        .or(creds.subscription_type())
        .unwrap_or_default()
        .to_lowercase();
    let flag = |get: fn(&crate::usage::ProfileAccount) -> Option<bool>| {
        account.and_then(get).unwrap_or(false)
    };

    if kind.contains("enterprise") {
        Some(Plan::Enterprise)
    } else if kind.contains("team") {
        let premium = org
            .and_then(|o| o.seat_tier.as_deref())
            .is_some_and(|seat| seat.contains("premium"));
        Some(if premium {
            Plan::TeamPremium
        } else {
            Plan::TeamStandard
        })
    } else if tier.contains("20x") {
        Some(Plan::Max20x)
    } else if kind.contains("max") || tier.contains("max") || flag(|a| a.has_claude_max) {
        Some(Plan::Max5x)
    } else if kind.contains("pro") || flag(|a| a.has_claude_pro) {
        Some(Plan::Pro)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        credentials,
        usage::{ProfileAccount, ProfileOrganization},
    };

    fn creds(subscription_type: Option<&str>, tier: Option<&str>) -> OAuthCredentials {
        let json = serde_json::json!({"claudeAiOauth": {
            "accessToken": "t",
            "subscriptionType": subscription_type,
            "rateLimitTier": tier,
        }});
        credentials::parse(&json.to_string()).unwrap().unwrap()
    }

    fn profile(kind: Option<&str>, tier: Option<&str>, seat: Option<&str>) -> Profile {
        Profile {
            account: None,
            organization: Some(ProfileOrganization {
                organization_type: kind.map(Into::into),
                rate_limit_tier: tier.map(Into::into),
                seat_tier: seat.map(Into::into),
            }),
        }
    }

    #[test]
    fn team_seat_is_not_mistaken_for_max_despite_its_tier() {
        // The real shape of a Team seat.
        let p = profile(
            Some("claude_team"),
            Some("default_claude_max_5x"),
            Some("team_tier_1"),
        );
        assert_eq!(
            detect_plan(Some(&p), &creds(None, None)),
            Some(Plan::TeamStandard)
        );
    }

    #[test]
    fn premium_seat_and_enterprise() {
        let premium = profile(Some("claude_team"), None, Some("team_premium"));
        assert_eq!(
            detect_plan(Some(&premium), &creds(None, None)),
            Some(Plan::TeamPremium)
        );
        let enterprise = profile(Some("claude_enterprise"), None, None);
        assert_eq!(
            detect_plan(Some(&enterprise), &creds(None, None)),
            Some(Plan::Enterprise)
        );
    }

    #[test]
    fn max_tiers() {
        let max20 = profile(Some("claude_max"), Some("default_claude_max_20x"), None);
        assert_eq!(
            detect_plan(Some(&max20), &creds(None, None)),
            Some(Plan::Max20x)
        );
        let max5 = profile(Some("claude_max"), Some("default_claude_max_5x"), None);
        assert_eq!(
            detect_plan(Some(&max5), &creds(None, None)),
            Some(Plan::Max5x)
        );
    }

    #[test]
    fn falls_back_to_the_credentials_without_a_profile() {
        assert_eq!(
            detect_plan(None, &creds(Some("pro"), None)),
            Some(Plan::Pro)
        );
        assert_eq!(
            detect_plan(None, &creds(Some("max"), Some("default_claude_max_20x"))),
            Some(Plan::Max20x)
        );
        assert_eq!(
            detect_plan(None, &creds(Some("team"), Some("default_raven"))),
            Some(Plan::TeamStandard)
        );
        assert_eq!(detect_plan(None, &creds(None, None)), None);
    }

    #[test]
    fn profile_organisation_beats_the_credentials() {
        let p = profile(Some("claude_pro"), None, None);
        assert_eq!(
            detect_plan(Some(&p), &creds(Some("max"), None)),
            Some(Plan::Pro)
        );
    }

    #[test]
    fn account_flags_are_the_last_resort() {
        let account = |max, pro| Profile {
            account: Some(ProfileAccount {
                has_claude_max: Some(max),
                has_claude_pro: Some(pro),
            }),
            organization: None,
        };
        assert_eq!(
            detect_plan(Some(&account(true, false)), &creds(None, None)),
            Some(Plan::Max5x)
        );
        assert_eq!(
            detect_plan(Some(&account(false, true)), &creds(None, None)),
            Some(Plan::Pro)
        );
        assert_eq!(
            detect_plan(Some(&account(false, false)), &creds(None, None)),
            None
        );
    }

    #[test]
    fn keys_round_trip_and_serialise_as_keys() {
        for plan in Plan::ALL {
            assert_eq!(Plan::from_key(plan.key()), Some(plan));
            assert_eq!(
                serde_json::to_string(&plan).unwrap(),
                format!("\"{}\"", plan.key())
            );
        }
        assert_eq!(Plan::from_key("nope"), None);
    }

    #[test]
    fn prices() {
        assert_eq!(Plan::Pro.monthly_usd(), 20.0);
        assert_eq!(Plan::TeamStandard.monthly_usd(), 25.0);
        assert_eq!(Plan::TeamPremium.annual_billed_monthly_usd(), Some(100.0));
        assert_eq!(Plan::Max5x.annual_billed_monthly_usd(), None);
        assert_eq!(Plan::Enterprise.monthly_usd(), 0.0);
    }
}
