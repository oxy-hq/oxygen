//! Cron schedule CRUD + run-now (multi-tenant, §12 FU4b).
//!
//! Lives in the `app` crate (not `agentic-http`) so the handlers can use
//! `WorkspaceAdmin` from `role_guards` — the agentic-http layer must not
//! depend on `app` (backend boundary rule). Routes are mounted under
//! `/{workspace_id}/agentic-schedules` in `workspace.rs`, behind the
//! standard `workspace_middleware`. All operations are scoped to
//! `workspace_id` (extracted from the parent path); mutations require
//! workspace-admin role.
//!
//! **A custom app's non-production function schedules are not the tenant's.**
//! A `function` row whose ref names an environment (`<app_id>/<env>/<fn>`,
//! `agentic_pipeline::function_schedule`) belongs to staff's switch on that
//! environment (environments design §4.5): `list` leaves it out, a read,
//! update, delete, run-now or backfill by its id answers `404` as an id in
//! another workspace does, and create and update refuse a `function` ref
//! naming an environment.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Extension, Path},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use uuid::Uuid;

use agentic_http::AgenticState;
use agentic_pipeline::WorkflowWorkspaceContext;
use agentic_pipeline::function_schedule::FunctionScheduleRef;
use agentic_pipeline::platform::PlatformContext;
use agentic_pipeline::scheduler::{
    BackfillRequest, BackfillResult, ScheduleError, ScheduleInput, backfill_schedule,
    create_schedule, delete_schedule, get_schedule, list_schedules, record_fire_success,
    run_schedule_now, update_schedule,
};
use agentic_runtime::entity::schedule;
use agentic_runtime::lifecycle::crud::runs::{
    insert_run_with_schedule, update_run_done, update_run_failed,
};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use tokio::sync::{mpsc, watch};

use crate::server::api::middlewares::role_guards::WorkspaceAdmin;

fn map_err(e: ScheduleError) -> Response {
    match e {
        ScheduleError::Invalid(m) => (StatusCode::BAD_REQUEST, m).into_response(),
        ScheduleError::NotFound => (StatusCode::NOT_FOUND, "schedule not found").into_response(),
        ScheduleError::Db(e) => {
            tracing::error!(target: "scheduler", error = %e, "schedule CRUD db error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// Whether a `function` schedule's ref names a non-production environment of
/// a custom app. A ref that names none — production's, or any other kind's —
/// is the tenant's as before.
fn names_environment(target_kind: &str, target_ref: &str) -> bool {
    target_kind == "function"
        && FunctionScheduleRef::parse(target_ref).is_some_and(|t| t.environment.is_some())
}

fn is_environment_row(row: &schedule::Model) -> bool {
    names_environment(&row.target_kind, &row.target_ref)
}

/// The tenant's schedule `id`: an environment's row is `NotFound`, the answer
/// an id in another workspace gets.
async fn tenant_schedule(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    id: &str,
) -> Result<schedule::Model, ScheduleError> {
    let row = get_schedule(db, workspace_id, id).await?;
    if is_environment_row(&row) {
        return Err(ScheduleError::NotFound);
    }
    Ok(row)
}

/// Refuse an input whose `function` ref names an environment.
fn tenant_input(input: &ScheduleInput) -> Result<(), ScheduleError> {
    if names_environment(&input.target_kind, &input.target_ref) {
        return Err(ScheduleError::Invalid(
            "a function schedule's target_ref is '<app_id>/<function_name>': a custom app's \
             schedules outside production are turned on per environment by Oxy staff"
                .into(),
        ));
    }
    Ok(())
}

pub async fn list(
    Extension(state): Extension<Arc<AgenticState>>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
) -> Response {
    match list_schedules(&state.db, workspace_id).await {
        Ok(rows) => {
            let rows: Vec<schedule::Model> = rows
                .into_iter()
                .filter(|row| !is_environment_row(row))
                .collect();
            Json(rows).into_response()
        }
        Err(e) => map_err(e),
    }
}

pub async fn get(
    Extension(state): Extension<Arc<AgenticState>>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Path((workspace_id, id)): Path<(Uuid, String)>,
) -> Response {
    match tenant_schedule(&state.db, workspace_id, &id).await {
        Ok(m) => Json(m).into_response(),
        Err(e) => map_err(e),
    }
}

pub async fn create(
    _: WorkspaceAdmin,
    Extension(state): Extension<Arc<AgenticState>>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
    Json(input): Json<ScheduleInput>,
) -> Response {
    if let Err(e) = tenant_input(&input) {
        return map_err(e);
    }
    match create_schedule(&state.db, workspace_id, input).await {
        Ok(m) => (StatusCode::CREATED, Json(m)).into_response(),
        Err(e) => map_err(e),
    }
}

pub async fn update(
    _: WorkspaceAdmin,
    Extension(state): Extension<Arc<AgenticState>>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Path((workspace_id, id)): Path<(Uuid, String)>,
    Json(input): Json<ScheduleInput>,
) -> Response {
    let guarded = match tenant_schedule(&state.db, workspace_id, &id).await {
        Ok(_) => tenant_input(&input),
        Err(e) => Err(e),
    };
    if let Err(e) = guarded {
        return map_err(e);
    }
    match update_schedule(&state.db, workspace_id, &id, input).await {
        Ok(m) => Json(m).into_response(),
        Err(e) => map_err(e),
    }
}

pub async fn delete(
    _: WorkspaceAdmin,
    Extension(state): Extension<Arc<AgenticState>>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Path((workspace_id, id)): Path<(Uuid, String)>,
) -> Response {
    if let Err(e) = tenant_schedule(&state.db, workspace_id, &id).await {
        return map_err(e);
    }
    match delete_schedule(&state.db, workspace_id, &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}

#[derive(Serialize)]
pub struct RunNowResponse {
    pub run_id: String,
}

pub async fn run_now(
    _: WorkspaceAdmin,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Path((workspace_id, id)): Path<(Uuid, String)>,
) -> Response {
    // Fetch once to inspect target_kind before routing.
    let schedule = match tenant_schedule(&state.db, workspace_id, &id).await {
        Ok(s) => s,
        Err(e) => return map_err(e),
    };

    if schedule.target_kind == "monitor_scan" {
        let granularity = match schedule
            .variables
            .as_ref()
            .and_then(|v| v.get("granularity"))
            .and_then(|g| g.as_str())
            .map(ToString::to_string)
        {
            Some(g) => g,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    "missing granularity in schedule variables",
                )
                    .into_response();
            }
        };

        let run_id = Uuid::new_v4().to_string();
        let mut meta = serde_json::json!({ "granularity": granularity });
        agentic_pipeline::scheduler::stamp_trigger_metadata(
            &mut meta,
            &Some("manual".into()),
            &None,
            &None,
        );
        if let Err(e) = insert_run_with_schedule(
            &state.db,
            &run_id,
            &format!("Anomaly scan ({granularity})"),
            None,
            "monitor_scan",
            Some(meta),
            &schedule.id,
            workspace_id,
        )
        .await
        {
            tracing::error!(error = %e, "run_now: failed to create monitor scan run row");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
        record_fire_success(&state.db, &schedule.id, &run_id).await;

        // Register before spawning so SSE, cancel, and graceful shutdown all
        // see this run. Monitor scans don't use HITL answers, but we still
        // create the channel to satisfy the register signature.
        let (answer_tx, _answer_rx) = mpsc::channel::<String>(1);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        state.register(&run_id, answer_tx, cancel_tx);

        let db = state.db.clone();
        let state_bg = state.clone();
        let run_id_bg = run_id.clone();
        let schedule_id_bg = schedule.id.clone();
        let platform_bg = platform.clone();
        tokio::spawn(async move {
            // Honour a cancel that arrived before the scan started.
            if *cancel_rx.borrow() {
                if let Err(e) = update_run_failed(&db, &run_id_bg, "cancelled by user").await {
                    tracing::error!(error = %e, run_id = %run_id_bg, "failed to write cancel to DB");
                }
                state_bg.notify(&run_id_bg);
                state_bg.deregister(&run_id_bg);
                return;
            }

            let Some(port) = platform_bg.as_monitor_scan_port() else {
                if let Err(e) =
                    update_run_failed(&db, &run_id_bg, "monitor scan not available").await
                {
                    tracing::error!(error = %e, run_id = %run_id_bg, "failed to write unavailable to DB");
                }
                state_bg.notify(&run_id_bg);
                state_bg.deregister(&run_id_bg);
                return;
            };

            match port.run_monitor_scan(&db, workspace_id, &granularity).await {
                Ok(summary) => {
                    if let Err(e) = update_run_done(&db, &run_id_bg, &summary, None).await {
                        tracing::error!(error = %e, run_id = %run_id_bg, "failed to mark scan run done");
                    }
                }
                Err(e) => {
                    if let Err(db_err) = update_run_failed(&db, &run_id_bg, &e).await {
                        tracing::error!(error = %db_err, run_id = %run_id_bg, "failed to mark scan run failed");
                    }
                    agentic_pipeline::scheduler::set_schedule_last_error(
                        &db,
                        &schedule_id_bg,
                        Some(&e),
                    )
                    .await;
                }
            }

            // Wake the SSE subscriber, then remove from the active-run maps.
            // Order matters: notify must come before deregister so the Notify
            // permit is delivered while the Arc is still alive in notifiers.
            state_bg.notify(&run_id_bg);
            state_bg.deregister(&run_id_bg);
        });

        Json(RunNowResponse { run_id }).into_response()
    } else if schedule.target_kind == "health_eval" {
        // Internal per-workspace health eval. `fire_schedule` can't handle this
        // kind (it only seeds workflow/airway/agent runs). A manual fire enqueues
        // the same `health_eval_workspace` task the scheduled fire does, so the
        // eval runs on the worker fleet and survives this instance; the runtime
        // marks the run done or failed. `target_ref` is the workspace id.
        match agentic_pipeline::scheduler::enqueue_health_eval_run_now(
            &state.db,
            workspace_id,
            &schedule.id,
        )
        .await
        {
            Ok(run_id) => {
                record_fire_success(&state.db, &schedule.id, &run_id).await;
                Json(RunNowResponse { run_id }).into_response()
            }
            Err(e) => {
                tracing::error!(error = %e, "run_now: failed to enqueue health eval");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
            }
        }
    } else {
        let workspace: Arc<dyn WorkflowWorkspaceContext> = platform.clone();
        match run_schedule_now(&state.db, workspace_id, workspace.as_ref(), &id).await {
            Ok(run_id) => Json(RunNowResponse { run_id }).into_response(),
            Err(e) => map_err(e),
        }
    }
}

#[derive(Serialize)]
pub struct BackfillResponse {
    pub run_ids: Vec<String>,
    pub planned: usize,
}

impl From<BackfillResult> for BackfillResponse {
    fn from(r: BackfillResult) -> Self {
        Self {
            run_ids: r.run_ids,
            planned: r.planned,
        }
    }
}

/// Seed one run per cron occurrence in the requested window, tagged as
/// backfill. Workspace-admin only — like `run_now`, this fires runs
/// out-of-band of the normal cadence.
pub async fn backfill(
    _: WorkspaceAdmin,
    Extension(state): Extension<Arc<AgenticState>>,
    Extension(platform): Extension<Arc<dyn PlatformContext>>,
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
    Path((workspace_id, id)): Path<(Uuid, String)>,
    Json(body): Json<BackfillRequest>,
) -> Response {
    if let Err(e) = tenant_schedule(&state.db, workspace_id, &id).await {
        return map_err(e);
    }
    let workspace: Arc<dyn WorkflowWorkspaceContext> = platform.clone();
    match backfill_schedule(&state.db, workspace_id, workspace.as_ref(), &id, body).await {
        Ok(result) => Json(BackfillResponse::from(result)).into_response(),
        Err(e) => map_err(e),
    }
}

#[cfg(test)]
#[path = "schedules_tests.rs"]
mod tests;
