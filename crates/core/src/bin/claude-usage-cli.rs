//! Prints the Claude plan limits for the signed-in Claude Code user.
//!
//! Usage: claude-usage-cli [--config-dir DIR[,DIR...]]

use std::{
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use claude_usage_core::{credentials, usage};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("claude-usage-cli: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut config_dir = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config-dir" => {
                config_dir = Some(args.next().ok_or("--config-dir needs a value")?);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }

    let dirs = credentials::config_dirs(config_dir.as_deref());
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
