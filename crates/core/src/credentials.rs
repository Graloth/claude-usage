//! Locating and parsing the OAuth credentials Claude Code stores locally.
//!
//! Strictly read-only. Refreshing the token here could invalidate the refresh
//! token Claude Code holds, so an expired access token is reported to the
//! caller and left for Claude Code to renew.
//!
//! Where the login lives:
//! - macOS: the login Keychain, generic password service
//!   `Claude Code-credentials`.
//! - Linux and Windows: `.credentials.json` in the Claude config directory.
//!
//! The stored JSON also holds unrelated secrets (MCP server tokens, the
//! refresh token). Only the fields in [`OAuthCredentials`] are deserialised.

use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::Deserialize;

#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const CREDENTIALS_FILE: &str = ".credentials.json";

/// The claude.ai OAuth login of the signed-in Claude Code user.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthCredentials {
    access_token: String,
    /// Unix epoch, milliseconds.
    expires_at: Option<i64>,
    subscription_type: Option<String>,
    rate_limit_tier: Option<String>,
}

impl OAuthCredentials {
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    /// Unix epoch in milliseconds, if the credentials carry an expiry.
    pub fn expires_at_ms(&self) -> Option<i64> {
        self.expires_at
    }

    /// Plan hint such as `pro`, `max` or `team`.
    pub fn subscription_type(&self) -> Option<&str> {
        self.subscription_type.as_deref()
    }

    pub fn rate_limit_tier(&self) -> Option<&str> {
        self.rate_limit_tier.as_deref()
    }

    pub fn is_expired_at(&self, now_ms: i64) -> bool {
        self.expires_at.is_some_and(|at| at <= now_ms)
    }
}

// Never print the token, even in `{:?}` of a larger struct or a panic message.
impl fmt::Debug for OAuthCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthCredentials")
            .field("access_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("subscription_type", &self.subscription_type)
            .field("rate_limit_tier", &self.rate_limit_tier)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialsError {
    /// Every source was readable and none held a login.
    #[error("no Claude Code login found (looked in: {})", .tried.join(", "))]
    NotFound { tried: Vec<String> },
    /// A source exists but could not be read or understood (locked Keychain,
    /// permissions, malformed JSON). Distinct from `NotFound` so the user is
    /// not told to sign in again when the real problem is access.
    #[error("could not read Claude Code credentials from {source_name}: {reason}")]
    Unreadable { source_name: String, reason: String },
}

/// Claude config directories, in lookup order.
///
/// `override_list` (for example from a setting) wins over `CLAUDE_CONFIG_DIR`;
/// both are comma-separated. Without either, the XDG location and `~/.claude`
/// are searched.
pub fn config_dirs(override_list: Option<&str>) -> Vec<PathBuf> {
    resolve_config_dirs(
        override_list,
        env::var("CLAUDE_CONFIG_DIR").ok().as_deref(),
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        env::home_dir(),
    )
}

fn resolve_config_dirs(
    override_list: Option<&str>,
    env_list: Option<&str>,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let explicit = [override_list, env_list]
        .into_iter()
        .flatten()
        .find(|list| !list.trim().is_empty());
    if let Some(list) = explicit {
        return list
            .split(',')
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
            .map(|dir| expand_home(dir, home.as_deref()))
            .collect();
    }

    let xdg = xdg_config_home
        .filter(|dir| !dir.as_os_str().is_empty())
        .or_else(|| home.as_ref().map(|h| h.join(".config")));
    xdg.map(|dir| dir.join("claude"))
        .into_iter()
        .chain(home.map(|h| h.join(".claude")))
        .collect()
}

fn expand_home(dir: &str, home: Option<&Path>) -> PathBuf {
    match (dir.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(dir),
    }
}

/// Parses the credentials JSON. `Ok(None)` means the JSON is fine but holds no
/// claude.ai login (for example an API-key user).
pub fn parse(json: &str) -> Result<Option<OAuthCredentials>, serde_json::Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Envelope {
        claude_ai_oauth: Option<OAuthCredentials>,
    }

    let envelope: Envelope = serde_json::from_str(json)?;
    Ok(envelope
        .claude_ai_oauth
        .filter(|creds| !creds.access_token.is_empty()))
}

/// Finds Claude Code's login on this machine.
pub fn load(config_dirs: &[PathBuf]) -> Result<OAuthCredentials, CredentialsError> {
    load_from(sources(config_dirs))
}

fn load_from(sources: Vec<Source>) -> Result<OAuthCredentials, CredentialsError> {
    let mut tried = Vec::new();
    // A problem with one source must not hide a working one, but if nothing
    // works the problem is more useful than "not found".
    let mut failure: Option<CredentialsError> = None;
    let mut fail = |source_name: String, reason: String| {
        failure.get_or_insert(CredentialsError::Unreadable {
            source_name,
            reason,
        });
    };

    for source in sources {
        let name = source.to_string();
        match source.read() {
            Ok(None) => tried.push(name),
            Err(reason) => fail(name, reason),
            Ok(Some(raw)) => match parse(&raw) {
                Ok(Some(creds)) => return Ok(creds),
                Ok(None) => tried.push(name),
                Err(e) => fail(name, format!("invalid JSON: {e}")),
            },
        }
    }
    Err(failure.unwrap_or(CredentialsError::NotFound { tried }))
}

enum Source {
    #[cfg(target_os = "macos")]
    Keychain,
    File(PathBuf),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(target_os = "macos")]
            Self::Keychain => f.write_str("macOS Keychain"),
            Self::File(path) => write!(f, "{}", path.display()),
        }
    }
}

impl Source {
    /// The raw credentials JSON, or `None` if this source has no entry.
    fn read(&self) -> Result<Option<String>, String> {
        match self {
            #[cfg(target_os = "macos")]
            Self::Keychain => read_keychain(),
            Self::File(path) => match fs::read_to_string(path) {
                Ok(raw) => Ok(Some(raw)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.to_string()),
            },
        }
    }
}

fn sources(config_dirs: &[PathBuf]) -> Vec<Source> {
    let files = config_dirs
        .iter()
        .map(|dir| Source::File(dir.join(CREDENTIALS_FILE)));
    #[cfg(target_os = "macos")]
    {
        std::iter::once(Source::Keychain).chain(files).collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        files.collect()
    }
}

/// `security` exit status for `errSecItemNotFound`.
#[cfg(target_os = "macos")]
const SECURITY_ITEM_NOT_FOUND: i32 = 44;

/// Reads the Keychain item through `/usr/bin/security`, the same tool Claude
/// Code uses, so no access prompt appears for the Apple-signed binary.
#[cfg(target_os = "macos")]
fn read_keychain() -> Result<Option<String>, String> {
    let output = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .map_err(|e| format!("could not run /usr/bin/security: {e}"))?;

    match output.status.code() {
        Some(0) => {
            let raw = String::from_utf8(output.stdout)
                .map_err(|_| "Keychain item is not valid UTF-8".to_string())?;
            let raw = raw.trim_end_matches('\n');
            Ok((!raw.is_empty()).then(|| raw.to_owned()))
        }
        Some(SECURITY_ITEM_NOT_FOUND) => Ok(None),
        _ => Err(format!(
            "security exited with {}: {} (is the Keychain unlocked?)",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shaped like the real store: the login next to unrelated secrets.
    const STORE: &str = r#"{
        "mcpOAuth": {"plugin:example|abc": {"accessToken": "mcp-secret"}},
        "claudeAiOauth": {
            "accessToken": "sk-ant-oat-test",
            "refreshToken": "sk-ant-ort-test",
            "expiresAt": 1000,
            "refreshTokenExpiresAt": 2000,
            "scopes": ["user:profile"],
            "subscriptionType": "team",
            "rateLimitTier": "default_raven"
        }
    }"#;

    #[test]
    fn parses_login_and_ignores_other_secrets() {
        let creds = parse(STORE).unwrap().unwrap();
        assert_eq!(creds.access_token(), "sk-ant-oat-test");
        assert_eq!(creds.subscription_type(), Some("team"));
        assert_eq!(creds.rate_limit_tier(), Some("default_raven"));
        assert_eq!(creds.expires_at_ms(), Some(1000));
    }

    #[test]
    fn debug_output_hides_every_secret() {
        let dump = format!("{:?}", parse(STORE).unwrap().unwrap());
        assert!(!dump.contains("sk-ant"));
        assert!(!dump.contains("mcp-secret"));
        assert!(dump.contains("<redacted>"));
    }

    #[test]
    fn expiry_is_inclusive_and_optional() {
        let creds = parse(STORE).unwrap().unwrap();
        assert!(!creds.is_expired_at(999));
        assert!(creds.is_expired_at(1000));

        let no_expiry = parse(r#"{"claudeAiOauth":{"accessToken":"t"}}"#)
            .unwrap()
            .unwrap();
        assert!(!no_expiry.is_expired_at(i64::MAX));
    }

    #[test]
    fn missing_or_empty_login_is_none_not_an_error() {
        assert!(parse(r#"{"mcpOAuth":{}}"#).unwrap().is_none());
        assert!(
            parse(r#"{"claudeAiOauth":{"accessToken":""}}"#)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(parse("not json").is_err());
        assert!(parse(r#"{"claudeAiOauth":{"expiresAt":1}}"#).is_err());
    }

    fn dirs(
        override_list: Option<&str>,
        env_list: Option<&str>,
        xdg: Option<&str>,
        home: Option<&str>,
    ) -> Vec<PathBuf> {
        resolve_config_dirs(
            override_list,
            env_list,
            xdg.map(PathBuf::from),
            home.map(PathBuf::from),
        )
    }

    #[test]
    fn config_dirs_default_to_xdg_then_dot_claude() {
        assert_eq!(
            dirs(None, None, None, Some("/home/u")),
            [
                PathBuf::from("/home/u/.config/claude"),
                PathBuf::from("/home/u/.claude")
            ]
        );
        assert_eq!(
            dirs(None, None, Some("/xdg"), Some("/home/u")),
            [
                PathBuf::from("/xdg/claude"),
                PathBuf::from("/home/u/.claude")
            ]
        );
    }

    #[test]
    fn config_dirs_override_beats_env_and_expands_home() {
        assert_eq!(
            dirs(Some("~/a, /b ,"), Some("/env"), None, Some("/home/u")),
            [PathBuf::from("/home/u/a"), PathBuf::from("/b")]
        );
        assert_eq!(
            dirs(Some("  "), Some("/env"), None, Some("/home/u")),
            [PathBuf::from("/env")]
        );
    }

    /// A scratch directory holding `.credentials.json` with `contents`, if any.
    /// Goes through `load_from` with file sources only, so the result does not
    /// depend on the machine's own login (the Keychain on macOS).
    fn load_file(test: &str, contents: Option<&str>) -> Result<OAuthCredentials, CredentialsError> {
        let dir = env::temp_dir().join(format!("claude-usage-{test}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        if let Some(contents) = contents {
            fs::write(dir.join(CREDENTIALS_FILE), contents).unwrap();
        }
        let result = load_from(vec![Source::File(dir.join(CREDENTIALS_FILE))]);
        fs::remove_dir_all(&dir).unwrap();
        result
    }

    #[test]
    fn loads_login_from_credentials_file() {
        let creds = load_file("ok", Some(STORE)).unwrap();
        assert_eq!(creds.access_token(), "sk-ant-oat-test");
    }

    #[test]
    fn absent_file_is_not_found_and_lists_what_was_tried() {
        let err = load_file("absent", None).unwrap_err();
        assert!(matches!(&err, CredentialsError::NotFound { tried } if tried.len() == 1));
        assert!(err.to_string().contains(CREDENTIALS_FILE));
    }

    #[test]
    fn corrupt_file_is_unreadable_not_not_found() {
        let err = load_file("corrupt", Some("{oops")).unwrap_err();
        assert!(matches!(err, CredentialsError::Unreadable { .. }));
    }

    #[test]
    fn broken_source_does_not_hide_a_working_one() {
        let dir = env::temp_dir().join(format!("claude-usage-fallback-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.json");
        fs::write(&good, STORE).unwrap();
        let bad = dir.join("bad.json");
        fs::write(&bad, "{oops").unwrap();

        let result = load_from(vec![Source::File(bad), Source::File(good)]);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(result.unwrap().access_token(), "sk-ant-oat-test");
    }

    #[test]
    fn missing_home_does_not_panic() {
        assert!(dirs(None, None, None, None).is_empty());
    }
}
