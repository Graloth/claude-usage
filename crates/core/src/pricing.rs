//! Anthropic API list prices and the cost of one response.
//!
//! The table follows <https://platform.claude.com/docs/en/about-claude/pricing>
//! as fetched on 2026-10-19. Prices change: re-check before trusting it, and
//! add a row (more specific patterns first) when a model is released.

/// Cache writes cost this multiple of the base input price (5-minute TTL).
const WRITE_5M_MULTIPLIER: f64 = 1.25;
/// Same for the 1-hour TTL.
const WRITE_1H_MULTIPLIER: f64 = 2.0;
/// Fast mode (Opus 5.5, Opus 5, Opus 4.8) bills every token category at twice
/// the standard rate; cache multipliers stack on top, which a flat 2x matches.
const FAST_MULTIPLIER: f64 = 2.0;

/// Token counts of one API response, as logged in a transcript.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    /// Cache writes with the 5-minute TTL.
    pub write_5m: u64,
    /// Cache writes with the 1-hour TTL.
    pub write_1h: u64,
    /// Cache reads (hits).
    pub read: u64,
    pub fast: bool,
}

/// Prices in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub family: &'static str,
    pub input: f64,
    pub output: f64,
    /// Cache read price. Not a fixed fraction of `input`: Fable 5.1 pays 2.5%,
    /// Opus 5.5 and Sonnet 5.5 pay 5%, everything else 10%.
    pub read: f64,
    /// Higher prices for long prompts, where a model has them.
    pub long: Option<LongTier>,
}

/// Prices that apply once a prompt exceeds `threshold` tokens. A prompt counts
/// every input token: fresh input, cache writes and cache reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LongTier {
    pub threshold: u64,
    pub input: f64,
    pub output: f64,
    pub read: f64,
}

struct Row {
    /// The row matches when the lower-cased model id contains any of these.
    patterns: &'static [&'static str],
    price: Price,
}

const fn price(family: &'static str, input: f64, output: f64, read: f64) -> Price {
    Price {
        family,
        input,
        output,
        read,
        long: None,
    }
}

/// Matched in order, so a specific pattern must come before a general one
/// (`fable-5-1` before `fable-5`, `opus-4-5` before `opus-4`).
const ROWS: &[Row] = &[
    Row {
        patterns: &["fable-5-1", "mythos-5-1"],
        price: price("Fable", 10.0, 50.0, 0.25),
    },
    Row {
        patterns: &["fable-5", "mythos-5"],
        price: price("Fable", 10.0, 50.0, 1.0),
    },
    Row {
        patterns: &["opus-5-5"],
        price: price("Opus", 4.0, 20.0, 0.2),
    },
    Row {
        patterns: &["opus-5"],
        price: price("Opus", 5.0, 25.0, 0.5),
    },
    // Opus 4.5 to 4.9 cost a third of Opus 4.1 and earlier.
    Row {
        patterns: &["opus-4-5", "opus-4-6", "opus-4-7", "opus-4-8", "opus-4-9"],
        price: price("Opus", 5.0, 25.0, 0.5),
    },
    Row {
        patterns: &["opus-4", "3-opus"],
        price: price("Opus", 15.0, 75.0, 1.5),
    },
    Row {
        patterns: &["sonnet-5-5"],
        price: price("Sonnet", 2.0, 10.0, 0.1),
    },
    Row {
        patterns: &["sonnet-5"],
        price: price("Sonnet", 2.0, 10.0, 0.2),
    },
    Row {
        patterns: &["sonnet"],
        price: price("Sonnet", 3.0, 15.0, 0.3),
    },
    Row {
        patterns: &["haiku-5"],
        price: Price {
            long: Some(LongTier {
                threshold: 100_000,
                input: 0.5,
                output: 2.5,
                read: 0.05,
            }),
            ..price("Haiku", 0.1, 0.5, 0.01)
        },
    },
    Row {
        patterns: &["haiku-4"],
        price: price("Haiku", 1.0, 5.0, 0.1),
    },
    Row {
        patterns: &["3-5-haiku", "haiku-3-5"],
        price: price("Haiku", 0.8, 4.0, 0.08),
    },
    Row {
        patterns: &["haiku"],
        price: price("Haiku", 0.25, 1.25, 0.03),
    },
];

/// Unknown Claude models are priced like the current Sonnet so they still
/// show up.
const FALLBACK: Price = price("Other", 2.0, 10.0, 0.1);
/// Non-Claude models routed through Claude Code (proxies, gateways) have no
/// Anthropic list price and count as free.
const NON_CLAUDE: Price = price("Other", 0.0, 0.0, 0.0);

pub fn price_for(model: &str) -> Price {
    let id = model.to_ascii_lowercase();
    ROWS.iter()
        .find(|row| row.patterns.iter().any(|pattern| id.contains(pattern)))
        .map(|row| row.price)
        .unwrap_or(if id.contains("claude") {
            FALLBACK
        } else {
            NON_CLAUDE
        })
}

/// `Fable`, `Opus`, `Sonnet`, `Haiku` or `Other`.
pub fn family_of(model: &str) -> &'static str {
    price_for(model).family
}

/// Cost in USD of one API response.
pub fn cost_of(model: &str, usage: &TokenUsage) -> f64 {
    let mut p = price_for(model);
    let prompt = usage.input + usage.write_5m + usage.write_1h + usage.read;
    if let Some(long) = p.long
        && prompt > long.threshold
    {
        p.input = long.input;
        p.output = long.output;
        p.read = long.read;
    }

    let per_million = usage.input as f64 * p.input
        + usage.output as f64 * p.output
        + usage.write_5m as f64 * p.input * WRITE_5M_MULTIPLIER
        + usage.write_1h as f64 * p.input * WRITE_1H_MULTIPLIER
        + usage.read as f64 * p.read;
    let usd = per_million / 1_000_000.0;
    if usage.fast {
        usd * FAST_MULTIPLIER
    } else {
        usd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: u64 = 1_000_000;

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    fn only(f: impl FnOnce(&mut TokenUsage)) -> TokenUsage {
        let mut usage = TokenUsage::default();
        f(&mut usage);
        usage
    }

    /// Cost of one million tokens of a single category, to read prices back.
    fn per_mtok(model: &str, f: impl FnOnce(&mut TokenUsage)) -> f64 {
        cost_of(model, &only(f))
    }

    #[test]
    fn each_family_is_priced_and_labelled() {
        for (model, family, input, output) in [
            ("claude-fable-5-1", "Fable", 10.0, 50.0),
            ("claude-opus-5-5", "Opus", 4.0, 20.0),
            ("claude-sonnet-5-5", "Sonnet", 2.0, 10.0),
            ("claude-haiku-4-5", "Haiku", 1.0, 5.0),
        ] {
            assert_eq!(family_of(model), family);
            assert_close(per_mtok(model, |u| u.input = M), input);
            assert_close(per_mtok(model, |u| u.output = M), output);
        }

        // Haiku 5.5 reaches its list price only below 100k prompt tokens; a
        // million input tokens would already be in the long tier.
        assert_eq!(family_of("claude-haiku-5-5"), "Haiku");
        assert_close(
            per_mtok("claude-haiku-5-5", |u| u.input = 100_000),
            100_000.0 * 0.1 / 1e6,
        );
        assert_close(
            per_mtok("claude-haiku-5-5", |u| u.output = 100_000),
            100_000.0 * 0.5 / 1e6,
        );
    }

    #[test]
    fn cache_writes_cost_more_than_input_by_ttl() {
        let model = "claude-opus-4-5-20251101"; // $5 input
        assert_close(per_mtok(model, |u| u.write_5m = M), 6.25);
        assert_close(per_mtok(model, |u| u.write_1h = M), 10.0);
    }

    #[test]
    fn cache_read_price_differs_per_model() {
        for (model, read) in [
            ("claude-fable-5-1", 0.25),
            ("claude-fable-5", 1.0),
            ("claude-opus-5-5", 0.2),
            ("claude-opus-5", 0.5),
            ("claude-sonnet-5-5", 0.1),
            ("claude-sonnet-5", 0.2),
            ("claude-sonnet-4-6", 0.3),
            ("claude-haiku-4-5", 0.1),
        ] {
            assert_close(per_mtok(model, |u| u.read = M), read);
        }
    }

    #[test]
    fn opus_generations_are_told_apart() {
        for (model, input) in [
            ("claude-opus-5-5-20261001", 4.0),
            ("claude-opus-5-20260601", 5.0),
            ("claude-opus-4-8", 5.0),
            ("claude-opus-4-5-20251101", 5.0),
            ("claude-opus-4-1-20250805", 15.0),
            ("claude-opus-4-20250514", 15.0),
            ("claude-3-opus-20240229", 15.0),
        ] {
            assert_close(per_mtok(model, |u| u.input = M), input);
            assert_eq!(family_of(model), "Opus");
        }
    }

    #[test]
    fn older_haiku_and_sonnet_generations() {
        assert_close(per_mtok("claude-3-5-haiku-20241022", |u| u.input = M), 0.8);
        assert_close(per_mtok("claude-3-haiku-20240307", |u| u.input = M), 0.25);
        assert_close(per_mtok("claude-sonnet-4-5", |u| u.output = M), 15.0);
        assert_close(per_mtok("claude-3-7-sonnet-20250219", |u| u.input = M), 3.0);
    }

    #[test]
    fn haiku_5_prompts_over_100k_use_the_long_tier() {
        let model = "claude-haiku-5-5";
        // The threshold counts the whole prompt: input + cache writes + reads.
        let at_threshold = TokenUsage {
            input: 40_000,
            write_5m: 30_000,
            read: 30_000,
            output: M,
            ..TokenUsage::default()
        };
        assert_eq!(
            at_threshold.input + at_threshold.write_5m + at_threshold.read,
            100_000
        );
        // Not "over": standard prices.
        assert_close(
            cost_of(model, &at_threshold),
            (40_000.0 * 0.1 + 30_000.0 * 0.1 * 1.25 + 30_000.0 * 0.01 + M as f64 * 0.5) / 1e6,
        );

        let over = TokenUsage {
            read: at_threshold.read + 1,
            ..at_threshold
        };
        assert_close(
            cost_of(model, &over),
            (40_000.0 * 0.5 + 30_000.0 * 0.5 * 1.25 + 30_001.0 * 0.05 + M as f64 * 2.5) / 1e6,
        );
    }

    #[test]
    fn long_tier_is_haiku_5_only() {
        // 1M tokens of context at standard pricing on every other model.
        let usage = TokenUsage {
            input: 900_000,
            ..TokenUsage::default()
        };
        assert_close(cost_of("claude-sonnet-5-5", &usage), 0.9 * 2.0);
    }

    #[test]
    fn fast_mode_doubles_everything_including_cache() {
        let usage = TokenUsage {
            input: M,
            output: M,
            write_5m: M,
            write_1h: M,
            read: M,
            fast: false,
        };
        let standard = cost_of("claude-opus-5-5", &usage);
        let fast = cost_of(
            "claude-opus-5-5",
            &TokenUsage {
                fast: true,
                ..usage
            },
        );
        assert_close(fast, standard * 2.0);
        // 8 + 40 for input/output, per the fast-mode table.
        assert_close(
            cost_of(
                "claude-opus-5-5",
                &TokenUsage {
                    input: M,
                    output: M,
                    fast: true,
                    ..TokenUsage::default()
                },
            ),
            48.0,
        );
    }

    #[test]
    fn unknown_models() {
        // Unknown Claude: like the current Sonnet.
        assert_eq!(family_of("claude-future-9"), "Other");
        assert_close(per_mtok("claude-future-9", |u| u.input = M), 2.0);
        // Not Claude at all: free.
        assert_eq!(family_of("gpt-5"), "Other");
        assert_close(per_mtok("gpt-5", |u| u.output = M), 0.0);
        assert_close(per_mtok("", |u| u.input = M), 0.0);
    }

    #[test]
    fn model_ids_are_matched_case_insensitively() {
        assert_eq!(family_of("Claude-Opus-4-5"), "Opus");
    }

    #[test]
    fn mythos_is_priced_like_fable() {
        assert_close(per_mtok("claude-mythos-5-1", |u| u.read = M), 0.25);
        assert_close(per_mtok("claude-mythos-5", |u| u.input = M), 10.0);
        assert_eq!(family_of("claude-mythos-5"), "Fable");
    }
}
