//! `/admin/metrics/*` — cross-tenant operator metrics for the admin
//! dashboard. The headline is **LLM cost**: token usage is persisted in
//! `agentic_run_events` (JSONB payloads on `llm_start`/`llm_end`), but dollar
//! cost is *computed at read time* from per-model rates
//! (`agentic_llm::pricing`). So this module sums tokens in SQL, then prices
//! each model bucket in Rust.
//!
//! Gated on `Action::PlatformOperate` by `admin::router`. "Cross-tenant" is what
//! an unbounded grant sees: the gate cannot see grant scope, so the overview
//! narrows its run rollup to the orgs a bounded grant names (in the CTE, so the
//! totals, the day series, the model split and the org leaderboard all agree),
//! and the per-org detail fences on its path org.
//!
//! Split by responsibility across sibling modules: `metrics_rollup` holds the SQL
//! (the run-usage CTE and the token rollups), `metrics_pricing` prices and folds the
//! rows in Rust. This file keeps the routes, the response shapes and the handlers.

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query};
use axum::response::Response;
use axum::routing::get;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::internal_jobs::{connect, db_err};
use super::metrics_pricing::{build_org_detail, build_overview};
use super::metrics_rollup::{fetch_day_model_rows, fetch_org_model_rows, fetch_org_usage_day_rows};
use super::scope::list_scope;
use crate::server::router::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/metrics/llm-usage", get(llm_usage))
        .route("/metrics/orgs/{org_id}/llm-usage", get(org_llm_usage))
}

// Response shape

#[derive(Serialize, Debug, Default)]
pub struct UsageTotals {
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub run_count: i64,
    /// Runs whose model is in the pricing table (i.e. contribute to `cost_usd`).
    pub priced_run_count: i64,
}

#[derive(Serialize, Debug)]
pub struct DayCost {
    pub day: String,
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub run_count: i64,
}

#[derive(Serialize, Debug)]
pub struct ModelCost {
    pub model: String,
    /// `None` when the model isn't in the pricing table — tokens are still
    /// reported so the UI can flag "unpriced" usage rather than hide it.
    pub cost_usd: Option<f64>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub run_count: i64,
}

#[derive(Serialize, Debug)]
pub struct OrgCost {
    pub org_id: Uuid,
    pub org_name: String,
    pub org_slug: String,
    pub cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub run_count: i64,
}

#[derive(Serialize, Debug)]
pub struct LlmUsageOverview {
    pub window_days: i32,
    pub total: UsageTotals,
    pub by_day: Vec<DayCost>,
    pub by_model: Vec<ModelCost>,
    pub by_org: Vec<OrgCost>,
}

/// Per-org usage detail. Unlike `LlmUsageOverview.by_org` (a cross-tenant
/// leaderboard truncated to the top 10 by cost), this is scoped to a single
/// org server-side, so it's correct for *any* tenant — and carries the daily
/// series for a trend sparkline.
#[derive(Serialize, Debug)]
pub struct OrgUsageDetail {
    pub window_days: i32,
    pub total: UsageTotals,
    pub by_day: Vec<DayCost>,
}

#[derive(Deserialize, Default)]
pub struct UsageQuery {
    pub days: Option<i32>,
}

// Handler

pub async fn llm_usage(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Query(q): Query<UsageQuery>,
) -> Result<Json<LlmUsageOverview>, Response> {
    let days = q.days.unwrap_or(30).clamp(1, 365);
    let db = connect().await?;
    let scope = list_scope(&db, &actor)
        .await
        .map_err(axum::response::IntoResponse::into_response)?;

    let day_rows = fetch_day_model_rows(&db, days, scope.as_deref())
        .await
        .map_err(db_err)?;
    let org_rows = fetch_org_model_rows(&db, days, scope.as_deref())
        .await
        .map_err(db_err)?;

    Ok(Json(build_overview(days, day_rows, org_rows)))
}

async fn org_llm_usage(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Query(q): Query<UsageQuery>,
) -> Result<Json<OrgUsageDetail>, Response> {
    let days = q.days.unwrap_or(30).clamp(1, 365);
    let db = connect().await?;
    // Scope. `PlatformOperate` is held by every Global Admin regardless of bound, so
    // unfenced this reads another tenant's LLM cost and token totals. Milder than the
    // subdomain toggle — a read, and cost rather than content — but the same axis, and
    // it is the fourth `{org_id}` router rather than a special case.
    crate::server::api::admin::scope::deny_out_of_scope(&db, &actor, org_id)
        .await
        .map_err(axum::response::IntoResponse::into_response)?;
    let day_rows = fetch_org_usage_day_rows(&db, days, org_id)
        .await
        .map_err(db_err)?;
    Ok(Json(build_org_detail(days, day_rows)))
}

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;
