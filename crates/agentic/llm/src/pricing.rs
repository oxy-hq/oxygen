//! Per-model token pricing for run-cost summaries on the coordinator
//! dashboard.
//!
//! Rates are **input-per-million / output-per-million** USD as published
//! by the providers. Cache writes are billed at a markup above input;
//! cache reads at a steep discount. The table is intentionally small —
//! unknown models surface as `cost = None` on the run detail rather than
//! a fabricated number, so a model addition is visible (cost vanishes
//! from the UI) until someone updates this file.
//!
//! Sources: Anthropic & OpenAI public pricing pages, as of the run that
//! `cost_for_model` is called against. If pricing changes after a run
//! completes, the historical cost we display will be slightly off — an
//! acceptable trade for not persisting per-call cost at write time.

/// Per-million-token rates for one model.
#[derive(Debug, Clone, Copy)]
pub struct ModelPricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
    /// Cache *writes* (Anthropic only today). Falls back to input rate
    /// when unset so cache_creation tokens never silently cost nothing.
    pub cache_creation_per_million: Option<f64>,
    /// Cache *reads*. Falls back to a 10× discount on input when unset.
    pub cache_read_per_million: Option<f64>,
}

impl ModelPricing {
    pub const fn new(input: f64, output: f64) -> Self {
        Self {
            input_per_million: input,
            output_per_million: output,
            cache_creation_per_million: None,
            cache_read_per_million: None,
        }
    }

    pub const fn with_cache(mut self, creation: f64, read: f64) -> Self {
        self.cache_creation_per_million = Some(creation);
        self.cache_read_per_million = Some(read);
        self
    }
}

/// The shared Anthropic list row for `model` (`oxy_shared::claude_pricing`,
/// the table the platform's cost pages read too).
fn claude_rates(model: &str) -> Option<ModelPricing> {
    oxy_shared::claude_pricing::claude_rates(model)
        .map(|r| ModelPricing::new(r.input, r.output).with_cache(r.cache_write, r.cache_read))
}

/// Resolve the published rates for `model`. The lookup is a series of
/// prefix matches so model variants (e.g. `claude-sonnet-4-6-20251022`)
/// hit the same rate as the canonical id. Returns `None` for any model
/// not in the table — the caller surfaces this as "cost unavailable".
pub fn rates_for(model: &str) -> Option<ModelPricing> {
    if let Some(p) = claude_rates(model) {
        return Some(p);
    }

    // OpenAI — GPT-5 generation. `-mini` / `-nano` variants are cheaper tiers
    // and must be matched before the bare `gpt-5` prefix.
    if model.starts_with("gpt-5-nano") {
        return Some(ModelPricing::new(0.05, 0.40));
    }
    if model.starts_with("gpt-5-mini") {
        return Some(ModelPricing::new(0.25, 2.0));
    }
    if model.starts_with("gpt-5") {
        return Some(ModelPricing::new(1.25, 10.0));
    }

    // OpenAI — GPT-4o generation.
    if model.starts_with("gpt-4o-mini") {
        return Some(ModelPricing::new(0.15, 0.60));
    }
    if model.starts_with("gpt-4o") {
        return Some(ModelPricing::new(2.50, 10.0));
    }

    // OpenAI — reasoning (o-series). `o3-mini` before `o3`, `o1-mini` before
    // `o1`, so a bare-family prefix never shadows the cheaper mini tier.
    if model.starts_with("o3-mini") {
        return Some(ModelPricing::new(1.10, 4.40));
    }
    if model.starts_with("o3") {
        return Some(ModelPricing::new(2.0, 8.0));
    }
    if model.starts_with("o1-mini") {
        return Some(ModelPricing::new(1.10, 4.40));
    }
    if model.starts_with("o1") {
        return Some(ModelPricing::new(15.0, 60.0));
    }

    None
}

/// Compute the USD cost of one call given its token counts. Returns
/// `None` only when the model isn't in the pricing table — partial
/// information (e.g. zero cache tokens) is fine.
pub fn cost_for_call(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
) -> Option<f64> {
    let p = rates_for(model)?;
    // Fallbacks mirror the docstring on `ModelPricing`: cache writes
    // default to the input rate, cache reads to a 10× input discount.
    let cache_creation_rate = p.cache_creation_per_million.unwrap_or(p.input_per_million);
    let cache_read_rate = p
        .cache_read_per_million
        .unwrap_or(p.input_per_million / 10.0);
    let cost = (input_tokens as f64) * p.input_per_million / 1_000_000.0
        + (output_tokens as f64) * p.output_per_million / 1_000_000.0
        + (cache_creation_tokens as f64) * cache_creation_rate / 1_000_000.0
        + (cache_read_tokens as f64) * cache_read_rate / 1_000_000.0;
    Some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sonnet_basic_cost() {
        // 1M input + 1M output on Sonnet 4 = $3 + $15 = $18.
        let cost = cost_for_call("claude-sonnet-4-6", 1_000_000, 1_000_000, 0, 0).unwrap();
        assert!((cost - 18.0).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn sonnet_with_cache() {
        // 1M cache read = 1M * $0.30/M = $0.30.
        let cost = cost_for_call("claude-sonnet-4-6", 0, 0, 0, 1_000_000).unwrap();
        assert!((cost - 0.30).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(cost_for_call("gpt-99-future", 1, 1, 0, 0).is_none());
    }

    #[test]
    fn claude_5_family_is_priced() {
        assert!(rates_for("claude-opus-5").is_some());
        assert!(rates_for("claude-sonnet-5").is_some());
        assert!(rates_for("claude-sonnet-5-20260101").is_some());
    }

    /// The published list prices, one row per current model. A wrong row here
    /// overstated Opus 4.8 three times over, and the react cost cap reads it.
    #[test]
    fn claude_list_prices() {
        for (model, input, output) in [
            ("claude-fable-5-2", 8.0, 40.0),
            ("claude-fable-5-1", 10.0, 50.0),
            ("claude-fable-5", 10.0, 50.0),
            ("claude-fable-5-20260101", 10.0, 50.0),
            ("claude-opus-5-5", 4.0, 20.0),
            ("claude-opus-5", 5.0, 25.0),
            ("claude-opus-4-8", 5.0, 25.0),
            ("claude-opus-4-6", 5.0, 25.0),
            ("claude-opus-4-1", 15.0, 75.0),
            ("claude-sonnet-5-5", 2.0, 10.0),
            ("claude-sonnet-5", 2.0, 10.0),
            ("claude-sonnet-4-6", 3.0, 15.0),
            ("claude-haiku-5-5", 0.10, 0.50),
            ("claude-haiku-4-5-20251001", 1.0, 5.0),
            ("us.anthropic.claude-opus-4-1-20250805-v1:0", 15.0, 75.0),
            ("us.anthropic.claude-sonnet-5-5-v1:0", 2.0, 10.0),
            ("claude-3-5-haiku-20241022", 0.80, 4.0),
            ("claude-3-opus-20240229", 15.0, 75.0),
            ("anthropic/claude-opus-4.6", 5.0, 25.0),
            ("anthropic/claude-fable-5.2", 8.0, 40.0),
            ("anthropic/claude-opus-5.5", 4.0, 20.0),
        ] {
            let p = rates_for(model).unwrap_or_else(|| panic!("{model} unpriced"));
            assert_eq!(
                (p.input_per_million, p.output_per_million),
                (input, output),
                "{model}"
            );
        }
        assert_eq!(
            rates_for("claude-sonnet-5-5")
                .unwrap()
                .cache_read_per_million,
            Some(0.10)
        );
        assert_eq!(
            rates_for("claude-opus-5-5").unwrap().cache_read_per_million,
            Some(0.20)
        );
    }

    #[test]
    fn gpt5_and_o3_are_priced() {
        assert!(rates_for("gpt-5").is_some());
        assert!(rates_for("gpt-5-mini").is_some());
        assert!(rates_for("gpt-5-nano").is_some());
        assert!(rates_for("o3").is_some());
        assert!(rates_for("o3-mini").is_some());
    }

    #[test]
    fn family_mini_prefixes_win_over_bare_family() {
        // `gpt-5-mini` must NOT resolve to the pricier bare `gpt-5` tier, and
        // `o3-mini` must not resolve to the bare `o3` tier.
        assert_ne!(
            rates_for("gpt-5-mini").map(|p| p.input_per_million),
            rates_for("gpt-5").map(|p| p.input_per_million)
        );
        assert_ne!(
            rates_for("o3-mini").map(|p| p.input_per_million),
            rates_for("o3").map(|p| p.input_per_million)
        );
    }

    #[test]
    fn haiku_4_5_matches_claude_4_family() {
        // `claude-haiku-4-5` is a Claude 4.5 haiku id — must resolve to the
        // haiku-4 rate card, not fall through to None.
        assert_eq!(
            rates_for("claude-haiku-4-5").map(|p| p.input_per_million),
            rates_for("claude-haiku-4").map(|p| p.input_per_million),
        );
    }

    #[test]
    fn variant_suffix_matches_base() {
        let canon = cost_for_call("claude-sonnet-4-6", 1000, 1000, 0, 0);
        let variant = cost_for_call("claude-sonnet-4-6-20251022", 1000, 1000, 0, 0);
        assert_eq!(canon, variant);
    }

    #[test]
    fn default_model_is_priced() {
        // The platform default must always resolve to a rate — otherwise every
        // fall-through run (LlmClient::new, empty model_ref) silently reports a
        // "cost unavailable". Guards against the default drifting to an id this
        // table doesn't know.
        assert!(
            rates_for(crate::DEFAULT_MODEL).is_some(),
            "DEFAULT_MODEL {} has no pricing entry",
            crate::DEFAULT_MODEL
        );
    }
}
