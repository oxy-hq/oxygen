//! The cached per-anomaly decomposition. The first call runs airlayer's
//! recursive driver search and writes the result onto the row; every later call
//! returns it, which is why the cache lives on the anomaly rather than in a
//! request-keyed store.
//!
//! Served by any pod. The semantic model comes through `resolve_scan` (the
//! compile boundary, or the working copy on a node that holds one) exactly as
//! the dry run (`preview`) and the metric tree's own explain read it, the
//! anomaly is a Postgres row, and the queries go to the warehouse. It used to
//! ask for the working copy and hand the runner no scan, so the runner read
//! the semantic model off the workspace directory — which only the Factory has.

use agentic_http::AgenticState;
use axum::extract::{Extension, Json, Path, Query};
use chrono::Utc;
use entity::metric_anomalies::{self, Entity as AnomaliesEntity};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_metric_monitoring as monitoring;
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use super::error::AnomalyError;
use crate::agentic_wiring::OxyMetricTreeRunner;
use crate::server::api::metric_tree::resolve_scan;
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, PreaggCacheCtx, WorkspaceManagerReadOnly,
};

#[derive(Debug, Deserialize, Default)]
pub struct ExplainAnomalyQuery {
    /// Force a fresh computation even if the row already has a cached result.
    #[serde(default)]
    pub refresh: bool,
}

/// `POST /workspaces/{workspace_id}/semantic/anomalies/{id}/explain` — run
/// (or return cached) airlayer explain for this anomaly bucket.
///
/// First call computes and writes the JSON result onto the row. Subsequent
/// calls return the cached payload instantly so the Insights drawer
/// survives page refreshes without re-running the 20-30s recursive search.
/// `?refresh=true` busts the cache for the rescan affordance in the UI.
pub async fn explain_anomaly(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    Extension(state): Extension<Arc<AgenticState>>,
    preagg_ctx: PreaggCacheCtx,
    Path((_workspace_id, anomaly_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<ExplainAnomalyQuery>,
) -> Result<Json<serde_json::Value>, AnomalyError> {
    let row = AnomaliesEntity::find_by_id(anomaly_id)
        .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_manager.workspace_id))
        .one(&state.db)
        .await?
        .ok_or(AnomalyError::NotFound)?;

    // Cheap path: hand back the cache.
    if !q.refresh
        && let Some(cached) = &row.explain_cache
    {
        return Ok(Json(cached.clone()));
    }

    // Build the explain request from the anomaly's fields. Current = the
    // anomaly bucket; previous = the same phase one seasonal cycle earlier,
    // derived from the monitor's persisted seasonality (falling back to the
    // granularity default for pre-column rows). Aligning to the seasonal
    // period — not a hardcoded offset — is what keeps a daily/weekly-seasonal
    // anomaly compared against the same weekday rather than the weekend.
    let current = row.period_start.date_naive();
    let periods = agentic_analytics::anomaly_period::resolve_seasonal_period(
        row.seasonal_period,
        &row.granularity,
    );
    let previous =
        agentic_analytics::anomaly_period::shift_date_back(current, &row.granularity, periods);
    let current_str = current.format("%Y-%m-%d").to_string();
    let previous_str = previous.format("%Y-%m-%d").to_string();

    // Scope the explain to the anomaly's segment (e.g. the specific restaurant
    // a `group_by` monitor flagged) so the totals and decomposition reflect
    // that segment, not the chain-wide aggregate.
    let filters = agentic_analytics::anomaly_store::segment_query_filters(&row.filters);

    use agentic_analytics::MetricTreeRunner as _;
    // Explain is the heaviest metric-tree op there is — a recursive driver
    // search that fires tens of queries — so it is the one that gains most from
    // a rollup. Attached the same way the IDE's `/metric-tree/explain` does; a
    // request no rollup covers still falls through to the warehouse.
    // Fresh rollups only, for the same reason the scan requires them: this
    // explains an anomaly the scan already persisted, and a stale rollup would
    // attribute the drop it invented to whichever driver lags with it.
    let renewal_threshold_secs =
        preagg_ctx.renewal_threshold_secs_or(&workspace_manager.config_manager);
    // `source` owns the materialised semantic model; it has to outlive the run.
    let source = resolve_scan(&workspace_manager)
        .await
        .map_err(|_| AnomalyError::Unavailable("the semantic model is not readable yet".into()))?;
    let runner = OxyMetricTreeRunner::new(workspace_manager, user.id, role)
        .with_scan(source.scan.clone())
        .with_preagg(preagg_ctx.cache.clone(), renewal_threshold_secs)
        .requiring_fresh_rollups();
    let mut config = oxy_airlayer_compat::engine::metric_tree_ops::ExplainConfig::default();
    config.deep = false;

    let result = runner
        .run_explain(
            row.measure.clone(),
            row.time_dimension.clone(),
            (current_str.clone(), current_str),
            (previous_str.clone(), previous_str),
            filters,
            config,
        )
        .await
        .map_err(|e| AnomalyError::Scan(monitoring::ScanError::FetchSeries(e)))?;

    let value = serde_json::to_value(&result)
        .map_err(|e| AnomalyError::Internal(format!("serialize explain: {e}")))?;

    // Persist cache. Failures here are non-fatal — the user gets the
    // result; next click will just recompute.
    let mut active = row.into_active_model();
    active.explain_cache = Set(Some(value.clone()));
    active.explain_cached_at = Set(Some(Utc::now().into()));
    if let Err(e) = active.update(&state.db).await {
        tracing::warn!(
            target: "metric_anomalies",
            anomaly_id = %anomaly_id,
            error = %e,
            "failed to persist explain cache"
        );
    }

    Ok(Json(value))
}
