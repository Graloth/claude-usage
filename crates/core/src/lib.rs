//! Platform-independent core of claude-usage: finding Claude Code's login
//! and talking to the usage API. No UI dependencies, so it can be tested and
//! driven from a plain CLI.

pub mod credentials;
pub mod usage;
