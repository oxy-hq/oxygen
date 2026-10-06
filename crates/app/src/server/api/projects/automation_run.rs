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
//! Staging ([`staging_hold`]): the environment guard lets this route's
//! `POST` through on a custom app's staging host
//! (`custom_app_env_request::is_staging_write`), same as it lets an
//! agent ask through — but unlike an ask, an automation run never runs held.
//! `start_automation_run` refuses it outright with `409 held_in_staging`,
//! logged as one `app.staging.held` row for a caller who may open this app's
//! staging; a caller who may not gets the same `404 EnvironmentRefused` a
//! staging ask refuses with. Production is unaffected.

pub mod executor;
mod settle;
pub mod task;

use std::collections::HashMap;

use agentic_automation::AutomationConfig;
use agentic_pipeline::automation_run::AutomationRunError;
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

use crate::server::api::custom_apps_env_resolve::may_open_non_production;
use crate::server::api::custom_apps_functions::write_record::WriteRecord;
use crate::server::api::custom_apps_gates::{check_custom_app_gates, parse_versioned_body};
use crate::server::api::custom_apps_staging_held::{HeldActor, HeldRow, record_held};
use crate::server::api::custom_apps_staging_pin::request::find_app;
use crate::server::api::projects::agent_ask_staging::{ask_app_ref, refused};
use crate::server::router::AppState;
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

/// The held write an automation run would have made, as the audit shape:
/// never a table, since nothing ran.
fn automation_write(automation_id: &str) -> WriteRecord {
    WriteRecord {
        plane: "automation",
        namespace: automation_id.to_string(),
        verb: "RUN".to_string(),
        table: String::new(),
        rows: None,
        statements: 1,
        op: None,
        note: None,
    }
}

/// What a staging host's automation-run request answers, before anything is
/// looked up or written: `Ok(())` in production (unchanged), else `Err` —
/// `409` logged as one `app.staging.held` row for a caller who may open this
/// app's staging, else the same `404 EnvironmentRefused` a staging ask
/// refuses with. Fail closed: a lookup miss is a refusal, never a run.
async fn staging_hold(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    user: &AuthenticatedUser,
    project_id: Uuid,
    automation_id: &str,
) -> Result<(), Response> {
    let refusal = || refused(&AppEnvironment::Staging, "an automation run");
    let environment = request_environment(headers).map_err(|e| e.into_response())?;
    match environment {
        AppEnvironment::Production => return Ok(()),
        AppEnvironment::Staging => {}
        other => return Err(refused(&other, "an automation run")),
    }
    let app = match ask_app_ref(headers) {
        Some(r) => find_app(db, &r).await,
        None => None,
    };
    let Some(app) = app else {
        return Err(refusal());
    };
    if app.project_id != project_id {
        return Err(refusal());
    }
    let caller = crate::server::authz::Caller::from_user(user);
    if !may_open_non_production(db, &caller, &app).await {
        return Err(refusal());
    }
    record_held(
        db,
        HeldRow {
            app_id: app.id,
            app_slug: app.slug.clone(),
            org_id: app.org_id,
            project_id,
            environment: AppEnvironment::Staging.name(),
            actor: HeldActor::User {
                id: user.id,
                email: user.email.clone(),
            },
            function_or_surface: AUTOMATION_SURFACE.to_string(),
            mode: AUTOMATION_SURFACE.to_string(),
            request_id: None,
            writes: vec![automation_write(automation_id)],
            invocation_id: None,
            trace_id: None,
            token_id: None,
        },
    )
    .await;
    Err(held_in_staging())
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
    if let Err(resp) = staging_hold(
        &gates_ctx.db,
        &headers,
        &gates_ctx.user,
        project_id,
        &automation_id,
    )
    .await
    {
        return resp;
    }
    let req: AutomationRunRequest = match parse_versioned_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

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

    // Discover all automation files (.procedure.yml / .automation.yml)
    // recursively under the workspace root, matching the convention used
    // by `list_workflows` in the config manager.
    // The custom-app automation-id is the file's base name without the
    // double extension; the file may live in any subdirectory (e.g.
    // `workflows/foo.automation.yml`), not just the project root.
    // Compile boundary first. This handler serves custom-app bundles from the
    // public router, so on a replica there is no working copy — and since
    // `require_root()` landed, `list_workflows()` errors there rather than
    // answering `[]`. That is the right error and the wrong outcome for a route
    // whose artifact is already compiled.
    let compiled: Option<AutomationConfig> =
        compiled_automation(&proj_ctx.workspace_manager().config_manager, &automation_id).await;

    // Boundary missed and there is nothing to fall through to. `list_workflows()`
    // would error here (that is `require_root`), but a 500 describes a fault on
    // this node; the truth is the workspace is not compiled yet.
    if compiled.is_none() && !proj_ctx.workspace_manager().config_manager.can_read_disk() {
        if let Ok(db) = oxy::database::client::establish_connection().await {
            crate::server::api::middlewares::workspace_context::enqueue_lazy_compile(
                &db, project_id,
            )
            .await;
        }
        return err_with_code(
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "automation {automation_id} is not in the compiled revision and this \
                 instance holds no working copy; a compile has been enqueued — retry shortly"
            ),
            "automation_needs_recompile",
        );
    }

    let automation_config: AutomationConfig = if let Some(config) = compiled {
        config
    } else {
        let all_automations = match proj_ctx
            .workspace_manager()
            .config_manager
            .list_automations()
            .await
        {
            Ok(w) => w,
            Err(e) => {
                error!(error = %e, "list_workflows failed");
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "could not enumerate project workflows",
                );
            }
        };
        // Collect *all* matches by basename so we can detect collisions —
        // `list_workflows()` walks `read_dir` in filesystem-dependent order,
        // so a bare `.find()` against duplicate basenames in different
        // subdirectories (`workflows/refresh.automation.yml` and
        // `staging/refresh.automation.yml`) resolves non-deterministically.
        // Pick the alphabetically-first match for stable behaviour and
        // `warn!` so the operator notices the collision.
        let mut matches: Vec<&str> = all_automations
            .iter()
            .map(|a| a.file_path.as_str())
            .filter(|rel| matches_automation_id(rel, &automation_id))
            .collect();
        matches.sort();
        let automation_path = match matches.first() {
            Some(p) => {
                if matches.len() > 1 {
                    let all_paths: Vec<String> = matches.iter().map(|p| p.to_string()).collect();
                    warn!(
                        automation_id = %automation_id,
                        chosen = %p,
                        matches = ?all_paths,
                        "automation-id resolved to multiple automation files; picked the \
                         alphabetically-first one. Consider renaming one of the files \
                         or passing a qualified path."
                    );
                }
                // Workspace-relative now. `resolve_file` turns it absolute
                // through the storage layer, which checks the result is still
                // inside the project — `workspace_path().join(..)` would not.
                match proj_ctx
                    .workspace_manager()
                    .config_manager
                    .resolve_file(p)
                    .await
                {
                    Ok(abs) => std::path::PathBuf::from(abs),
                    Err(e) => {
                        error!(path = %p, error = %e, "resolve automation path failed");
                        return err(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "could not resolve automation file",
                        );
                    }
                }
            }
            None => {
                let tried = automation_basenames(&automation_id).join(", ");
                return err_with_hint(
                    StatusCode::NOT_FOUND,
                    format!("automation '{automation_id}' not found"),
                    "automation_not_found",
                    format!(
                        "Looked for: {tried} (recursively under the project root). \
                         Pass the automation's base name without the extension \
                         (e.g. for `weekly_summary.procedure.yml`, call \
                         `useProcedureRun({{ procedureId: 'weekly_summary' }})`)."
                    ),
                );
            }
        };

        // `tokio::fs::read_to_string` so we don't block the executor
        // thread on potentially-slow disk I/O. Matches the workspace
        // /workflows route's pattern. YAML parsing below is synchronous
        // but fast enough on automation-sized files (~10 KB typical) that
        // spawn_blocking adds more overhead than it saves.
        let automation_yaml = match tokio::fs::read_to_string(&automation_path).await {
            Ok(s) => s,
            Err(e) => {
                error!(path = ?automation_path, error = %e, "read automation file failed");
                return err(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "could not read automation file",
                );
            }
        };
        match serde_yaml::from_str(&automation_yaml) {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "automation YAML parse failed");
                return err_with_code(
                    StatusCode::BAD_REQUEST,
                    format!("automation YAML parse failed: {e}"),
                    "automation_invalid_yaml",
                );
            }
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

/// Best-effort categorization of automation-runner failures.
fn automation_error_to_code(e: &AutomationRunError) -> (&'static str, String) {
    let msg = e.to_string();
    let lower = msg.to_lowercase();
    let code = if lower.contains("agent")
        && (lower.contains("not configured") || lower.contains("inlineagentrunner"))
    {
        "automation_requires_agent_runner"
    } else if lower.contains("rate limit") {
        "automation_rate_limited"
    } else if lower.contains("timeout") || lower.contains("timed out") {
        "automation_run_timeout"
    } else if lower.contains("connect") || lower.contains("connection") {
        "automation_warehouse_unreachable"
    } else {
        "automation_run_failed"
    };
    (code, msg)
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
    let _gates_ctx = match check_custom_app_gates(&headers, project_id).await {
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
        Ok(true) => {}
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
mod tests {
    use super::*;

    /// The same rule, exercised through the real lookup rather than the helper.
    ///
    /// A unit test on `matches_automation_id` alone does not bind the wiring:
    /// restore `find(|r| r.name == automation_id)` and it still passes. This
    /// goes through `compiled_automation`, so the call site is what is pinned.
    ///
    /// `Origin::Disk`, so no database is needed — `list_automations` reads the
    /// working copy and produces exactly the `AutomationEntry` shape the
    /// compiled arm gets: `name` from the YAML, `file_path` from the path.
    #[tokio::test]
    async fn the_lookup_finds_an_automation_whose_yaml_renames_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
            .await
            .expect("config");
        tokio::fs::create_dir_all(dir.path().join("procedures"))
            .await
            .expect("mkdir");
        // The shape 12 of the 18 shipped examples have.
        tokio::fs::write(
            dir.path().join("procedures/anonymize.automation.yml"),
            "name: anonymize_sample\ntasks: []\n",
        )
        .await
        .expect("automation");

        let manager = oxy::config::ConfigBuilder::new()
            .with_workspace_path(dir.path())
            .expect("workspace path")
            .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
            .await
            .expect("manager");

        assert!(
            compiled_automation(&manager, "anonymize").await.is_some(),
            "the id is the filename; matching the YAML `name:` misses every \
             automation that renames itself, and on a diskless node that miss \
             is a permanent 503"
        );
    }

    /// The two sources must answer the same question the same way.
    ///
    /// `automation_id` is a file's base name. The compiled arm used to match
    /// `AutomationEntry::name`, which is the YAML `name:` when the file
    /// declares one — twelve of the eighteen `examples/procedures/*.automation
    /// .yml` declare one that differs from their filename. Every one of those
    /// missed on a node with no working copy, and the miss becomes a `503
    /// "a compile has been enqueued — retry shortly"` that recompiling cannot
    /// clear, for an automation that IS in the revision.
    #[test]
    fn an_automation_is_found_by_its_filename_not_its_declared_name() {
        // The real shape: `anonymize.automation.yml` declaring
        // `name: anonymize_sample`.
        assert!(
            matches_automation_id("procedures/anonymize.automation.yml", "anonymize"),
            "the id is the filename, whatever the YAML calls itself"
        );
        assert!(
            !matches_automation_id("procedures/anonymize.automation.yml", "anonymize_sample"),
            "and the declared name is NOT an id — matching it would make the \
             compiled arm answer a question the FS arm never answers"
        );

        // The legacy extension, and a nested path.
        assert!(matches_automation_id(
            "workflows/staging/refresh.procedure.yml",
            "refresh"
        ));

        // A prefix is not a match, and neither is a different extension.
        assert!(!matches_automation_id(
            "procedures/anonymize_v2.automation.yml",
            "anonymize"
        ));
        assert!(!matches_automation_id(
            "procedures/anonymize.yml",
            "anonymize"
        ));
    }

    #[test]
    fn automation_error_classifies_known_patterns() {
        let cases = [
            (
                "connection refused to warehouse",
                "automation_warehouse_unreachable",
            ),
            ("OpenAI returned rate limit", "automation_rate_limited"),
            ("query timed out", "automation_run_timeout"),
            (
                "InlineAgentRunner not configured",
                "automation_requires_agent_runner",
            ),
            ("something else entirely", "automation_run_failed"),
        ];
        for (msg, want) in cases {
            let e = AutomationRunError::Inline(msg.to_string());
            let (code, _) = automation_error_to_code(&e);
            assert_eq!(code, want, "for message {msg:?}");
        }
    }
}

/// The automation's compiled definition, matched by name on the manager's
/// revision. `None` on any miss — including "this manager reads the working
/// copy", which the trait reports as `Ok(None)` — and the caller falls through
/// to the working copy, which is the compile-boundary contract.
///
/// Generic over the capability, not bound to `WorkingCopy`: these are Postgres
/// reads keyed by revision, so they are exactly as valid on a replica that owns
/// no disk.
///
/// Matching is by `name`, which `oxy-compile` derives from the file stem, so it
/// Whether `file_path` is the file `automation_id` names.
///
/// The id is a file's base name without the double extension — that is the
/// contract the 404 hint states, and what a custom app passes.
///
/// Shared because the two sources answered differently. The FS arm always
/// matched this way; the compiled arm matched `AutomationEntry::name`, which is
/// the YAML `name:` when the file declares one (`oxy_compile::compile_
/// automation`) and only falls back to the path otherwise. Twelve of the
/// eighteen `examples/procedures/*.automation.yml` declare a name that differs
/// from their filename, so the compiled lookup missed for all of them — and on
/// a node with no working copy the miss hits `can_read_disk()` and answers
/// `503 "a compile has been enqueued — retry shortly"`. Recompiling cannot
/// change the name, so that 503 is permanent for an automation that IS in the
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

/// is the same key the filesystem lookup below uses minus the extension. The
/// collision handling there exists because `read_dir` order is unstable; a
/// compiled row set is keyed and needs none.
async fn compiled_automation<S: oxy::config::DiskSlot + Send + Sync>(
    config_manager: &ConfigManager<S>,
    automation_id: &str,
) -> Option<AutomationConfig> {
    let rows = config_manager
        .list_automations()
        .await
        .map_err(|e| warn!(error = %e, "automation list failed"))
        .ok()?;
    let row = rows
        .into_iter()
        .find(|r| matches_automation_id(&r.file_path, automation_id))?;
    let artifact = config_manager
        .automation_definition(&row.file_path)
        .await
        .map_err(|e| warn!(error = ?e, "automation read failed"))
        .ok()??;
    serde_json::from_value(artifact)
        .map_err(|e| warn!(error = %e, "compiled automation did not deserialise"))
        .ok()
}
