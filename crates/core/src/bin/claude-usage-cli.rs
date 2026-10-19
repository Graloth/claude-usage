//! Prints the Claude plan limits for the signed-in Claude Code user, or, with
//! `--scan`, what the local transcripts say about recent usage.
//!
//! Usage:
//!   claude-usage-cli [--config-dir DIR[,DIR...]]
//!   claude-usage-cli --scan [--days N] [--no-cache] [--config-dir DIR[,DIR...]]
//!
//! `--scan` needs no login. It keeps its incremental cache at
//! `claude_usage_core::scanner::default_cache_path()` (for example
//! `~/Library/Caches/claude-usage/scan.json` on macOS); `--no-cache` ignores
//! and does not write it, which gives a cold-run timing.

use std::{
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use claude_usage_core::{
    credentials,
    scanner::{self, ScanOptions, ScanReport},
    usage,
};
use jiff::{Timestamp, tz::TimeZone};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("claude-usage-cli: {message}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Default)]
struct Args {
    config_dir: Option<String>,
    scan: bool,
    days: Option<u32>,
    no_cache: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut parsed = Args::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config-dir" => {
                parsed.config_dir = Some(args.next().ok_or("--config-dir needs a value")?);
            }
            "--scan" => parsed.scan = true,
            "--no-cache" => parsed.no_cache = true,
            "--days" => {
                let value = args.next().ok_or("--days needs a value")?;
                parsed.days = Some(value.parse().map_err(|_| format!("bad --days {value:?}"))?);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if !parsed.scan && (parsed.days.is_some() || parsed.no_cache) {
        return Err("--days and --no-cache only apply with --scan".into());
    }
    Ok(parsed)
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let dirs = credentials::config_dirs(args.config_dir.as_deref());
    if args.scan {
        return run_scan(&args, dirs);
    }

    let creds = credentials::load(&dirs).map_err(|e| e.to_string())?;

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    println!(
        "plan: {} (tier {})",
        creds.subscription_type().unwrap_or("unknown"),
        creds.rate_limit_tier().unwrap_or("unknown")
    );
    if let Some(expires_at) = creds.expires_at_ms() {
        let minutes = (expires_at - now_ms) / 60_000;
        println!(
            "token: {}",
            if minutes >= 0 {
                format!("valid for {minutes} min")
            } else {
                format!("expired {} min ago", -minutes)
            }
        );
    }

    let usage = usage::Client::new()
        .fetch_usage(&creds)
        .map_err(|e| e.to_string())?;

    for limit in usage.limits() {
        let label = limit
            .scope_label()
            .map_or_else(|| limit.kind.clone(), |s| format!("{} ({s})", limit.kind));
        println!(
            "{label:<28} {:>5.1}%  resets {}",
            limit.percent,
            limit.resets_at.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

fn run_scan(args: &Args, config_dirs: Vec<std::path::PathBuf>) -> Result<(), String> {
    let tz = TimeZone::try_system().unwrap_or_else(|e| {
        eprintln!("claude-usage-cli: could not detect the time zone ({e}), using UTC");
        TimeZone::UTC
    });
    let opts = ScanOptions {
        days: args.days.unwrap_or(30),
        config_dirs,
        cache_path: if args.no_cache {
            None
        } else {
            scanner::default_cache_path()
        },
    };

    let started = std::time::Instant::now();
    let report = scanner::scan(&opts, Timestamp::now(), &tz).map_err(|e| e.to_string())?;
    let elapsed = started.elapsed();

    print_scan(&report);
    eprintln!(
        "scanned {} files in {:.2?} (cache: {})",
        report.files_scanned,
        elapsed,
        opts.cache_path
            .as_deref()
            .map_or_else(|| "off".to_owned(), |p| p.display().to_string())
    );
    Ok(())
}

fn print_scan(report: &ScanReport) {
    let t = &report.totals;
    println!("last {} days, API-equivalent cost", report.days);
    println!("total        ${:>10.2}  ({} messages)", t.cost, t.messages);
    println!(
        "tokens       in {}  out {}  cache write {}  cache read {}",
        t.input, t.output, t.cache_write, t.cache_read
    );
    println!(
        "active days  {}  streak {}",
        report.active_days, report.streak
    );
    for family in &report.families {
        println!("  {:<8} ${:>10.2}", family.name, family.cost);
    }
    println!("last 7 days");
    for day in report.daily.iter().rev().take(7).rev() {
        println!(
            "  {}  ${:>8.2}  {:>9} tokens  {:>5} messages",
            day.date, day.cost, day.tokens, day.messages
        );
    }
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
}
