# Claude Usage

Cross-platform tray app (Linux, macOS, Windows) showing your Claude plan limits and what your Claude Code usage would have cost at API list prices.

Based on [gnome-claude-usage](https://github.com/naueramant/gnome-claude-usage) by Jonas Tranberg, which does the same as a GNOME Shell extension. This project ports the idea to a standalone Tauri 2 app so it works on any desktop.

Status: early scaffold. The tray icon and popup shell exist; limits, cost and heatmap are not implemented yet.

## Design

- Tauri 2 (Rust). The popup webview is created when you click the tray icon and destroyed when it loses focus, so nothing but the small Rust tray process runs while idle.
- Reads the OAuth credentials Claude Code already stores locally, and your Claude Code transcripts under `~/.claude/projects`. Nothing leaves your machine except the request to Anthropic's usage endpoint.

## Development

Requires Rust (pinned in `rust-toolchain.toml`), Node and pnpm.

```bash
pnpm install
pnpm tauri dev
```

## Licence

GNU General Public License v2.0 or later, see [LICENSE](LICENSE).

Not affiliated with or endorsed by Anthropic. Claude is a trademark of Anthropic, PBC.
