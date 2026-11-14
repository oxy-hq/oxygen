//! `POST /api/projects/{project_id}/procedures/{procedure_id}/runs`     (start)
//! `GET  /api/projects/{project_id}/procedures/runs/{run_id}`            (poll)
//! `POST /api/projects/{project_id}/procedures/runs/{run_id}/cancel`     (cancel)
//!
//! Long-running batch surface for custom-app bundles. An automation
//! is a `.automation.yml` (or back-compat `.procedure.yml`) file in
//! the project — multi-step orchestration (SQL, agent calls, file
//! writes, etc.). Bundles use this to expose "Generate report" /
//! "Recompute" buttons that produce structured artifacts.
//!
//! Pipeline: reuses
//! `agentic_pipeline::automation_run::run_inline_automation_with_render_context`
//! (the same path CLI `oxy run` and the MCP automation tool use). The
//! handler does not run it: the start endpoint registers the run and
//! enqueues one `TaskScope::Global` task ([`task`]), and a driver process
//! — a worker-fleet pod, or this one under `OXY_ROLE=all` — claims and
//! executes it ([`executor`]). A deploy or a restart of the pod that took
//! the request no longer takes the run with it. A driver that dies mid-run
//! has its claim requeued, and the attempt that next claims it finds the
//! run already begun and closes it as `failed` /
//! `automation_run_interrupted` rather than running its steps a second
//! time — at most once per run; the user starts it again.
//! State the bundle reads lives in the `customer_app_procedure_runs` DB
//! table (see `migration::m20260526_000001_create_customer_app_procedure_runs`).
//!
//! Cancellation: the cancel endpoint closes the row as `cancelled` (so
//! pollers see it at once, from any replica) and writes the durable
//! cancel flag on the run, which the driver polls and turns into a stop.
//! The first terminal state wins ([`settle`]): a result that lands after
//! the cancel is discarded.
//!
//! Non-production ([`staging_hold`]): the environment guard lets this route's
//! `POST` through on a custom app's staging host or sandbox whose
//! `workspace-actions` plane is `held`
//! (`custom_app_env_request::is_held_workspace_action`), same as it lets an
//! agent ask through — but unlike an ask, an automation run never runs held.
//! `start_automation_run` refuses it outright with `409 held_in_staging`,
//! logged as one `app.staging.held` row for a caller who may open this app's
//! environment; a caller who may not, or an environment whose plane is
//! `refused`, gets the same `404 EnvironmentRefused` a staging ask refuses
//! with. Production is unaffected.
//!
//! `preview` ([`preview_mode`]), which the guard lets through as it does
//! `held`, runs the automation as a workspace-preview dry run instead.

pub mod executor;
pub mod preview_mode;
pub mod task;

/// The run row's state machine — the first terminal state wins, the first
/// attempt to begin wins — and the failure classifier. `oxy-app`'s, because
/// the workspace-preview sweep settles a preview-mode run's row in the same
/// tick it finishes the preview run.
pub use oxy_app::surface::automation_runs::{automation_error_to_code, settle};

use std::collections::HashMap;

use agentic_automation::AutomationConfig;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use entity::customer_app_procedure_runs as proc_run;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tracing::{error, instrument, warn};
use uuid::Uuid;

use oxy::config::ConfigManager;
use oxy_app_core::custom_app_env_request::request_environment;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;

use crate::agent_ask_staging::{non_production_app, refused, workspace_actions_mode};
use crate::custom_apps_gates::check_custom_app_gates;
use oxy_app::surface::custom_app_semantic::{CustomAppContextExt, parse_versioned_body};
use oxy_app_core::AppState;
use oxy_custom_apps::planes::WorkspaceActionsMode;
use sea_orm::ExprTrait;

/// The surface an automation run's held row is listed under, and its `mode`.
const AUTOMATION_SURFACE: &str = "automation";

/// `409 held_in_staging`: a custom app's staging may not start an automation
/// run at all (unlike an ask, nothing runs held). The body names the surface
/// so a bundle can tell this apart from every other refusal.
fn held_in_staging() -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({
            "error": "held_in_staging",
            "surface": AUTOMATION_SURFACE,
            "what": "starting an automation run from an app's staging",
        })),
    )
        .into_response()
}

/// What a non-production host's automation-run request is to do, decided
/// before anything is looked up or written.
enum Hold {
    /// Production: start the run as before.
    Proceed,
    /// Answered here: a refusal, or the `409 held_in_staging`.
    Answer(Response),
    /// `workspace-actions: preview`: run it as a preview dry run.
    Preview(Box<preview_mode::PreviewTarget>),
}

/// [`Hold`] for a staging or sandbox host's automation-run request. The
/// environment's `workspace-actions` plane decides, not its kind: `held`
/// records one `app.staging.held` row and answers `409 held_in_staging`,
/// `refused` answers the `404 EnvironmentRefused` an ask refuses with,
/// `preview` hands back the target to run. A caller who may not open the
/// environment gets that `404` too, and an unreadable plane `409`. Fail
/// closed: a lookup miss is a refusal, never a run.
async fn staging_hold(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    user: &AuthenticatedUser,
    project_id: Uuid,
    automation_id: &str,
) -> Hold {
    let environment = match request_environment(headers) {
        Ok(AppEnvironment::Production) => return Hold::Proceed,
        Ok(environment) => environment,
        Err(e) => return Hold::Answer(e.into_response()),
    };
    let Some(app) = non_production_app(db, headers, user, project_id, &environment).await else {
        return Hold::Answer(refused(&environment, "an automation run"));
    };
    let mode = match workspace_actions_mode(db, app.id, &environment).await {
        Ok(mode) => mode,
        Err(unreadable) => return Hold::Answer(unreadable),
    };
    let target = preview_mode::PreviewTarget { app, environment };
    match mode {
        WorkspaceActionsMode::Refused => {
            Hold::Answer(refused(&target.environment, "an automation run"))
        }
        WorkspaceActionsMode::Preview => Hold::Preview(Box::new(target)),
        WorkspaceActionsMode::Held => {
            preview_mode::record_run_request(db, &target, user, automation_id, AUTOMATION_SURFACE)
                .await;
            Hold::Answer(held_in_staging())
        }
    }
}

#[derive(Serialize)]
struct ApiErr {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (
        status,
        Json(ApiErr {
            message: msg.into(),
            code: None,
            hint: None,
        }),
    )
        .into_response()
}

fn err_with_code(status: StatusCode, msg: impl Into<String>, code: &'static str) -> Response {
    (
        status,
        Json(ApiErr {
            message: msg.into(),
            code: Some(code),
            hint: None,
        }),
    )
        .into_response()
}

fn err_with_hint(
    status: StatusCode,
    msg: impl Into<String>,
    code: &'static str,
    hint: impl Into<String>,
) -> Response {
    (
        status,
        Json(ApiErr {
            message: msg.into(),
            code: Some(code),
            hint: Some(hint.into()),
        }),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationRunRequest {
    /// Bag of params to inject into the automation's render context.
    /// Each key becomes available to automation SQL templates as
    /// `{{ params.<key> }}`.
    #[serde(default)]
    pub params: Option<JsonValue>,
}

#[derive(Debug, Serialize)]
pub struct AutomationRunStartResponse {
    pub run_id: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AutomationRunPollResponse {
    Running {
        #[serde(skip_serializing_if = "Option::is_none")]
        progress: Option<ProgressFrame>,
    },
    Done {
        result: AutomationResult,
    },
    Failed {
        error: AutomationError,
    },
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgressFrame {
    pub step: String,
    pub percent: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct AutomationError {
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AutomationResult {
    pub summary: String,
    pub outputs: HashMap<String, JsonValue>,
}

#[instrument(skip_all, fields(project_id = %project_id, automation_id = %automation_id))]
pub async fn start_automation_run(
    State(app_state): State<AppState>,
    Path((project_id, automation_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let gates_ctx = match check_custom_app_gates(&headers, project_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let preview = match staging_hold(
        &gates_ctx.db,
        &headers,
        &gates_ctx.user,
        project_id,
        &automation_id,
    )
    .await
    {
        Hold::Proceed => None,
        Hold::Answer(resp) => return resp,
        Hold::Preview(target) => Some(*target),
    };
    let req: AutomationRunRequest = match parse_versioned_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    if let Some(target) = preview {
        return preview_mode::start(&gates_ctx, target, &automation_id, req.params).await;
    }

    let agentic_state = match app_state.agentic_state.as_ref() {
        Some(s) => s.clone(),
        None => {
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                "automation runtime not configured in this deployment",
            );
        }
    };
    let db = agentic_state.db.clone();

    let proj_ctx = match gates_ctx.build_project_context().await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    // The promoted revision is the only source. The id is a file's base name
    // without the double extension (`workflows/foo.automation.yml` is `foo`),
    // and the file may live in any subdirectory, so the lookup lists the
    // revision's automations and matches on that name.
    //
    // This used to fall through to the working copy when the revision missed
    // and the pod held one: list the directory, read the file, parse it. That
    // made the answer depend on which pod served the request, and the working
    // copy is being retired (`internal-docs/factory-retirement.md`, Phase 4).
    // A manager with no pinned revision means nothing is compiled yet, and
    // `compiled_automation` is not asked: on `Origin::Disk` it would read the
    // directory itself.
    let manager = &proj_ctx.workspace_manager().config_manager;
    let lookup = if manager.revision_id().is_some() {
        compiled_automation(manager, &automation_id).await
    } else {
        Lookup::NothingCompiled
    };

    let automation_config = match lookup {
        Lookup::Found(config) => config,
        // The revision listed its automations and none has this file name. A
        // compile cannot fix a misspelt id, so this is not a retry and queues
        // nothing; an automation added since the last compile shows once the
        // compile that promotes it lands.
        Lookup::NoMatch => {
            let tried = automation_basenames(&automation_id).join(", ");
            return err_with_hint(
                StatusCode::NOT_FOUND,
                format!("automation '{automation_id}' not found"),
                "automation_not_found",
                format!(
                    "Looked for: {tried} (anywhere in the compiled revision). Pass the \
                     automation's base name without the extension (e.g. for \
                     `weekly_summary.automation.yml`, `weekly_summary`). An automation \
                     added since the last compile appears once that compile lands."
                ),
            );
        }
        // The row is in the revision and is not an automation this runner can
        // run. The same answer as before, and no compile changes it.
        Lookup::Invalid(e) => {
            warn!(error = %e, "compiled automation did not deserialise");
            return err_with_code(
                StatusCode::BAD_REQUEST,
                format!("automation YAML parse failed: {e}"),
                "automation_invalid_yaml",
            );
        }
        // Nothing to answer from yet, or the read itself failed: a compile (or
        // a moment) is the fix.
        Lookup::NothingCompiled | Lookup::Failed => {
            if let Ok(db) = oxy::database::client::establish_connection().await {
                oxy_app::surface::enqueue_lazy_compile(&db, project_id).await;
            }
            return err_with_code(
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "automation {automation_id} could not be read from a compiled revision; a \
                     compile has been enqueued — retry shortly"
                ),
                "automation_needs_recompile",
            );
        }
    };

    // Register the run and hand it to the queue. Nothing is driven here: this
    // replica may be gone before the automation's first step, and the run must
    // not go with it. The task carries the automation resolved above and the
    // caller the gate authenticated — see `task` for why both travel by value.
    let queued = match task::ProcedureRunTask::for_request(
        &gates_ctx,
        &automation_id,
        &automation_config,
        req.params,
    ) {
        Ok(t) => t,
        Err(e) => {
            error!(error = %e, "automation run could not be encoded for the queue");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not register automation run",
            );
        }
    };
    if let Err(e) = task::submit(&db, &queued).await {
        error!(run_id = %queued.run_id, error = %e, "automation run insert failed");
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not register automation run",
        );
    }

    let resp = AutomationRunStartResponse {
        run_id: queued.run_id.to_string(),
    };
    (StatusCode::ACCEPTED, Json(resp)).into_response()
}

#[instrument(skip_all, fields(project_id = %project_id, run_id = %run_id))]
pub async fn poll_automation_run(
    State(app_state): State<AppState>,
    Path((project_id, run_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
) -> Response {
    let _gates_ctx = match check_custom_app_gates(&headers, project_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let agentic_state = match app_state.agentic_state.as_ref() {
        Some(s) => s.clone(),
        None => {
            return err(
                StatusCode::SERVICE_UNAVAILABLE,
                "automation runtime not configured",
            );
        }
    };
    let run_uuid = match Uuid::parse_str(&run_id) {
        Ok(u) => u,
        Err(_) => {
            return err_with_code(
                StatusCode::BAD_REQUEST,
                "invalid run_id",
                "automation_run_invalid_id",
            );
        }
    };
    let row = match proc_run::Entity::find_by_id(run_uuid)
        .one(&agentic_state.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return err_with_code(
                StatusCode::NOT_FOUND,
                "automation run not found",
                "automation_run_not_found",
            );
        }
        Err(e) => {
            error!(run_id = %run_id, error = %e, "automation poll lookup failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "lookup failed");
        }
    };
    // Cross-project leakage defense.
    if row.workspace_id != project_id {
        return err_with_code(
            StatusCode::FORBIDDEN,
            "run does not belong to this project",
            "thread_project_mismatch",
        );
    }

    // A run its driver gave up on never gets a terminal write from that
    // driver. Close it here rather than leave the bundle polling `running`
    // until the two-hour sweep. One primary-key read, only while `running`.
    let row = settle::reconcile_abandoned(&agentic_state.db, row).await;

    let resp = match row.status.as_str() {
        "running" => AutomationRunPollResponse::Running {
            progress: row.progress_step.as_ref().map(|step| ProgressFrame {
                step: step.clone(),
                percent: row.progress_percent.unwrap_or(0).clamp(0, 100) as u8,
            }),
        },
        "done" => {
            let summary = row
                .result_summary
                .clone()
                .unwrap_or_else(|| "Automation completed.".to_string());
            let outputs: HashMap<String, JsonValue> = row
                .result_outputs
                .clone()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            AutomationRunPollResponse::Done {
                result: AutomationResult { summary, outputs },
            }
        }
        "failed" => AutomationRunPollResponse::Failed {
            error: AutomationError {
                message: row
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "automation failed".into()),
                code: row.error_code.clone(),
            },
        },
        "cancelled" => AutomationRunPollResponse::Cancelled,
        other => AutomationRunPollResponse::Failed {
            error: AutomationError {
                message: format!("unexpected status: {other}"),
                code: Some("automation_unknown_status".into()),
            },
        },
    };
    Json(resp).into_response()
}

/// `POST /api/projects/{project_id}/procedures/runs/{run_id}/cancel`
///
/// Two-step cancel: close the DB row as `cancelled` (durable; any replica
/// can do it, and pollers see the terminal state on their next request)
/// AND write the run's durable cancel flag, which the driver — usually in
/// another process — polls and turns into a stop of the LLM call / SQL
/// execution within a few seconds. Returns 204 on success.
#[instrument(skip_all, fields(project_id = %project_id, run_id = %run_id))]
pub async fn cancel_automation_run(
    State(app_state): State<AppState>,
    Path((project_id, run_id)): Path<(Uuid, String)>,
    headers: HeaderMap,
) -> Response {
    let gates_ctx = match check_custom_app_gates(&headers, project_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let agentic_state = match app_state.agentic_state.as_ref() {
        Some(s) => s.clone(),
        None => return err(StatusCode::SERVICE_UNAVAILABLE, "runtime not configured"),
    };
    let run_uuid = match Uuid::parse_str(&run_id) {
        Ok(u) => u,
        Err(_) => {
            return err_with_code(
                StatusCode::BAD_REQUEST,
                "invalid run_id",
                "automation_run_invalid_id",
            );
        }
    };
    // Outside production only an environment's own preview runs may be
    // cancelled (the guard let this through because its plane is `preview`).
    if let Err(refusal) = preview_mode::may_cancel(
        &agentic_state.db,
        &headers,
        &gates_ctx.user,
        project_id,
        run_uuid,
    )
    .await
    {
        return refusal;
    }
    let row = match proc_run::Entity::find_by_id(run_uuid)
        .one(&agentic_state.db)
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => {
            return err_with_code(
                StatusCode::NOT_FOUND,
                "automation run not found",
                "automation_run_not_found",
            );
        }
        Err(e) => {
            error!(run_id = %run_id, error = %e, "cancel: lookup failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "lookup failed");
        }
    };
    if row.workspace_id != project_id {
        return err_with_code(
            StatusCode::FORBIDDEN,
            "run does not belong to this project",
            "thread_project_mismatch",
        );
    }
    // Idempotent: already terminal → no-op.
    if matches!(row.status.as_str(), "done" | "failed" | "cancelled") {
        return StatusCode::NO_CONTENT.into_response();
    }

    // Stamp the cancel and close the row, in one statement.
    //
    // The driver is normally another process, so there is no handle here to
    // abort. Closing the row now is what lets pollers see `cancelled` from
    // whichever replica took this request, rather than `running` until the
    // driver notices. It is safe against every race because a terminal write
    // only ever moves a `running` row (`settle`): a driver that finishes a
    // moment later finds the row closed and discards its result, and one that
    // claims the task later finds it closed and does not start.
    //
    // Race: the driver can flip the row to `done` between the status check
    // above and this write. The `WHERE status = 'running'` is what keeps this
    // from stamping a terminal row — a leftover `cancel_requested_at` on a
    // `done` row would otherwise stick around until the 24h TTL eviction.
    let mut closing = settle::cancelled();
    closing.cancel_requested_at = sea_orm::ActiveValue::Set(Some(Utc::now().into()));
    match settle::close_running(&agentic_state.db, run_uuid, closing, settle::Guard::Running).await
    {
        // Row reached a terminal state between the status read and this
        // write. The terminal row is the right answer; nothing more to do.
        Ok(false) => return StatusCode::NO_CONTENT.into_response(),
        Ok(true) => {
            preview_mode::end_queued_of_closed_polls(&agentic_state.db, Some(run_uuid)).await
        }
        Err(e) => {
            warn!(run_id = %run_id, error = %e, "cancel: DB stamp failed");
        }
    }

    // Tell the driver to stop. `request_cancel` sets the durable flag on the
    // run; the driver's cancel forwarder polls it and trips the task's cancel
    // token, which drops the automation mid-step. Without it the row would
    // read `cancelled` while the run burned LLM and warehouse budget to its
    // natural end. A row with no queued twin (a run started before the queue
    // existed) matches nothing here, and that is fine.
    if let Err(e) =
        agentic_runtime::crud::request_cancel(&agentic_state.db, &run_uuid.to_string()).await
    {
        warn!(run_id = %run_id, error = %e, "cancel: durable cancel flag failed");
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Periodic maintenance for the `customer_app_procedure_runs` table.
/// Combines three jobs into one pass so the startup loop only has
/// to schedule one task. Caller invokes this on a timer (default:
/// every 10 min from `spawn_periodic_sweep`).
///
/// 1. **Evict terminal rows older than 24h.** Bundles polling
///    after-the-fact get `automation_run_not_found` instead of a
///    stale `done`; aligns with the spec's TTL.
///
/// 2. **Reconcile stamp-only cancels.** The cancel endpoint closes
///    the row itself, so a row with `status = 'running'` AND
///    `cancel_requested_at` set was stamped by a replica still on a
///    release that only stamped, and no driver has claimed the task
///    since to honour it. Promote to `cancelled` once the stamp is
///    older than the grace window.
///
/// 3. **Mark stuck-running rows failed.** A row with
///    `status = 'running'` and `started_at` older than 2 hours has
///    no driver that will finish it — a run from before the queue
///    whose replica died, or one no driver ever claimed. Mark
///    `failed` with a clear code. This is the backstop; a run its
///    driver gave up on is closed sooner, by the poll endpoint.
pub async fn sweep_terminal_runs(
    db: &sea_orm::DatabaseConnection,
) -> Result<SweepReport, sea_orm::DbErr> {
    use sea_orm::sea_query::Expr;

    // 1. Terminal eviction.
    let evict_cutoff = Utc::now() - chrono::Duration::hours(24);
    let evicted = proc_run::Entity::delete_many()
        .filter(proc_run::Column::CompletedAt.lt(evict_cutoff))
        .filter(Expr::col(proc_run::Column::Status).is_in(["done", "failed", "cancelled"]))
        .exec(db)
        .await?
        .rows_affected;

    // 2. Cross-instance cancel reconciliation. 30s grace lets the
    //    originating instance write its own cancelled row first.
    let cancel_grace = Utc::now() - chrono::Duration::seconds(30);
    let now: chrono::DateTime<chrono::FixedOffset> = Utc::now().into();
    let stuck_cancelled = proc_run::Entity::update_many()
        .col_expr(proc_run::Column::Status, Expr::value("cancelled"))
        .col_expr(proc_run::Column::CompletedAt, Expr::value(now))
        .col_expr(
            proc_run::Column::ErrorMessage,
            Expr::value("cancelled by user (reconciled by sweep)"),
        )
        .col_expr(
            proc_run::Column::ErrorCode,
            Expr::value("automation_run_cancelled"),
        )
        .filter(proc_run::Column::Status.eq("running"))
        .filter(proc_run::Column::CancelRequestedAt.lt(cancel_grace))
        .exec(db)
        .await?
        .rows_affected;

    // 3. Stuck-running detection. 2-hour cutoff: longest legitimate
    //    automations run in minutes; anything `running` for 2 hours
    //    is orphaned. Guard against double-counting rows step 2 just
    //    handled by requiring `cancel_requested_at IS NULL`. The cutoff is
    //    named because an attempt stepping aside for a live one waits no
    //    longer than this (`executor::admission`).
    let stuck_cutoff = Utc::now() - chrono::Duration::seconds(settle::ORPHAN_SWEEP_AFTER_SECS);
    let stuck_failed = proc_run::Entity::update_many()
        .col_expr(proc_run::Column::Status, Expr::value("failed"))
        .col_expr(proc_run::Column::CompletedAt, Expr::value(now))
        .col_expr(
            proc_run::Column::ErrorMessage,
            Expr::value("automation timed out (no progress for 2+ hours)"),
        )
        .col_expr(
            proc_run::Column::ErrorCode,
            Expr::value("automation_run_orphaned"),
        )
        .filter(proc_run::Column::Status.eq("running"))
        .filter(proc_run::Column::StartedAt.lt(stuck_cutoff))
        .filter(proc_run::Column::CancelRequestedAt.is_null())
        .exec(db)
        .await?
        .rows_affected;

    // A poll row failed or cancelled above may belong to a preview run that
    // never started: end that too, or it would run what the bundle was told
    // had failed.
    if stuck_cancelled + stuck_failed > 0 {
        preview_mode::end_queued_of_closed_polls(db, None).await;
    }

    Ok(SweepReport {
        evicted,
        stuck_cancelled,
        stuck_failed,
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SweepReport {
    pub evicted: u64,
    pub stuck_cancelled: u64,
    pub stuck_failed: u64,
}

/// Spawn the periodic sweep task. Runs every 10 minutes — fast
/// enough that stuck cancels become terminal within typical
/// polling windows, slow enough not to load the DB with delete-
/// many sweeps. Stops cleanly on shutdown.
pub fn spawn_periodic_sweep(
    db: sea_orm::DatabaseConnection,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(10 * 60));
        // Skip the immediate first tick — startup is busy enough.
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    match sweep_terminal_runs(&db).await {
                        Ok(report) => {
                            if report.evicted + report.stuck_cancelled + report.stuck_failed > 0 {
                                tracing::info!(
                                    evicted = report.evicted,
                                    stuck_cancelled = report.stuck_cancelled,
                                    stuck_failed = report.stuck_failed,
                                    "custom-app procedure runs swept",
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "custom-app procedure sweep failed");
                        }
                    }
                }
                _ = shutdown.cancelled() => {
                    tracing::debug!("custom-app procedure sweep shutting down");
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests;

/// The automation's compiled definition, matched by file name on the manager's
/// revision. `None` on any miss; the caller answers "not compiled yet, retry".
/// Callers must check the manager pins a revision first: on `Origin::Disk` the
/// manager reads its directory and this would too.
///
/// Generic over the capability, not bound to `WorkingCopy`: these are Postgres
/// reads keyed by revision, so they are exactly as valid on a replica that owns
/// no disk.
///
/// Matching is by `name`, which `oxy-compile` derives from the file stem, so it
/// Whether `file_path` is the file `automation_id` names.
///
/// The id is a file's base name without the double extension — that is the
/// contract the 503 message states, and what a custom app passes.
///
/// Shared because the two sources answered differently. The FS arm always
/// matched this way; the compiled arm matched `AutomationEntry::name`, which is
/// the YAML `name:` when the file declares one (`oxy_compile::compile_
/// automation`) and only falls back to the path otherwise. Twelve of the
/// eighteen `examples/procedures/*.automation.yml` declare a name that differs
/// from their filename, so the compiled lookup missed for all of them, and the
/// miss read as "not found" (or, on a node with no working copy, as a `503`
/// retry that recompiling cannot clear) for an automation that IS in the
/// revision.
fn automation_basenames(automation_id: &str) -> [String; 2] {
    [
        format!("{automation_id}.procedure.yml"),
        format!("{automation_id}.automation.yml"),
    ]
}

fn matches_automation_id(file_path: &str, automation_id: &str) -> bool {
    let targets = automation_basenames(automation_id);
    std::path::Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| targets.iter().any(|t| t == n))
}

/// What looking an automation up in the manager's revision found.
enum Lookup {
    Found(AutomationConfig),
    /// The revision listed its automations and none matches the id.
    NoMatch,
    /// A row matches and its definition is not an automation this runner can
    /// run.
    Invalid(serde_json::Error),
    /// The manager reads no revision, so there is nothing compiled to look in.
    /// Not produced by [`compiled_automation`]: the caller checks first,
    /// because on `Origin::Disk` that function would read the directory.
    NothingCompiled,
    /// The boundary did not answer, or answered inconsistently (a listed row
    /// with no definition).
    Failed,
}

/// The automation `automation_id` names in the manager's revision, and why not
/// when there is none. The caller answers each reason differently: only
/// [`Lookup::Failed`] is helped by a compile.
async fn compiled_automation<S: oxy::config::DiskSlot + Send + Sync>(
    config_manager: &ConfigManager<S>,
    automation_id: &str,
) -> Lookup {
    let rows = match config_manager.list_automations().await {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "automation list failed");
            return Lookup::Failed;
        }
    };
    let Some(row) = rows
        .into_iter()
        .find(|r| matches_automation_id(&r.file_path, automation_id))
    else {
        return Lookup::NoMatch;
    };
    let artifact = match config_manager.automation_definition(&row.file_path).await {
        Ok(Some(artifact)) => artifact,
        Ok(None) => {
            warn!(file_path = %row.file_path, "a listed automation has no definition");
            return Lookup::Failed;
        }
        Err(e) => {
            warn!(error = ?e, "automation read failed");
            return Lookup::Failed;
        }
    };
    match serde_json::from_value(artifact) {
        Ok(config) => Lookup::Found(config),
        Err(e) => Lookup::Invalid(e),
    }
}
