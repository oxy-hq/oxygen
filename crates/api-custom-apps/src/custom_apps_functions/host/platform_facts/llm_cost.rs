//! `llmCostUsd` and `llmUnpricedRuns`: what an org's agent runs cost in LLM
//! usage, per window, priced the way Admin → Metrics prices it.
//!
//! **The runs are `agentRuns`' runs.** A window's roots are the top-level runs
//! `agentRuns` counts in it (`exclude_daemon_runs`: no platform daemon, no
//! staff preview or dry run), and each brings every run it delegated to, at
//! any depth. A delegated run is created without its root's source or
//! metadata, so it cannot be judged on its own row: it is its root's cost, in
//! its root's window. A window's cost is therefore what that window's
//! `agentRuns` cost. Admin → Metrics' per-org figure prices every run in the
//! org's workspaces, so it reads higher by exactly the platform's and staff's
//! own runs (and its days are rolling, not ISO weeks).
//!
//! **Priced as Admin → Metrics prices**: the same per-run model and token sums
//! (`metrics_rollup::RUN_TOKENS`), summed per org, window and model, and the
//! same rates (`metrics_pricing::price`). A model the rate table lacks adds
//! nothing to the dollars and counts its runs in `llmUnpricedRuns`, as Admin →
//! Metrics' `run_count - priced_run_count` does. Tokens and a model name are
//! all it reads from an event: no prompt, no answer.
//!
//! **Bounded like `agentRuns`**: the roots are cut to the windows' outer
//! bounds, the descent follows the `parent_run_id` index and the events their
//! `(run_id, seq)` index.

use sea_orm::FromQueryResult;
use uuid::Uuid;

use super::OxyPlatformWeek;
use super::queries::WINDOW_CTES;
use oxy_usage_report::customer_runs::exclude_daemon_runs;
use oxy_usage_report::llm_usage::RUN_TOKENS;
use oxy_usage_report::llm_usage::price;

/// One org's LLM usage in one window on one model, not yet priced.
#[derive(Clone, Debug, Default, FromQueryResult)]
pub(super) struct LlmCostRow {
    pub org_id: Uuid,
    pub idx: i64,
    /// `None` for runs whose `llm_end` events named no model: unpriced.
    pub model: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation: i64,
    pub cache_read: i64,
    /// Runs on `model` that made an LLM call, delegated runs included.
    pub runs: i64,
}

/// The windowed read, with the six parameters every windowed read takes
/// (`queries`), `RECURSIVE` for the descent from each root to its delegated
/// runs. `UNION`, not `UNION ALL`, so the descent ends even on a cycle the
/// schema does not forbid.
pub(super) fn sql() -> String {
    format!(
        "WITH RECURSIVE {WINDOW_CTES}, \
         roots AS ( \
             SELECT r.id, w.org_id, win.idx \
             FROM agentic_runs r \
             JOIN workspaces w ON w.id = r.workspace_id \
             JOIN win ON r.created_at >= win.starts_at AND r.created_at < win.ends_at \
             WHERE w.org_id IS NOT NULL AND r.parent_run_id IS NULL \
               AND r.created_at >= $4 AND r.created_at < $5{customer}), \
         tree AS ( \
             SELECT id, org_id, idx FROM roots \
             UNION \
             SELECT c.id, t.org_id, t.idx \
             FROM agentic_runs c JOIN tree t ON c.parent_run_id = t.id), \
         run_usage AS ( \
             SELECT t.org_id, t.idx, {RUN_TOKENS} \
             FROM tree t \
             JOIN agentic_run_events e \
                 ON e.run_id = t.id AND e.event_type IN ('llm_start', 'llm_end') \
             GROUP BY t.org_id, t.idx, t.id) \
         SELECT org_id, idx, model, \
                SUM(input_tokens)::bigint AS input_tokens, \
                SUM(output_tokens)::bigint AS output_tokens, \
                SUM(cache_creation)::bigint AS cache_creation, \
                SUM(cache_read)::bigint AS cache_read, \
                COUNT(*)::bigint AS runs \
         FROM run_usage GROUP BY org_id, idx, model",
        customer = exclude_daemon_runs("r."),
    )
}

/// Prices `row` into `week`: its dollars, or, on a model the rate table
/// lacks, its runs as unpriced and no dollars.
pub(super) fn add(week: &mut OxyPlatformWeek, row: &LlmCostRow) {
    let usd = price(
        &row.model,
        row.input_tokens,
        row.output_tokens,
        row.cache_creation,
        row.cache_read,
    );
    match usd {
        Some(usd) => week.llm_cost_usd += usd,
        None => week.llm_unpriced_runs += u64::try_from(row.runs).unwrap_or(0),
    }
}
