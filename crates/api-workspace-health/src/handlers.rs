//! The staff console's workspace-health routes, under `/api/admin`: the
//! cross-tenant rollup, the on-demand eval trigger, and a workspace's history of
//! status changes. Each is fenced on the caller's grant (`admin_scope`): a
//! bounded grant sees and evaluates only its own orgs' workspaces, and one
//! outside it answers the 404 a missing workspace does.

use axum::{
    Json,
    extract::{Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use oxy_app::surface::admin_db::{connect, db_err, error_body};
use oxy_app::surface::admin_scope as scope;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_workspace_health::history::{HealthHistory, history_of, window_days};
use oxy_workspace_health::{WorkspaceHealthResponse, health_rollup};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Debug, Deserialize)]
pub struct HistoryParams {
    days: Option<i64>,
}

/// `GET /admin/workspace-health/{workspace_id}/history?days=` — a workspace's
/// status changes, newest first. Postgres only.
///
/// Fenced on the workspace's org like the eval trigger beside it: a bounded
/// grant reads the history of its own orgs' workspaces, and one outside the
/// grant answers the 404 a missing workspace does.
pub async fn workspace_health_history(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<HealthHistory>, Response> {
    let db = connect().await?;
    scope::deny_out_of_scope_for_workspace(&db, &actor, workspace_id)
        .await
        .map_err(|status| match status {
            StatusCode::NOT_FOUND => error_body(status, "workspace_not_found", None),
            other => error_body(other, "scope_unreadable", None),
        })?;
    let history = history_of(&db, workspace_id, window_days(params.days), Utc::now())
        .await
        .map_err(db_err)?;
    Ok(Json(history))
}
