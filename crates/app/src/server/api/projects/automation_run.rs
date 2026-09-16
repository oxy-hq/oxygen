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
//! automation runs in a spawned task; state lives in the
//! `customer_app_procedure_runs` DB table (see
//! `migration::m20260526_000001_create_customer_app_procedure_runs`)
//! so server restarts don't drop in-flight runs from the bundle's
//! point of view.
//!
//! Cancellation: the cancel endpoint stamps `cancel_requested_at` on
//! the row AND aborts the JoinHandle in the in-process registry. The
//! abort is the fast path (kills the spawn immediately); the DB
//! stamp is the durable record for cross-instance / restart cases.

use std::collections::HashMap;
use std::sync::Arc;

use agentic_automation::AutomationConfig;
use agentic_pipeline::automation_run::AutomationRunError;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use dashmap::DashMap;
use entity::customer_app_procedure_runs as proc_run;
use entity::customer_app_procedure_runs::ActiveModel as ProcRunActiveModel;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use sentry::SentryFutureExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::task::JoinHandle;
use tracing::{error, instrument, warn};
use uuid::Uuid;

use oxy::config::ConfigManager;

use crate::server::api::custom_apps_gates::{check_custom_app_gates, parse_versioned_body};
use crate::server::router::AppState;
use sea_orm::ExprTrait;

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

/// In-process map of run_id → tokio JoinHandle. Used by the cancel
/// endpoint to abort the spawned task immediately on the same
/// instance. DB row's `cancel_requested_at` is the durable record;
/// this is just the fast path. Cleaned up when the spawned task
/// completes.
fn join_handles() -> &'static DashMap<String, JoinHandle<()>> {
    use std::sync::OnceLock;
    static HANDLES: OnceLock<DashMap<String, JoinHandle<()>>> = OnceLock::new();
    HANDLES.get_or_init(DashMap::new)
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

    let run_id = Uuid::new_v4();
    let now = Utc::now().into();
    let insert = ProcRunActiveModel {
        id: ActiveValue::Set(run_id),
        workspace_id: ActiveValue::Set(project_id),
        procedure_id: ActiveValue::Set(automation_id.clone()),
        status: ActiveValue::Set("running".to_string()),
        params: ActiveValue::Set(req.params.clone()),
        progress_step: ActiveValue::Set(None),
        progress_percent: ActiveValue::Set(None),
        result_summary: ActiveValue::Set(None),
        result_outputs: ActiveValue::Set(None),
        error_message: ActiveValue::Set(None),
        error_code: ActiveValue::Set(None),
        cancel_requested_at: ActiveValue::Set(None),
        started_at: ActiveValue::Set(now),
        completed_at: ActiveValue::Set(None),
    };
    if let Err(e) = insert.insert(&db).await {
        error!(run_id = %run_id, error = %e, "automation run insert failed");
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not register automation run",
        );
    }

    let render_context = req
        .params
        .as_ref()
        .map(|p| serde_json::json!({ "params": p }));

    let proj_ctx_run = proj_ctx;
    let db_for_task = db.clone();
    let run_id_str = run_id.to_string();
    let run_id_for_task = run_id_str.clone();
    let handle = tokio::spawn(
        async move {
            let workspace: Arc<dyn agentic_automation::WorkspaceContext> = Arc::new(proj_ctx_run);
            let result = agentic_pipeline::automation_run::run_inline_automation_with_render_context(
                workspace.as_ref(),
                automation_config,
                None,
                render_context,
                None,
            )
            .await;

            // Was a cancel requested mid-run? Check the DB row before we
            // record a result so a race between user-cancel + automation-
            // completion lands on the right terminal state.
            let cancel_seen = proc_run::Entity::find_by_id(run_id)
                .one(&db_for_task)
                .await
                .ok()
                .flatten()
                .and_then(|r| r.cancel_requested_at)
                .is_some();

            let update = match (cancel_seen, result) {
                (true, _) => set_cancelled(run_id),
                (false, Ok(outputs)) => set_done(run_id, outputs),
                (false, Err(e)) => set_failed(run_id, &e),
            };
            if let Err(db_err) = update.update(&db_for_task).await {
                error!(run_id = %run_id_for_task, error = %db_err, "automation run completion update failed");
            }
            join_handles().remove(&run_id_for_task);
        }
        // The run outlives the 202 this handler returns, so the request's
        // hub is gone once the automation starts. Its tasks are
        // tenant-authored and its failures carry their SQL and results;
        // `agentic_pipeline`'s targets are not `custom_apps`, so barrier 1
        // does not see them and the tag must travel
        // (`middlewares::sentry_surface`).
        .bind_hub(sentry::Hub::current()),
    );
    join_handles().insert(run_id_str.clone(), handle);

    let resp = AutomationRunStartResponse { run_id: run_id_str };
    (StatusCode::ACCEPTED, Json(resp)).into_response()
}

fn set_done(run_id: Uuid, outputs: HashMap<String, JsonValue>) -> ProcRunActiveModel {
    let summary = if outputs.is_empty() {
        "Automation completed.".to_string()
    } else {
        format!("Automation completed — {} task outputs.", outputs.len())
    };
    let outputs_json = serde_json::to_value(&outputs).unwrap_or(JsonValue::Null);
    ProcRunActiveModel {
        id: ActiveValue::Set(run_id),
        status: ActiveValue::Set("done".into()),
        result_summary: ActiveValue::Set(Some(summary)),
        result_outputs: ActiveValue::Set(Some(outputs_json)),
        completed_at: ActiveValue::Set(Some(Utc::now().into())),
        ..Default::default()
    }
}

fn set_failed(run_id: Uuid, e: &AutomationRunError) -> ProcRunActiveModel {
    let (code, message) = automation_error_to_code(e);
    ProcRunActiveModel {
        id: ActiveValue::Set(run_id),
        status: ActiveValue::Set("failed".into()),
        error_message: ActiveValue::Set(Some(message)),
        error_code: ActiveValue::Set(Some(code.to_string())),
        completed_at: ActiveValue::Set(Some(Utc::now().into())),
        ..Default::default()
    }
}

fn set_cancelled(run_id: Uuid) -> ProcRunActiveModel {
    ProcRunActiveModel {
        id: ActiveValue::Set(run_id),
        status: ActiveValue::Set("cancelled".into()),
        error_message: ActiveValue::Set(Some("cancelled by user".into())),
        error_code: ActiveValue::Set(Some("automation_run_cancelled".into())),
        completed_at: ActiveValue::Set(Some(Utc::now().into())),
        ..Default::default()
    }
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
/// Two-step cancel: stamp the DB row (durable; visible cross-instance,
/// survives restart) AND abort the in-process JoinHandle (fast; kills
/// the LLM call / SQL execution immediately on this instance). Returns
/// 204 on success — pollers see the terminal state on next request.
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

    // Stamp the durable cancel marker. The spawned task reads this
    // after the automation returns; if the abort below kills it first,
    // we still need a non-stale DB record so a future poll sees the
    // right state.
    //
    // Race: the spawned task can flip the row to `done` between the
    // status check above and this stamp. Re-check inside an UPDATE
    // … WHERE status = 'running' so we don't stamp a terminal row
    // — the sweep filter at sweep_terminal_runs only acts on
    // running rows, so a leftover cancel_requested_at on a `done`
    // row would otherwise stick around until the 24h TTL eviction.
    let stamp_res = proc_run::Entity::update_many()
        .col_expr(
            proc_run::Column::CancelRequestedAt,
            sea_orm::sea_query::Expr::value(chrono::DateTime::<chrono::FixedOffset>::from(
                Utc::now(),
            )),
        )
        .filter(proc_run::Column::Id.eq(run_uuid))
        .filter(proc_run::Column::Status.eq("running"))
        .exec(&agentic_state.db)
        .await;
    match stamp_res {
        Ok(out) if out.rows_affected == 0 => {
            // Row reached terminal state between status read and
            // stamp. The terminal row is the right answer; nothing
            // more to do.
            return StatusCode::NO_CONTENT.into_response();
        }
        Ok(_) => {}
        Err(e) => {
            warn!(run_id = %run_id, error = %e, "cancel: DB stamp failed");
        }
    }

    // Fast path: abort the in-process spawn. Only works on the
    // instance that started the run. The set_cancelled handler in
    // the task completion path covers the cross-instance case — when
    // an instance without the handle calls cancel, the stamp above
    // is the only effect, and when the automation naturally finishes
    // it observes cancel_requested_at and writes the cancelled row.
    if let Some((_, handle)) = join_handles().remove(&run_id) {
        handle.abort();
        // Spawned task's drop path will write the cancelled row.
        // But the task may not get a chance to run (abort) — write
        // here too so pollers see the state immediately. Idempotent
        // with whatever the task may eventually write because both
        // write 'cancelled' + completed_at.
        if let Err(e) = set_cancelled(run_uuid).update(&agentic_state.db).await {
            warn!(run_id = %run_id, error = %e, "cancel: terminal update failed");
        }
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
/// 2. **Reconcile cross-instance cancels.** A row with
///    `status = 'running'` AND `cancel_requested_at` set means the
///    originating instance saw the stamp but either died before
///    observing it or the cancel was issued on a different
///    instance whose abort is a no-op here. Promote to `cancelled`
///    once the stamp is older than the abort window.
///
/// 3. **Mark stuck-running rows failed.** A row with
///    `status = 'running'` and `started_at` older than 2 hours is
///    almost certainly orphaned (originating instance crashed
///    mid-run). Mark `failed` with a clear code.
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
    //    handled by requiring `cancel_requested_at IS NULL`.
    let stuck_cutoff = Utc::now() - chrono::Duration::hours(2);
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
