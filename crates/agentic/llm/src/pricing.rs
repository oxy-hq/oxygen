//! Per-model token pricing for run-cost summaries on the coordinator
//! dashboard.
//!
//! Rates are **input-per-million / output-per-million** USD as published
//! by the providers. Cache writes are billed at a markup above input;
//! cache reads at a steep discount.
//!
//! Rates come from the `llm-prices` crate first: the providers' list prices,
//! each row citing the pricing page it was read from and the date it was
//! checked, with the provider's own cache-write (5-minute) and cache-read
//! rates. Dated ids (`claude-sonnet-4-5-20250929`) resolve to their base row.
//! Ids it does not know fall back to the prefix table below, and anything
//! neither knows surfaces as `cost = None` on the run detail rather than a
//! fabricated number.
//!
//! If pricing changes after a run completes, the historical cost we display
//! will be slightly off — an acceptable trade for not persisting per-call
//! cost at write time.

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

/// Resolve the published rates for `model`. The lookup is a series of
/// prefix matches so model variants (e.g. `claude-sonnet-4-6-20251022`)
/// hit the same rate as the canonical id. Returns `None` for any model
/// not in the table — the caller surfaces this as "cost unavailable".
pub fn rates_for(model: &str) -> Option<ModelPricing> {
    if let Some(p) = list_rates(model) {
        return Some(p);
    }
    prefix_rates(model)
}

/// The provider's list price from `llm-prices`, when it has both an input and
/// an output price for `model`. Cache rates stay `None` when the provider
/// lists none, so `cost_for_call`'s fallbacks apply as before.
fn list_rates(model: &str) -> Option<ModelPricing> {
    let p = llm_prices::lookup(model)?;
    Some(ModelPricing {
        input_per_million: p.input?,
        output_per_million: p.output?,
        cache_creation_per_million: p.cache_write("5m"),
        cache_read_per_million: p.cache_read,
    })
}

/// Hand-kept prefix rates for ids `llm-prices` does not know.
fn prefix_rates(model: &str) -> Option<ModelPricing> {
    // Anthropic — Claude 5 family. Priced at the same published tiers as the
    // Claude 4 generation (opus/sonnet/haiku). Checked before the `-4`
    // prefixes below so a `claude-*-5` id never accidentally matches a shorter
    // family prefix. `claude-haiku-4-5` is intentionally a Claude 4.5 haiku id
    // (matched by the `claude-haiku-4` arm), not a 5-family model.
    if model.starts_with("claude-opus-5") {
        return Some(ModelPricing::new(15.0, 75.0).with_cache(18.75, 1.50));
    }
    if model.starts_with("claude-sonnet-5") {
        return Some(ModelPricing::new(3.0, 15.0).with_cache(3.75, 0.30));
    }
    if model.starts_with("claude-haiku-5") {
        return Some(ModelPricing::new(0.80, 4.0).with_cache(1.0, 0.08));
    }

    // Anthropic — Claude 4 family (incl. 4.5 point releases like
    // `claude-haiku-4-5`, which share the 4-family rate card).
    if model.starts_with("claude-opus-4") {
        return Some(ModelPricing::new(15.0, 75.0).with_cache(18.75, 1.50));
    }
    if model.starts_with("claude-sonnet-4") {
        return Some(ModelPricing::new(3.0, 15.0).with_cache(3.75, 0.30));
    }
    if model.starts_with("claude-haiku-4") {
        return Some(ModelPricing::new(0.80, 4.0).with_cache(1.0, 0.08));
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
        // Claude 5 ids (already referenced in-repo, e.g. `claude-sonnet-5`)
        // must resolve to a rate rather than showing blank cost.
        assert!(rates_for("claude-opus-5").is_some());
        assert!(rates_for("claude-sonnet-5").is_some());
        assert!(rates_for("claude-sonnet-5-20260101").is_some());
        // Sonnet 5 lists at $2 / $10, below the Sonnet 4 rate card.
        let sonnet_5 = cost_for_call("claude-sonnet-5", 1_000_000, 1_000_000, 0, 0).unwrap();
        assert!((sonnet_5 - 12.0).abs() < 1e-9, "got {sonnet_5}");
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
    fn haiku_4_5_is_priced_at_its_own_rate() {
        // `claude-haiku-4-5` lists at $1 / $5, not Haiku 3.5's $0.80 / $4.
        let p = rates_for("claude-haiku-4-5").unwrap();
        assert!((p.input_per_million - 1.0).abs() < 1e-9);
        assert!((p.output_per_million - 5.0).abs() < 1e-9);
    }

    #[test]
    fn default_model_bills_at_list_price() {
        // The platform default, claude-opus-4-8, lists at $5 / $25 with $6.25
        // cache writes and $0.50 cache reads — not the Opus 4.0 $15 / $75.
        let cost = cost_for_call(
            crate::DEFAULT_MODEL,
            1_000_000,
            1_000_000,
            1_000_000,
            1_000_000,
        )
        .unwrap();
        assert!(
            (cost - (5.0 + 25.0 + 6.25 + 0.50)).abs() < 1e-9,
            "got {cost}"
        );
    }

    #[test]
    fn openai_cache_reads_use_the_listed_rate() {
        // gpt-4o cache reads list at $1.25/M (half of input), not input / 10.
        let cost = cost_for_call("gpt-4o", 0, 0, 0, 1_000_000).unwrap();
        assert!((cost - 1.25).abs() < 1e-9, "got {cost}");
    }

    #[test]
    fn ids_missing_from_list_prices_use_the_prefix_table() {
        // A future dated Sonnet 5 id is not in llm-prices; the prefix arm
        // still prices it rather than returning None.
        assert!(rates_for("claude-sonnet-5-20991231-preview").is_some());
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
