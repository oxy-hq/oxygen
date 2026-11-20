//! What a run's tokens cost: the SQL columns that read them and the rate lookup
//! that prices them.
//!
//! Admin -> Metrics and `ctx.platform.facts()` both read a run's tokens with
//! [`RUN_TOKENS`] and price them with [`price`], so an account's cost to serve
//! and the Metrics page come from one rate table.

use agentic_llm::pricing::cost_for_call;

/// One run's model and token sums, the columns every rollup here prices. Each
/// run collapses to ONE model (the max `llm_end.model` — runs are effectively
/// single-model), so downstream GROUP BYs can attribute the run's tokens to a
/// priced model bucket. Token casts mirror `agentic_runtime`'s per-run usage
/// query.
///
/// Reads the run's `agentic_run_events` aliased `e`, joined with
/// `e.event_type IN ('llm_start', 'llm_end')` (so a run that made no LLM call
/// is no row at all, rather than an unpriced one) and grouped by run. Shared
/// with `ctx.platform.facts()`'s LLM cost, which prices with
/// `metrics_pricing::price`, so the facts and this page cannot price a run
/// differently.
pub const RUN_TOKENS: &str = "\
    max(e.payload->>'model') FILTER (WHERE e.event_type = 'llm_end') AS model, \
    COALESCE(SUM((e.payload->>'prompt_tokens')::bigint) \
        FILTER (WHERE e.event_type = 'llm_start'), 0) AS input_tokens, \
    COALESCE(SUM((e.payload->>'output_tokens')::bigint) \
        FILTER (WHERE e.event_type = 'llm_end'), 0) AS output_tokens, \
    COALESCE(SUM((e.payload->>'cache_creation_input_tokens')::bigint) \
        FILTER (WHERE e.event_type = 'llm_end'), 0) AS cache_creation, \
    COALESCE(SUM((e.payload->>'cache_read_input_tokens')::bigint) \
        FILTER (WHERE e.event_type = 'llm_end'), 0) AS cache_read";

/// Price a single model bucket. `None` model or an unknown model yields
/// `None` cost (tokens still count toward usage, just not dollars).
///
/// The one place a token rollup becomes dollars: `ctx.platform.facts()` prices
/// its per-window LLM cost with it too (over `metrics_rollup::RUN_TOKENS`), so
/// an account's cost there and on this page come from one rate table.
pub fn price(model: &Option<String>, input: i64, output: i64, cc: i64, cr: i64) -> Option<f64> {
    let model = model.as_deref()?;
    cost_for_call(
        model,
        input.max(0) as u64,
        output.max(0) as u64,
        cc.max(0) as u64,
        cr.max(0) as u64,
    )
}
