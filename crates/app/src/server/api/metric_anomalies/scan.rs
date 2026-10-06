//! The scan trigger: run every `.monitor.yml` entry once and upsert what it
//! finds. Long-running by nature — the handler waits a bounded time and then
//! reports `pending`, leaving the scan to finish in the background.

use agentic_http::AgenticState;
use agentic_runtime::lifecycle::crud::runs::{insert_run, update_run_done, update_run_failed};
use axum::extract::{Extension, Json, Query};
use chrono::{TimeZone, Utc};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_metric_monitoring as monitoring;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use super::error::AnomalyError;
use crate::agentic_wiring::OxyMetricTreeRunner;
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, PreaggCacheCtx, WorkspaceManagerWorkingCopy,
};

#[derive(Debug, Serialize)]
pub struct ScanResponse {
    pub monitors_scanned: usize,
    pub monitors_failed: usize,
    pub anomalies_persisted: usize,
    /// `true` when the scan was still running at response time. The scan
    /// continues in the background and results will appear in subsequent
    /// list calls. Clients should refetch anomalies after a short delay.
    #[serde(default)]
    pub pending: bool,
    /// One entry per monitor that errored, so the UI can tell the user
    /// *which* monitor failed and *why* rather than just a count. Empty on a
    /// clean scan and (necessarily) on the `pending` path, where the scan is
    /// still running and failures aren't known yet.
    #[serde(default)]
    pub failures: Vec<ScanFailureDetail>,
}

/// A single failed monitor, flattened for the wire. Mirrors the fields the
/// inbox needs to name the monitor + segment and show the error message.
#[derive(Debug, Serialize)]
pub struct ScanFailureDetail {
    pub measure: String,
    pub time_dimension: String,
    pub granularity: String,
    pub label: Option<String>,
    /// Segment key when the failed monitor was a `group_by`/filtered segment
    /// (e.g. `"labor_daily.restaurant_id=loc-abc"`); empty for chain-wide.
    pub dimension_key: String,
    /// Raw filters identifying the segment, mirroring the persisted anomaly's
    /// `filters` so the inbox can render the failure with the same friendly
    /// segment label it uses for anomaly rows. `None` for chain-wide monitors.
    pub filters: Option<serde_json::Value>,
    /// Human-readable error (the `ScanError` `Display` chain).
    pub error: String,
}

fn to_failure_detail(f: &monitoring::service::MonitorFailure) -> ScanFailureDetail {
    let filters = if f.entry.filters.is_empty() {
        None
    } else {
        serde_json::to_value(&f.entry.filters).ok()
    };
    ScanFailureDetail {
        measure: f.entry.measure.clone(),
        time_dimension: f.entry.time_dimension.clone(),
        granularity: f.entry.granularity.airlayer_str().to_string(),
        label: f.entry.label.clone(),
        dimension_key: monitoring::config::MonitorFilter::key_for(&f.entry.filters),
        filters,
        error: f.error.to_string(),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ScanQuery {
    /// Override the reference date the scan treats as "now" (YYYY-MM-DD).
    /// Lets demos / tests target a period when warehouse data actually
    /// exists. Omitted = real `Utc::now()`.
    pub as_of: Option<String>,
}

/// `POST /workspaces/{workspace_id}/semantic/anomalies/scan` — run every
/// `.monitor.yml` entry once and upsert new anomalies into the inbox.
///
/// The scan runs inside a detached `tokio::spawn` task so it isn't cancelled
/// when the global 60-second request timeout fires. If the scan completes
/// within 55 seconds the full result is returned synchronously. If it takes
/// longer, the endpoint returns immediately with `pending: true` and the
/// task continues to completion in the background — the anomalies list will
/// reflect the new results on the next fetch.
pub async fn run_scan(
    WorkspaceManagerWorkingCopy(workspace_manager): WorkspaceManagerWorkingCopy,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    Extension(state): Extension<Arc<AgenticState>>,
    preagg_ctx: PreaggCacheCtx,
    Query(q): Query<ScanQuery>,
) -> Result<Json<ScanResponse>, AnomalyError> {
    let workspace_id = workspace_manager.workspace_id;

    // Debounce: if a scan completed or started within the last 60 s, return
    // immediately with `pending: true` rather than spawning a second parallel
    // scan over the same `.monitor.yml`. Reuses the same global registry that
    // the periodic cron tick uses so the debounce window is shared.
    if !monitoring::global_registry()
        .take_if_due(workspace_id, Duration::from_secs(60))
        .await
    {
        return Ok(Json(ScanResponse {
            monitors_scanned: 0,
            monitors_failed: 0,
            anomalies_persisted: 0,
            pending: true,
            failures: Vec::new(),
        }));
    }

    // Same rollup short-circuit the IDE's metric-tree handlers attach. A scan
    // fires one time-series query per monitor entry per granularity, which is
    // exactly the shape a rollup covers; without this the scheduled path and
    // the manual one answered the same question from different tiers.
    // `requiring_fresh_rollups` because a scan persists what it computes: a
    // rollup whose newest buckets are missing or partial reads as a drop,
    // `persist_scan` writes that to the Insights Inbox, and an unhealthy
    // transition pages Slack. `.monitor.yml`'s `freshness` guards *warehouse*
    // lag and knows nothing about a rollup whose `preagg_cycle` is behind.
    let runner = Arc::new(
        OxyMetricTreeRunner::new(workspace_manager.clone().into_read_only(), user.id, role)
            .with_preagg(
                preagg_ctx.cache.clone(),
                preagg_ctx.renewal_threshold_secs_or(&workspace_manager.config_manager),
            )
            .requiring_fresh_rollups(),
    );
    // Compile-boundary fast path. When the workspace is promoted +
    // flag is on, we materialise `.monitor.yml` into a tempdir from
    // Postgres and pass THAT path to scan_workspace. The tempdir is
    // owned by the spawned task below so it lives for the whole scan.
    let materialised_monitor = match crate::server::api::semantic_scan::materialise_monitor_config(
        &workspace_manager.config_manager,
    )
    .await
    {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = ?e, "run_scan: monitor materialise failed; falling through to FS");
            None
        }
    };
    let workspace_root = workspace_manager.config_manager.workspace_path();
    let fs_config_path = monitoring::default_config_path(workspace_root);
    let config_path = materialised_monitor
        .as_ref()
        .map(|m| m.config_path.clone())
        .unwrap_or(fs_config_path);
    let now = parse_as_of(q.as_of.as_deref())?;
    let backdated = q.as_of.is_some();
    let db = state.db.clone();

    // Create a run row so the scan appears in the coordinator.
    let run_id = Uuid::new_v4().to_string();
    let meta = serde_json::json!({ "trigger": "manual" });
    if let Err(e) = insert_run(
        &db,
        &run_id,
        "Anomaly scan",
        None,
        "monitor_scan",
        Some(meta),
        workspace_id,
    )
    .await
    {
        tracing::warn!(error = %e, "run_scan: failed to create run row; scan will proceed without coordinator tracking");
    }

    // Spawn the scan. The spawned task always finalises the run status so the
    // coordinator shows done/failed whether the 55-second wait fires or not.
    let (tx, rx) =
        tokio::sync::oneshot::channel::<Result<(monitoring::ScanResult, usize), AnomalyError>>();
    let db_bg = db.clone();
    let run_id_bg = run_id.clone();
    // Move the tempdir handle into the spawned task so it lives as
    // long as the scan does. Drop at task end cleans up the file.
    let _materialised_monitor_guard = materialised_monitor;
    tokio::spawn(async move {
        let _hold = _materialised_monitor_guard;
        let outcome = async {
            // Events already on record, so a bucket continuing a reported
            // slide is not made to re-clear its seasonal envelope alone.
            let open_events = monitoring::load_open_events(&db_bg, workspace_id)
                .await
                .map_err(AnomalyError::Db)?;
            let result = monitoring::scan_workspace(runner, &config_path, now, None, &open_events)
                .await
                .map_err(AnomalyError::Scan)?;
            let persisted = monitoring::persist_scan(&db_bg, workspace_id, &result)
                .await
                .map_err(AnomalyError::Db)?;
            // A scan pinned to a past date is a demo or a backfill; it says
            // nothing new about today, so it announces nothing.
            if !backdated {
                crate::server::anomaly_notify::enqueue_if_due(
                    &db_bg,
                    workspace_id,
                    result.notify.as_ref(),
                )
                .await;
            }
            Ok::<(monitoring::ScanResult, usize), AnomalyError>((result, persisted))
        }
        .await;

        let summary = match &outcome {
            Ok((result, persisted)) => format!(
                "scanned={} failed={} persisted={}",
                result.outcomes.len(),
                result.failures.len(),
                persisted
            ),
            Err(e) => format!("{e:?}"),
        };
        let has_failures = matches!(&outcome, Ok((r, _)) if !r.failures.is_empty());
        if outcome.is_err() || has_failures {
            let _ = update_run_failed(&db_bg, &run_id_bg, &summary).await;
        } else {
            let _ = update_run_done(&db_bg, &run_id_bg, &summary, None).await;
        }
        let _ = tx.send(outcome);
    });

    // Wait up to 55 s — safely inside the 60 s global timeout. If the scan
    // finishes in time, return the full tally. If it's still running, return
    // `pending: true`; the spawned task continues and updates the run row.
    match tokio::time::timeout(Duration::from_secs(55), rx).await {
        Ok(Ok(Ok((result, persisted)))) => Ok(Json(ScanResponse {
            monitors_scanned: result.outcomes.len(),
            monitors_failed: result.failures.len(),
            anomalies_persisted: persisted,
            pending: false,
            failures: result.failures.iter().map(to_failure_detail).collect(),
        })),
        Ok(Ok(Err(e))) => Err(e),
        Ok(Err(_)) => Err(AnomalyError::Internal("scan task dropped".into())),
        Err(_elapsed) => {
            tracing::info!(
                target: "metric_anomalies",
                workspace_id = %workspace_id,
                run_id = %run_id,
                "scan still running after 55 s — returning pending response"
            );
            // Run row stays "running"; the background task updates it on finish.
            Ok(Json(ScanResponse {
                monitors_scanned: 0,
                monitors_failed: 0,
                anomalies_persisted: 0,
                pending: true,
                failures: Vec::new(),
            }))
        }
    }
}

/// Parse the optional `as_of` query string. Accepts `YYYY-MM-DD`. Returns
/// real `Utc::now()` when absent so the cron tick path stays unchanged.
fn parse_as_of(input: Option<&str>) -> Result<chrono::DateTime<Utc>, AnomalyError> {
    let Some(raw) = input else {
        return Ok(Utc::now());
    };
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map(|d| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap()))
        .map_err(|e| AnomalyError::BadRequest(format!("invalid as_of '{raw}': {e}")))
}
