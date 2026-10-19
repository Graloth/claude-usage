//! Background refresh of the usage snapshot.
//!
//! One worker thread reads Claude Code's login, asks the usage endpoint for
//! the limits and publishes the result to the tray and the popup. The endpoint
//! rate-limits hard, so requests are spaced out and 429s are honoured.

use std::{
    sync::{
        Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use claude_usage_core::{
    credentials,
    usage::{self, Limit, UsageError},
};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::{
    notify::NotifyState,
    settings::{Settings, SettingsStore},
    snapshot::{Problem, Snapshot},
    tray,
};

/// Event the popup listens to; carries the new [`Snapshot`].
pub const UPDATED_EVENT: &str = "usage-updated";

/// Retry interval while the sign-in is missing or expired. Reading the login
/// is local and cheap, and this picks up a renewal by Claude Code soon after.
const LOGIN_RETRY: Duration = Duration::from_secs(60);
/// Retry after the server rejected the token or could not be reached.
const ERROR_RETRY: Duration = Duration::from_secs(120);
const MIN_RETRY_AFTER: Duration = Duration::from_secs(60);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(3600);
/// Least time between two requests, however often a refresh is asked for.
const MIN_REQUEST_GAP: Duration = Duration::from_secs(60);

/// State shared between the poller, the tray and the popup.
pub struct Shared {
    snapshot: Mutex<Snapshot>,
    wake: Sender<()>,
}

impl Shared {
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
            .lock()
            .map_or_else(|poisoned| poisoned.into_inner().clone(), |s| s.clone())
    }

    /// Asks the poller to refresh now (still subject to `MIN_REQUEST_GAP`). Also
    /// makes it re-read the settings, so a changed interval takes effect at once.
    pub fn refresh(&self) {
        // The receiver only disappears when the app is shutting down.
        let _ = self.wake.send(());
    }
}

/// Registers the shared state and starts the worker thread.
pub fn start(app: &AppHandle) {
    let (wake, requests) = mpsc::channel();
    app.manage(Shared {
        snapshot: Mutex::new(Snapshot::default()),
        wake,
    });

    let app = app.clone();
    thread::Builder::new()
        .name("usage-poller".into())
        .spawn(move || run(&app, &requests))
        .expect("could not start the usage poller thread");
}

fn run(app: &AppHandle, requests: &Receiver<()>) {
    let client = usage::Client::new();
    let mut last_request: Option<Instant> = None;
    let mut notifications = NotifyState::default();

    loop {
        // Re-read each round so settings changes apply without a restart.
        let settings = app.state::<SettingsStore>().get();
        let dirs = credentials::config_dirs(settings.claude_config_dir.as_deref());
        let interval = Duration::from_secs(u64::from(settings.refresh_minutes) * 60);

        let (result, delay) = poll_once(&client, &dirs, &mut last_request, interval);
        publish(app, result, &settings, &mut notifications);

        match requests.recv_timeout(delay) {
            // A refresh was requested: poll again now.
            Ok(()) => while requests.try_recv().is_ok() {},
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// One refresh attempt, and how long to wait before the next.
fn poll_once(
    client: &usage::Client,
    dirs: &[std::path::PathBuf],
    last_request: &mut Option<Instant>,
    interval: Duration,
) -> (Result<Vec<Limit>, Problem>, Duration) {
    let creds = match credentials::load(dirs) {
        Ok(creds) => creds,
        Err(e) => return (Err(Problem::from_credentials(&e)), LOGIN_RETRY),
    };
    // Skip the request when we already know it would be refused.
    if creds.is_expired_at(now_ms()) {
        return (Err(Problem::expired()), LOGIN_RETRY);
    }

    if let Some(at) = *last_request {
        thread::sleep(MIN_REQUEST_GAP.saturating_sub(at.elapsed()));
    }
    *last_request = Some(Instant::now());

    match client.fetch_usage(&creds) {
        Ok(usage) => (Ok(usage.limits()), interval),
        Err(e) => (Err(Problem::from_usage(&e)), retry_delay(&e, interval)),
    }
}

/// How long to wait after a failed request. `interval` is the user's normal
/// refresh interval.
fn retry_delay(error: &UsageError, interval: Duration) -> Duration {
    match error {
        UsageError::RateLimited { retry_after } => retry_after
            .unwrap_or(interval)
            .clamp(MIN_RETRY_AFTER, MAX_RETRY_AFTER),
        UsageError::Unauthorized | UsageError::Transport(_) => ERROR_RETRY,
        UsageError::Status(_) | UsageError::Parse(_) => interval,
    }
}

fn publish(
    app: &AppHandle,
    result: Result<Vec<Limit>, Problem>,
    settings: &Settings,
    notifications: &mut NotifyState,
) {
    let shared = app.state::<Shared>();
    let fresh = result.is_ok();
    let snapshot = {
        let mut guard = shared
            .snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match result {
            Ok(limits) => guard.record_success(&limits, now_ms()),
            Err(problem) => guard.record_problem(problem),
        }
        guard.clone()
    };

    tray::apply(app, &snapshot, settings);
    if let Err(e) = app.emit(UPDATED_EVENT, &snapshot) {
        eprintln!("claude-usage: could not notify the popup: {e}");
    }

    // Only fresh data can trigger notifications; stale values never do.
    if fresh {
        for note in notifications.observe(&snapshot.limits, settings) {
            if let Err(e) = app
                .notification()
                .builder()
                .title(&note.title)
                .body(&note.body)
                .show()
            {
                eprintln!("claude-usage: could not show notification: {e}");
            }
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_secs(300);

    #[test]
    fn rate_limit_waits_as_long_as_asked_within_bounds() {
        let delay = |secs: Option<u64>| {
            retry_delay(
                &UsageError::RateLimited {
                    retry_after: secs.map(Duration::from_secs),
                },
                INTERVAL,
            )
        };
        assert_eq!(delay(Some(600)), Duration::from_secs(600));
        assert_eq!(delay(Some(1)), MIN_RETRY_AFTER);
        assert_eq!(delay(Some(86_400)), MAX_RETRY_AFTER);
        assert_eq!(delay(None), INTERVAL);
    }

    #[test]
    fn other_errors_retry_sooner_or_at_the_normal_pace() {
        assert_eq!(
            retry_delay(&UsageError::Unauthorized, INTERVAL),
            ERROR_RETRY
        );
        assert_eq!(
            retry_delay(&UsageError::Transport("x".into()), INTERVAL),
            ERROR_RETRY
        );
        assert_eq!(retry_delay(&UsageError::Status(500), INTERVAL), INTERVAL);
    }
}
