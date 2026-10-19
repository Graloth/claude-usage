//! Platform-independent core of claude-usage: finding Claude Code's login,
//! talking to the usage API, and turning local transcripts into cost and
//! activity numbers. No UI dependencies, so it can be tested and driven from a
//! plain CLI.

pub mod credentials;
pub mod plan;
pub mod pricing;
pub mod scanner;
pub mod usage;
