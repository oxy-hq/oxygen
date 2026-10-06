//! Internal cross-tenant workspace health rollup.
//!
//! `evaluator` is pure (signal counts -> status); `queries` gathers signals
//! from shared Postgres; `alert` decides what is worth telling Slack — status
//! transitions, plus a recurring reminder for as long as a workspace stays
//! unhealthy — and pushes it. `history` keeps each status change, which the
//! state row overwrites.
pub(crate) mod alert;
pub(crate) mod app_availability;
pub(crate) mod eval_pass;
pub(crate) mod evaluator;
pub mod history;
pub(crate) mod queries;
pub(crate) mod reconcile;
pub(crate) mod smoke;

use axum::{
    Json, Router,
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::{Deserialize, Serialize};

use crate::server::router::AppState;
use evaluator::WorkspaceSignals;

use super::internal_jobs::{connect, db_err, error_body};
use super::scope;

/// Raw per-workspace signal counts, surfaced so the per-workspace Health tab
/// can show the underlying numbers behind each dimension's reason string.
#[derive(Serialize)]
pub(crate) struct SignalsRow {
    failed_runs: i64,
    timed_out_runs: i64,
    total_runs: i64,
    airway_last_run_failed: bool,
    airway_completed_with_errors: bool,
    open_high_anomalies: i64,
    open_medium_anomalies: i64,
    dead_letter_count: i64,
}

impl From<&WorkspaceSignals> for SignalsRow {
    fn from(s: &WorkspaceSignals) -> Self {
        Self {
            failed_runs: s.failed_runs,
            timed_out_runs: s.timed_out_runs,
            total_runs: s.total_runs,
            airway_last_run_failed: s.airway_last_run_failed,
            airway_completed_with_errors: s.airway_completed_with_errors,
            open_high_anomalies: s.open_high_anomalies,
            open_medium_anomalies: s.open_medium_anomalies,
            dead_letter_count: s.dead_letter_count,
        }
    }
}

#[derive(Serialize)]
pub struct WorkspaceHealthRow {
    workspace_id: uuid::Uuid,
    /// Workspace display name, resolved from the `workspaces` row. `None` only
    /// when the workspace was deleted between the signal scan and this read.
    workspace_name: Option<String>,
    /// Owning org name, `None` when the workspace has no `org_id` (or was
    /// deleted). Shown alongside the name so operators don't read bare UUIDs.
    org_name: Option<String>,
    status: String,
    reasons: Vec<String>,
    /// Per-dimension breakdown (job liveness / pipeline / queue /
    /// reconciliation), each with its own status. Served verbatim from the
    /// stored payload as opaque JSON — the read path does no live evaluation.
    dimensions: serde_json::Value,
    signals: serde_json::Value,
    /// Per-check reconciliation drift detail from the stored payload.
    reconciliation: serde_json::Value,
    /// Per-probe smoke-test detail from the stored payload. These verdicts can
    /// be older than `checked_at` — the smoke test runs on its own slower
    /// cadence, and `last_smoke_at` says when they were actually produced.
    smoke: serde_json::Value,
    /// Which smoke probe kinds are enabled (`[{ kind, enabled }]`), so the UI
    /// can render a disabled probe as "not enabled" instead of omitting it.
    /// Empty when the workspace's smoke test is disabled or the row predates it.
    smoke_probes: serde_json::Value,
    /// When the smoke probes last ran. `None` when no smoke test is configured.
    last_smoke_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// When the status last transitioned, from the persisted eval-pass state
    /// (`workspace_health_state.changed_at`). `None` when no eval pass has
    /// recorded this workspace yet — the rollup is computed live, but the
    /// transition time is only known from the periodic sweep.
    changed_at: Option<chrono::DateTime<chrono::FixedOffset>>,
    /// When the periodic sweep last evaluated this workspace
    /// (`workspace_health_state.updated_at`), refreshed on every pass even when
    /// the status is unchanged. `None` until the first sweep records it. Drives
    /// the "last checked" display so operators can tell stale rollups from live
    /// ones (the sweep runs every 10 minutes).
    checked_at: Option<chrono::DateTime<chrono::FixedOffset>>,
}

#[derive(Serialize)]
pub struct WorkspaceHealthResponse {
    pub workspaces: Vec<WorkspaceHealthRow>,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/workspace-health", get(list_workspace_health))
        .route(
            "/workspace-health/{workspace_id}/eval",
            post(trigger_workspace_health_eval),
        )
        .route(
            "/workspace-health/{workspace_id}/history",
            get(history::workspace_health_history),
        )
}

/// Cross-tenant health rollup, worst-first. Pure Postgres read (FleetOk): the
/// rollup is computed by the periodic sweep and persisted; this endpoint returns
/// the stored payload verbatim, joined with fresh display labels. No live signal
/// gathering, no external calls.
///
/// "Cross-tenant" is what an unbounded grant sees. A bounded grant gets the
/// rollup of the workspaces in the orgs it names — the same `Some(ids)` narrowing
/// the partner-scoped view already uses — and a workspace with no org is
/// platform-level, so it is not among them.
pub async fn list_workspace_health(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<WorkspaceHealthResponse>, Response> {
    let db = connect().await?;
    let reach = scope::list_scope(&db, &actor)
        .await
        .map_err(IntoResponse::into_response)?;
    let in_reach = match reach {
        None => None,
        Some(orgs) => Some(workspaces_of(&db, orgs).await.map_err(db_err)?),
    };
    let workspaces = health_rollup(&db, in_reach.as_deref())
        .await
        .map_err(db_err)?;
    Ok(Json(WorkspaceHealthResponse { workspaces }))
}

/// The workspaces belonging to `orgs` — what a bounded grant's rollup is narrowed to.
async fn workspaces_of(
    db: &sea_orm::DatabaseConnection,
    orgs: Vec<uuid::Uuid>,
) -> Result<Vec<uuid::Uuid>, sea_orm::DbErr> {
    entity::workspaces::Entity::find()
        .select_only()
        .column(entity::workspaces::Column::Id)
        .filter(entity::workspaces::Column::OrgId.is_in(orgs))
        .into_tuple()
        .all(db)
        .await
}

/// The rollup rows, worst-first, joined with display labels. `workspace_ids =
/// None` returns every workspace (the staff cross-tenant view); `Some(ids)` scopes
/// to that set (the partner-scoped view over its managed clients' workspaces).
/// Reads the persisted sweep state only — no live evaluation.
pub async fn health_rollup(
    db: &sea_orm::DatabaseConnection,
    workspace_ids: Option<&[uuid::Uuid]>,
) -> Result<Vec<WorkspaceHealthRow>, sea_orm::DbErr> {
    if matches!(workspace_ids, Some(ids) if ids.is_empty()) {
        return Ok(Vec::new());
    }
    let labels = queries::gather_workspace_labels(db).await?;

    let mut find = entity::workspace_health_state::Entity::find();
    if let Some(ids) = workspace_ids {
        find = find.filter(entity::workspace_health_state::Column::WorkspaceId.is_in(ids.to_vec()));
    }
    let rows = find.all(db).await?;

    let mut workspaces: Vec<WorkspaceHealthRow> = rows
        .into_iter()
        .map(|r| {
            let label = labels.get(&r.workspace_id);
            row_from_state(r, label)
        })
        .collect();
    // Worst-first: Unhealthy > Degraded > Healthy.
    workspaces.sort_by_key(|w| std::cmp::Reverse(status_rank(&w.status)));
    Ok(workspaces)
}

/// Response to an on-demand eval trigger: the `run_id` of the enqueued eval so
/// the client can correlate/poll. The refreshed row is fetched separately once
/// `checked_at` advances.
#[derive(Serialize)]
pub struct TriggerEvalResponse {
    pub run_id: String,
}

/// `?smoke=true` runs the workspace's smoke probes on this pass even if their
/// (default 6h) cadence has not elapsed — the Health tab's "Run smoke test"
/// button. Absent → false, the plain "Re-evaluate" trigger, which re-reads the
/// passive Postgres signals and reuses the last smoke verdicts.
///
/// It forces the *clock*, not the *config*: a workspace with
/// `smoke_test: { enabled: false }` runs no probes either way, so the button
/// can't bill an opted-out workspace for warehouse queries and agent tokens.
#[derive(Deserialize, Default)]
pub struct TriggerEvalParams {
    #[serde(default)]
    pub smoke: bool,
}

/// Enqueue an on-demand health eval for a single workspace and return its
/// `run_id` (HTTP 202). The eval (Postgres signals + reconciliation + Slack on a
/// transition, plus the smoke probes when `?smoke=true`) is a `TaskScope::Global`
/// `health_eval_workspace` task drained by the worker fleet — not run inline — so
/// a slow Toast reconciliation or a cold warehouse can't tie up the request and
/// the run survives an instance restart. The client polls the workspace-health
/// read until `checked_at` advances past the trigger.
///
/// `FleetOk` in `role_manifest.rs`: this handler only enqueues (a plain Postgres
/// insert), so it serves on any replica. The workspace-context build +
/// working-copy `reconcile.yml` fallthrough happen in the fleet executor that
/// drains the task — which lands on an FS-owning node on its own — so route
/// classification does not need to pin the request to the ide. The query param
/// does not change that: it rides in the task payload, not the request path.
///
/// Fenced on the workspace's org: a bounded grant cannot spend another tenant's
/// warehouse queries and agent tokens on a smoke run, and a workspace outside the
/// grant answers the same 404 a missing one does.
pub async fn trigger_workspace_health_eval(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(workspace_id): Path<uuid::Uuid>,
    Query(params): Query<TriggerEvalParams>,
) -> Result<Response, Response> {
    let db = connect().await?;
    scope::deny_out_of_scope_for_workspace(&db, &actor, workspace_id)
        .await
        .map_err(|status| match status {
            StatusCode::NOT_FOUND => error_body(status, "workspace_not_found", None),
            other => error_body(other, "scope_unreadable", None),
        })?;
    let run_id = agentic_pipeline::scheduler::enqueue_health_eval(&db, workspace_id, params.smoke)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e).into_response())?;
    Ok((StatusCode::ACCEPTED, Json(TriggerEvalResponse { run_id })).into_response())
}

/// The status/reasons/dimensions/signals/reconciliation parts of a row, taken
/// from the stored payload when present, else from the state row's own
/// `status`/`reasons` columns (a transient state right after the payload
/// migration backfills NULL, before the next sweep writes it).
struct PayloadParts {
    status: String,
    reasons: Vec<String>,
    dimensions: serde_json::Value,
    signals: serde_json::Value,
    reconciliation: serde_json::Value,
    smoke: serde_json::Value,
    smoke_probes: serde_json::Value,
}

fn payload_parts(
    payload: Option<serde_json::Value>,
    status_col: String,
    reasons_col: serde_json::Value,
) -> PayloadParts {
    let empty_arr = || serde_json::Value::Array(vec![]);
    match payload {
        Some(p) => PayloadParts {
            status: p
                .get("status")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or(status_col),
            reasons: p
                .get("reasons")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default(),
            dimensions: p.get("dimensions").cloned().unwrap_or_else(empty_arr),
            signals: p.get("signals").cloned().unwrap_or(serde_json::Value::Null),
            reconciliation: p.get("reconciliation").cloned().unwrap_or_else(empty_arr),
            smoke: p.get("smoke").cloned().unwrap_or_else(empty_arr),
            smoke_probes: p.get("smoke_probes").cloned().unwrap_or_else(empty_arr),
        },
        None => PayloadParts {
            status: status_col,
            reasons: serde_json::from_value(reasons_col).unwrap_or_default(),
            dimensions: empty_arr(),
            signals: serde_json::Value::Null,
            reconciliation: empty_arr(),
            smoke: empty_arr(),
            smoke_probes: empty_arr(),
        },
    }
}

/// Build a response row from a persisted state row + its display label.
fn row_from_state(
    r: entity::workspace_health_state::Model,
    label: Option<&queries::WorkspaceLabel>,
) -> WorkspaceHealthRow {
    let changed_at = r.changed_at;
    let checked_at = r.updated_at;
    let last_smoke_at = r.last_smoke_at;
    let workspace_id = r.workspace_id;
    let parts = payload_parts(r.payload, r.status, r.reasons);
    WorkspaceHealthRow {
        workspace_id,
        workspace_name: label.map(|l| l.name.clone()),
        org_name: label.and_then(|l| l.org_name.clone()),
        status: parts.status,
        reasons: parts.reasons,
        dimensions: parts.dimensions,
        signals: parts.signals,
        reconciliation: parts.reconciliation,
        smoke: parts.smoke,
        smoke_probes: parts.smoke_probes,
        last_smoke_at,
        changed_at: Some(changed_at),
        checked_at: Some(checked_at),
    }
}

fn status_rank(s: &str) -> u8 {
    match s {
        "unhealthy" => 2,
        "degraded" => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_payload_falls_back_to_columns() {
        // Right after the migration backfills NULL, the row's own status/reasons
        // columns still carry real data — use them, with empty detail.
        let parts = payload_parts(
            None,
            "degraded".to_string(),
            serde_json::json!(["x failed"]),
        );
        assert_eq!(parts.status, "degraded");
        assert_eq!(parts.reasons, vec!["x failed".to_string()]);
        assert!(parts.dimensions.as_array().unwrap().is_empty());
        assert!(parts.signals.is_null());
        assert!(parts.reconciliation.as_array().unwrap().is_empty());
        assert!(parts.smoke.as_array().unwrap().is_empty());
        assert!(parts.smoke_probes.as_array().unwrap().is_empty());
    }

    #[test]
    fn payload_without_smoke_keys_reads_as_no_checks() {
        // Rows written before the smoke dimension existed have neither `smoke`
        // nor `smoke_probes`; they must read as "no smoke data", not fail to
        // deserialize.
        let payload = serde_json::json!({
            "status": "healthy",
            "reasons": [],
            "dimensions": [],
            "signals": null,
            "reconciliation": []
        });
        let parts = payload_parts(Some(payload), "healthy".to_string(), serde_json::json!([]));
        assert!(parts.smoke.as_array().unwrap().is_empty());
        assert!(parts.smoke_probes.as_array().unwrap().is_empty());
    }

    #[test]
    fn stored_payload_is_returned_verbatim() {
        let payload = serde_json::json!({
            "workspace_id": uuid::Uuid::nil(),
            "status": "degraded",
            "reasons": ["x drifts 3.0% from source"],
            "dimensions": [],
            "signals": null,
            "reconciliation": [{ "check": "x", "status": "degraded" }],
            "smoke": [{ "check": "connection:bq", "status": "healthy" }],
            "smoke_probes": [{ "kind": "connection", "enabled": true }]
        });
        let parts = payload_parts(Some(payload), "healthy".to_string(), serde_json::json!([]));
        // Payload status wins over the column.
        assert_eq!(parts.status, "degraded");
        assert_eq!(parts.reasons[0], "x drifts 3.0% from source");
        assert_eq!(parts.reconciliation.as_array().unwrap().len(), 1);
        assert_eq!(parts.smoke.as_array().unwrap().len(), 1);
        assert_eq!(parts.smoke_probes.as_array().unwrap().len(), 1);
    }

    /// Anomalies no longer vote on the verdict (see `evaluator::WorkspaceSignals`),
    /// but the counts must still reach the Health tab — "visible, non-voting" is
    /// the whole point, and dropping them from the DTO would silently make the
    /// panel's anomaly rows read zero.
    #[test]
    fn anomaly_counts_still_reach_the_dto() {
        let mut s = WorkspaceSignals::empty(uuid::Uuid::nil());
        s.open_high_anomalies = 4;
        s.open_medium_anomalies = 2;
        let row = SignalsRow::from(&s);
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["open_high_anomalies"], 4);
        assert_eq!(json["open_medium_anomalies"], 2);
    }
}
