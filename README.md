# Claude Usage

Cross-platform tray app (Linux, macOS, Windows) showing your Claude plan limits and what your Claude Code usage would have cost at API list prices.

Based on [gnome-claude-usage](https://github.com/naueramant/gnome-claude-usage) by Jonas Tranberg, which does the same as a GNOME Shell extension. This project ports the idea to a standalone Tauri 2 app so it works on any desktop.

Status: early but usable on macOS. The tray ring and popup show your limits, what the last 30 days would have cost at API prices against your plan, and an activity heatmap. Settings are in the popup (gear icon). Linux and Windows are untested.

Not ported from the original yet: the token house, custom accent colour and tray placement.

## Design

- Tauri 2 (Rust). The popup webview is created when you click the tray icon and destroyed when it loses focus, so nothing but the small Rust tray process runs while idle.
- Reads the OAuth credentials Claude Code already stores locally, and your Claude Code transcripts under `~/.claude/projects`. Nothing leaves your machine except the request to Anthropic's usage endpoint.

Layout: `crates/core` is the UI-free library (credentials, usage API client) plus a small CLI; `src-tauri` and `src` are the tray app.

## Development

Requires Rust (pinned in `rust-toolchain.toml`), Node and pnpm. On Linux the tray app also needs Tauri's system packages, see the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/#linux).

Test the core without any GUI dependencies. You must be signed in to Claude Code (`claude`) and the token must not have expired, which it does a few hours after Claude Code last ran:

```bash
cargo run -p claude-usage-core --bin claude-usage-cli
cargo run -p claude-usage-core --bin claude-usage-cli -- --scan   # cost and activity from ~/.claude, no login needed
cargo test --workspace
```

It prints your plan, how long the token is valid and each limit. Credentials come from the macOS Keychain on macOS, or from `.credentials.json` in `~/.config/claude` or `~/.claude` elsewhere (override with `--config-dir` or `CLAUDE_CONFIG_DIR`). The tool only reads them and never refreshes the token.

Run the tray app:

```bash
pnpm install
pnpm tauri dev
```

On Linux, clicks on the tray icon are not reported to the app, so open the popup from the tray menu ("Show usage"). GNOME also needs an AppIndicator extension to show the tray icon at all.

## Licence

GNU General Public License v2.0 or later, see [LICENSE](LICENSE).

Not affiliated with or endorsed by Anthropic. Claude is a trademark of Anthropic, PBC.
