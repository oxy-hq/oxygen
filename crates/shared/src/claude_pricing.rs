//! Anthropic list prices: the one table both `agentic_llm::pricing` (the react
//! cost cap, run costs) and `oxy_observability::execution_analytics` (the cost pages) read,
//! so the two can never price one model differently.

/// Per-million-token USD rates for one Claude model. `cache_write` is the
/// 5-minute-TTL write rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClaudeRates {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_read: f64,
}

const fn r(input: f64, output: f64, cache_write: f64, cache_read: f64) -> ClaudeRates {
    ClaudeRates {
        input,
        output,
        cache_write,
        cache_read,
    }
}

/// Checked 2026-11-22 against the published price list. Longest ids first:
/// `claude-opus-5-5` must win over `claude-opus-5`, and an id matches only at a
/// version boundary, so a dated `claude-fable-5-20260101` is Fable 5, not 5.2.
const CLAUDE_RATES: &[(&str, ClaudeRates)] = &[
    ("claude-fable-5-2", r(8.0, 40.0, 10.0, 0.20)),
    ("claude-mythos-5-2", r(8.0, 40.0, 10.0, 0.20)),
    ("claude-fable-5", r(10.0, 50.0, 12.5, 1.0)),
    ("claude-mythos-5", r(10.0, 50.0, 12.5, 1.0)),
    ("claude-opus-5-5", r(4.0, 20.0, 5.0, 0.20)),
    ("claude-opus-5", r(5.0, 25.0, 6.25, 0.50)),
    ("claude-opus-4-8", r(5.0, 25.0, 6.25, 0.50)),
    ("claude-opus-4-7", r(5.0, 25.0, 6.25, 0.50)),
    ("claude-opus-4-6", r(5.0, 25.0, 6.25, 0.50)),
    ("claude-opus-4-5", r(5.0, 25.0, 6.25, 0.50)),
    ("claude-opus-4", r(15.0, 75.0, 18.75, 1.50)),
    ("claude-sonnet-5-5", r(2.0, 10.0, 2.5, 0.10)),
    ("claude-sonnet-5", r(2.0, 10.0, 2.5, 0.20)),
    ("claude-sonnet-4", r(3.0, 15.0, 3.75, 0.30)),
    // Haiku 5.5 up to 100K-token prompts; longer prompts cost 5x.
    ("claude-haiku-5", r(0.10, 0.50, 0.125, 0.01)),
    ("claude-haiku-4", r(1.0, 5.0, 1.25, 0.10)),
    // Claude 3 ids put the version first.
    ("claude-3-7-sonnet", r(3.0, 15.0, 3.75, 0.30)),
    ("claude-3-5-sonnet", r(3.0, 15.0, 3.75, 0.30)),
    ("claude-3-5-haiku", r(0.80, 4.0, 1.0, 0.08)),
    ("claude-3-opus", r(15.0, 75.0, 18.75, 1.50)),
    ("claude-3-haiku", r(0.25, 1.25, 0.30, 0.03)),
];

/// The list row for `model`, or `None` when the list names no such model.
///
/// The id is read from its last `claude-`, so a vendor-prefixed
/// `us.anthropic.claude-opus-4-1-…` matches, and with dots read as dashes, so a
/// gateway's `anthropic/claude-opus-4.6` matches. A row matches as a prefix
/// that ends the id or is followed by `-`, `@` or `[`. Case-sensitive: callers
/// that accept mixed case lowercase first.
pub fn claude_rates(model: &str) -> Option<ClaudeRates> {
    let key = model
        .rfind("claude-")
        .map_or(model, |i| &model[i..])
        .replace('.', "-");
    CLAUDE_RATES.iter().find_map(|(id, rates)| {
        let rest = key.strip_prefix(id)?;
        rest.chars()
            .next()
            .is_none_or(|c| matches!(c, '-' | '@' | '['))
            .then_some(*rates)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io(model: &str) -> Option<(f64, f64)> {
        claude_rates(model).map(|r| (r.input, r.output))
    }

    #[test]
    fn current_and_older_ids_resolve_to_their_rows() {
        for (model, want) in [
            ("claude-fable-5-2", (8.0, 40.0)),
            ("claude-fable-5-20260101", (10.0, 50.0)),
            ("claude-opus-5-5", (4.0, 20.0)),
            ("claude-opus-5", (5.0, 25.0)),
            ("claude-opus-4-8", (5.0, 25.0)),
            ("claude-opus-4-1-20250805", (15.0, 75.0)),
            ("claude-sonnet-5-5", (2.0, 10.0)),
            ("claude-sonnet-4-6", (3.0, 15.0)),
            ("claude-haiku-5-5", (0.10, 0.50)),
            ("claude-haiku-4-5-20251001", (1.0, 5.0)),
            ("claude-3-5-haiku-20241022", (0.80, 4.0)),
            ("claude-opus-4-8[1m]", (5.0, 25.0)),
            ("claude-sonnet-5-5@20261101", (2.0, 10.0)),
        ] {
            assert_eq!(io(model), Some(want), "{model}");
        }
    }

    #[test]
    fn vendor_prefixed_and_dotted_ids_match() {
        assert_eq!(
            io("us.anthropic.claude-opus-4-1-20250805-v1:0"),
            Some((15.0, 75.0))
        );
        assert_eq!(io("us.anthropic.claude-sonnet-5-5-v1:0"), Some((2.0, 10.0)));
        assert_eq!(io("anthropic/claude-opus-4.6"), Some((5.0, 25.0)));
        assert_eq!(io("anthropic/claude-sonnet-5.5"), Some((2.0, 10.0)));
    }

    #[test]
    fn a_row_matches_only_at_a_version_boundary() {
        assert_eq!(io("claude-opus-50"), None);
        assert_eq!(io("claude-zephyr-9"), None);
        assert_eq!(io("gpt-5"), None);
    }

    #[test]
    fn rows_are_ordered_so_no_row_is_shadowed() {
        for (i, (id, _)) in CLAUDE_RATES.iter().enumerate() {
            for (earlier, _) in &CLAUDE_RATES[..i] {
                let shadowed = id
                    .strip_prefix(earlier)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'));
                assert!(!shadowed, "{earlier} shadows {id}");
            }
        }
    }
}
