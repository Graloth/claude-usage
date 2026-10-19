//! Background scan of the local Claude Code transcripts: what the usage would
//! have cost at API prices and when it happened, plus the detected plan.
//!
//! Runs on its own thread so a slow scan never delays the limits poller. The
//! scan is incremental and cached on disk (see `claude_usage_core::scanner`).

use std::{
    sync::{
        Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use claude_usage_core::{
    credentials,
    plan::{self, Plan},
    scanner::{self, ScanOptions, ScanReport},
    usage,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::settings::{Section, Settings, SettingsStore};

/// Event the popup listens to; carries the new [`ScanSnapshot`].
pub const SCAN_EVENT: &str = "scan-updated";

/// The window the Value and heatmap sections cover.
const SCAN_DAYS: u32 = 30;
/// Re-check the plan this rarely: it changes with a subscription change, and
/// each check is a request to the profile endpoint.
const PLAN_TTL: Duration = Duration::from_secs(6 * 3600);
/// Retry a failed plan lookup sooner, but not at every scan.
const PLAN_RETRY: Duration = Duration::from_secs(600);
/// Least time between two scans, however often one is asked for.
const MIN_SCAN_GAP: Duration = Duration::from_secs(10);
/// Scan at least this often even when the user's refresh interval is shorter.
const MIN_INTERVAL: Duration = Duration::from_secs(120);
/// How long to idle when no section needs scan data.
const IDLE: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize)]
pub struct PlanInfo {
    pub name: String,
    /// Monthly price in USD.
    pub price_usd: f64,
}

impl From<Plan> for PlanInfo {
    fn from(plan: Plan) -> Self {
        Self {
            name: plan.name().to_owned(),
            price_usd: plan.monthly_usd(),
        }
    }
}

/// What the popup shows for Value and the heatmap. A failed scan keeps the
/// previous report and records why.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanSnapshot {
    pub report: Option<ScanReport>,
    pub plan: Option<PlanInfo>,
    pub problem: Option<String>,
}

pub struct ScanShared {
    state: Mutex<ScanSnapshot>,
    wake: Sender<()>,
}

impl ScanShared {
    pub fn snapshot(&self) -> ScanSnapshot {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Asks the worker to scan now (still subject to `MIN_SCAN_GAP`). Also
    /// makes it re-read the settings.
    pub fn refresh(&self) {
        // The receiver only disappears when the app is shutting down.
        let _ = self.wake.send(());
    }
}

/// Registers the shared state and starts the worker thread.
pub fn start(app: &AppHandle) {
    let (wake, requests) = mpsc::channel();
    app.manage(ScanShared {
        state: Mutex::new(ScanSnapshot::default()),
        wake,
    });

    let app = app.clone();
    thread::Builder::new()
        .name("transcript-scanner".into())
        .spawn(move || run(&app, &requests))
        .expect("could not start the transcript scanner thread");
}

/// The plan, looked up rarely and remembered.
struct PlanCache {
    plan: Option<Plan>,
    checked_at: Option<Instant>,
    /// How long `plan` stays valid.
    ttl: Duration,
}

impl PlanCache {
    fn new() -> Self {
        Self {
            plan: None,
            checked_at: None,
            ttl: Duration::ZERO,
        }
    }

    fn current(&mut self, dirs: &[std::path::PathBuf], client: &usage::Client) -> Option<Plan> {
        let fresh = self.checked_at.is_some_and(|at| at.elapsed() < self.ttl);
        if !fresh {
            let (plan, ttl) = detect_plan(dirs, client);
            self.plan = plan;
            self.ttl = ttl;
            self.checked_at = Some(Instant::now());
        }
        self.plan
    }
}

/// The plan and how long to trust the answer. Without the profile endpoint
/// the credentials alone give a coarser answer, retried sooner.
fn detect_plan(dirs: &[std::path::PathBuf], client: &usage::Client) -> (Option<Plan>, Duration) {
    let Ok(creds) = credentials::load(dirs) else {
        return (None, PLAN_RETRY);
    };
    match client.fetch_profile(&creds) {
        Ok(profile) => (plan::detect_plan(Some(&profile), &creds), PLAN_TTL),
        Err(_) => (plan::detect_plan(None, &creds), PLAN_RETRY),
    }
}

fn wants_scan(settings: &Settings) -> bool {
    settings.section_enabled(Section::Value) || settings.section_enabled(Section::Heatmap)
}

fn run(app: &AppHandle, requests: &Receiver<()>) {
    let client = usage::Client::new();
    let mut plan = PlanCache::new();
    let mut last_scan: Option<Instant> = None;

    loop {
        let settings = app.state::<SettingsStore>().get();
        let interval =
            Duration::from_secs(u64::from(settings.refresh_minutes) * 60).max(MIN_INTERVAL);

        let delay = if wants_scan(&settings) {
            if let Some(at) = last_scan {
                thread::sleep(MIN_SCAN_GAP.saturating_sub(at.elapsed()));
            }
            last_scan = Some(Instant::now());
            scan_once(app, &settings, &client, &mut plan);
            interval
        } else {
            IDLE
        };

        match requests.recv_timeout(delay) {
            Ok(()) => while requests.try_recv().is_ok() {},
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn scan_once(app: &AppHandle, settings: &Settings, client: &usage::Client, plan: &mut PlanCache) {
    let dirs = credentials::config_dirs(settings.claude_config_dir.as_deref());
    let options = ScanOptions {
        days: SCAN_DAYS,
        config_dirs: dirs.clone(),
        cache_path: app
            .path()
            .app_cache_dir()
            .ok()
            .map(|dir| dir.join("scan.json")),
    };
    let result = scanner::scan_local(&options);
    let detected = plan.current(&dirs, client);

    let shared = app.state::<ScanShared>();
    let snapshot = {
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match result {
            Ok(report) => {
                state.report = Some(report);
                state.problem = None;
            }
            Err(e) => state.problem = Some(format!("Could not read your Claude Code history: {e}")),
        }
        state.plan = detected.map(PlanInfo::from);
        state.clone()
    };

    if let Err(e) = app.emit(SCAN_EVENT, &snapshot) {
        eprintln!("claude-usage: could not notify the popup: {e}");
    }
}
