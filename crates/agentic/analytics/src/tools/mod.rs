//! Tool definitions and executors for the analytics domain.
//!
//! Tools are thin wrappers around [`Catalog`] and [`DatabaseConnector`] — they
//! expose raw data and let the LLM reason.  No validation logic lives inside a tool.
//!
//! # Scoping
//!
//! | State | Tools |
//! |---|---|
//! | `triage` | `search_automations` |
//! | `clarifying` | `search_catalog`, `list_tables`, `describe_table` |
//! | `specifying` | `search_catalog`, `sample_columns`, `get_join_path`, `list_tables`, `describe_table` |
//! | `solving` | `execute_preview` |
//! | `interpreting` | `render_chart` |
//!
//! `list_metrics` and `list_dimensions` were removed — `search_catalog`
//! returns both metrics and dimensions in a single call.
//! `get_metric_definition` was removed — `search_catalog` now includes the
//! formula/expression for each metric, making the separate lookup redundant.
//! `get_valid_dimensions` was removed — redundant with catalog context already
//! in the LLM prompt.  `get_column_range` was replaced by `sample_column` which
//! runs a live query instead of returning stale pre-computed catalog data.
//! `explain_plan` (stub) and `dry_run` (table-name substring check) were replaced
//! by `execute_preview` which runs the SQL with LIMIT 5 and returns real results.
//!
//! Implementation is split across sibling modules by concern:
//! - [`defs`]: tool definitions (ToolDef factories) per FSM state.
//! - [`clarifying`] / [`specifying`] / [`solving`] / [`interpreting`]: tool executors.
//! - [`database`]: `list_tables` / `describe_table` executors + schema cache.

use agentic_core::result::CellValue;
use agentic_core::tools::ToolError;
use serde_json::{Value, json};

pub mod anomalies;
pub mod clarifying;
pub mod database;
pub mod defs;
pub mod interpreting;
pub mod metric_tree;
pub mod solving;
pub mod specifying;

#[cfg(test)]
mod freshness_probe_tests;
#[cfg(test)]
mod tests;

pub use anomalies::{AnomalyToolContext, execute_anomaly_tool};
pub use clarifying::execute_clarifying_tool;
pub use database::{SchemaCache, execute_database_lookup_tool, new_schema_cache};
#[allow(unused_imports)]
pub use defs::{
    METRIC_TREE_TOOL_NAMES, anomaly_tools, clarifying_tools, interpreting_tools,
    is_metric_tree_tool, metric_tree_tools, propose_semantic_query_tool, solving_tools,
    specifying_tools, suggest_chart_config, triage_tools,
};
#[allow(unused_imports)]
pub use interpreting::{
    execute_interpreting_tool, validate_chart_column_types, validate_chart_config,
};
pub use metric_tree::execute_metric_tree_tool;
pub use solving::execute_solving_tool;
pub use specifying::execute_specifying_tool;

// ── Shared tool description strings ──────────────────────────────────────────
//
// Kept as constants so that triage, clarifying, and specifying stages stay in
// sync without copy-paste drift.

pub(super) const SEARCH_CATALOG_DESC: &str = "Batch-search the semantic catalog for measures AND dimensions in one call. \
     Use this to check whether the catalog has all the members needed to answer \
     the question before attempting a semantic shortcut. Returns \
     {metrics: [{name, description}], dimensions: [{name, description, type}]}.";

pub(super) const SEARCH_AUTOMATIONS_DESC: &str = "Search for existing automation YAML files and verified SQL files \
     that match a query. Returns a list of {name, path, description} entries. \
     Call this FIRST with key terms from the user's question. \
     If any entry directly answers the question, select it. \
     SQL files (.sql) are executed directly as verified queries — prefer them when available.";

pub(super) const SAMPLE_COLUMNS_DESC: &str = "Batch-sample multiple columns in one call. For each column, returns up \
     to 20 distinct non-null values plus statistics (row_count, distinct_count, \
     min, max; also avg and stdev for numeric columns). Accepts semantic view \
     names and dimension names as well as raw database table/column names. \
     Use this to verify filter values, confirm exact formats, and choose \
     date granularity — all in a single round-trip instead of calling \
     sample_column multiple times.";

pub(super) const CHECK_DATA_FRESHNESS_DESC: &str = "Check how current each semantic view's data is. For each view, \
     returns data_through (MAX of its watermark date column), staleness_days, \
     and — when the view declares them — its expected_cadence, \
     complete_through policy, and settlement caveat. Judge staleness against \
     the declared cadence: a monthly-loaded source 20 days behind is healthy, \
     an hourly one is not. Use BEFORE finalizing a query whose date range \
     touches the recent edge (today/yesterday/this week/month-to-date) or a \
     manually-loaded source, so filters and the eventual answer reflect the \
     data that actually exists. Returns {results: [{view, data_through, \
     staleness_days, expected_cadence?, complete_through?, caveat?}]}.";

// ── Shared observability helpers ─────────────────────────────────────────────

/// Emit a visible `tool.input` event on the current span.
pub(super) fn emit_tool_input(name: &str, params: &Value) {
    let input = serde_json::to_string(params).unwrap_or_default();
    let truncated = truncate_str(&input, 2000);
    tracing::info!(
        name: "tool.input",
        is_visible = true,
        tool_name = %name,
        input = %truncated,
    );
}

/// Emit a visible `tool.output` event on the current span.
pub(super) fn emit_tool_output(output: &Value) {
    let text = serde_json::to_string(output).unwrap_or_default();
    let truncated = truncate_str(&text, 4000);
    tracing::info!(
        name: "tool.output",
        is_visible = true,
        output = %truncated,
    );
}

/// Emit a visible `tool.error` event on the current span.
pub(super) fn emit_tool_error(err: &ToolError) {
    tracing::info!(
        name: "tool.output",
        is_visible = true,
        status = "error",
        error = %err,
    );
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}… ({} chars total)", &s[..max], s.len())
    }
}

// ── Shared cell → JSON conversion ────────────────────────────────────────────

pub(super) fn cell_to_json(cell: &CellValue) -> Value {
    match cell {
        CellValue::Text(s) => Value::String(s.clone()),
        CellValue::Number(n) => json!(n),
        CellValue::Null => Value::Null,
    }
}
