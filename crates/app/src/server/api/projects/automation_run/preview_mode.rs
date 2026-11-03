//! `workspace-actions: preview` — an automation run from a non-production
//! custom-app environment executes as a **workspace-preview dry run**
//! (`previews::runs`): reads and agent steps run for real, managed-Airhouse
//! writes land in the environment's own preview schemas, `http_request` is
//! `GET`/`HEAD` only and every other write is held and reported.
//!
//! The environment's `workspace_actions` plane decides (`staging_hold`):
//! `held` answers `409 held_in_staging`, `refused` answers `404`, `preview`
//! lands here. Nothing runs in the handler:
//!
//! 1. [`start_preview_run`] writes, in one transaction, the
//!    `customer_app_procedure_runs` row the bundle polls and the
//!    `workspace_preview_runs` row (same id, `kind = 'procedure'`,
//!    `branch = app-env/<app_id>/<env>`) that the preview queue drives. The
//!    global-run driver then runs it with the preview platform.
//! 2. [`settle_custom_app_runs`], on the preview sweep's tick, copies a
//!    finished run's outcome from `agentic_runs` onto the poll row through the
//!    same [`settle`] helpers every other writer uses, so the first terminal
//!    state wins.
//!
//! **The revision** is the request's effective semantic pin (the build the
//! environment serves, else the environment's own pin — the gate already
//! resolved it as `gate.staging_pin` and refused a pin that is not ready with
//! `503 semantic_branch_not_ready`), else the workspace's promoted revision.
//!
//! **`OXY_PREVIEW_RUNS` off** answers `409 preview_runs_disabled`; a preview
//! run never falls back to a live one.
//!
//! **The orphan reconcile skips these rows** ([`owns_run`]): a preview run
//! that ends `failed` is closed here with its real error, not as
//! `automation_run_orphaned` by a poll that got there first.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use entity::customer_app_procedure_runs::ActiveModel as ProcRunActiveModel;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_app_core::gate_response::{NotReady, pin_refused};
use oxy_auth::types::AuthenticatedUser;
use oxy_custom_apps::gate::CustomAppContext;
use oxy_custom_apps::staging_held::{HeldActor, HeldRow, record_held};
use oxy_custom_apps::staging_pin::request::AppRef;
use oxy_custom_apps::staging_pin::resolve::pin_for_environment;
use oxy_custom_apps::write_record::WriteRecord;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr,
    EntityTrait, Statement, TransactionTrait,
};
use serde_json::{Value as JsonValue, json};
use tracing::{error, warn};
use uuid::Uuid;

use super::{AUTOMATION_SURFACE, err, err_with_code, err_with_hint};

mod cancel;
mod settle;

use crate::server::previews::runs as preview_runs;
pub(super) use cancel::may_cancel;
pub use settle::settle_custom_app_runs;

/// The `mode` an automation run's held row carries when it ran as a preview.
const PREVIEW_MODE: &str = "preview";

/// The app and environment a `preview`-mode request resolved to.
pub(super) struct PreviewTarget {
    pub app: entity::apps::Model,
    pub environment: AppEnvironment,
}

/// The held write an automation run stands for, as the audit shape: never a
/// table, since the run is the unit.
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
        sent: None,
    }
}

/// Record one `app.staging.held` row for an automation run request: `mode` is
/// the surface (`automation`) when it was held, [`PREVIEW_MODE`] when it ran
/// as a preview.
pub(super) async fn record_run_request(
    db: &DatabaseConnection,
    target: &PreviewTarget,
    user: &AuthenticatedUser,
    automation_id: &str,
    mode: &str,
) {
    record_held(
        db,
        HeldRow {
            app_id: target.app.id,
            app_slug: target.app.slug.clone(),
            org_id: target.app.org_id,
            project_id: target.app.project_id,
            environment: target.environment.name(),
            actor: HeldActor::User {
                id: user.id,
                email: user.email.clone(),
            },
            function_or_surface: AUTOMATION_SURFACE.to_string(),
            mode: mode.to_string(),
            request_id: None,
            writes: vec![automation_write(automation_id)],
            invocation_id: None,
            trace_id: None,
            token_id: None,
        },
    )
    .await;
}

/// `409 preview_runs_disabled`: the deployment has not turned preview runs on.
fn preview_runs_disabled() -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "preview_runs_disabled",
            "message": "this environment runs automations as previews, and preview runs are \
                        not enabled on this deployment",
        })),
    )
        .into_response()
}

/// The branch name a custom-app environment's preview runs carry.
fn environment_branch(app_id: Uuid, environment: &AppEnvironment) -> String {
    format!("app-env/{app_id}/{}", environment.name())
}

/// The revision a preview run reads: the effective semantic pin of the app
/// and environment the hold authorized (the build that environment serves,
/// else the environment's own pin), else the workspace's promoted revision.
/// `Ok(None)` when nothing is compiled yet; `Err` is the response to send.
///
/// Resolved here from [`PreviewTarget`], not read from `gate.staging_pin`: the
/// gate names the app by `x-oxy-app` first, the hold by the `Host` first, and
/// the two can disagree. A pin set but not ready is refused
/// (`503 semantic_branch_not_ready`), never routed to production's revision.
async fn revision_to_read(
    db: &DatabaseConnection,
    gate: &CustomAppContext,
    target: &PreviewTarget,
) -> Result<Option<Uuid>, Response> {
    let caller = crate::server::authz::Caller::from_user(&gate.user);
    let app = AppRef::Id(target.app.id);
    let pin = pin_for_environment(db, &caller, gate.project_id, &app, &target.environment)
        .await
        .map_err(|e| {
            pin_refused(NotReady {
                environment: &e.environment,
                git_sha: e.git_sha(),
                fix: &e.fix(),
                planes_unreadable: e.is_planes_unreadable(),
            })
        })?;
    if pin.is_some() {
        return Ok(pin);
    }
    entity::workspaces::Entity::find_by_id(gate.project_id)
        .one(db)
        .await
        .map(|w| w.and_then(|w| w.current_revision_id))
        .map_err(|e| lookup_failed(&e))
}

/// `automation_id`'s file path in `revision_id`, matched by base name as the
/// production start matches it.
async fn automation_path(
    db: &DatabaseConnection,
    revision_id: Uuid,
    automation_id: &str,
) -> Result<Option<String>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT file_path FROM automation_definitions WHERE revision_id = $1 \
             ORDER BY file_path",
            [revision_id.into()],
        ))
        .await?;
    for row in rows {
        let path: String = row.try_get("", "file_path")?;
        if super::matches_automation_id(&path, automation_id) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// `POST …/runs` under `workspace-actions: preview`: resolve the revision and
/// the automation in it, then [`start_preview_run`]. Answers the flag first,
/// so a deployment without preview runs never touches a table.
pub(super) async fn start(
    gate: &CustomAppContext,
    target: PreviewTarget,
    automation_id: &str,
    params: Option<JsonValue>,
) -> Response {
    if !preview_runs::runs_enabled() {
        return preview_runs_disabled();
    }
    let db = &gate.db;
    let revision = match revision_to_read(db, gate, &target).await {
        Ok(Some(r)) => r,
        Ok(None) => return needs_recompile(automation_id),
        Err(response) => return response,
    };
    let path = match automation_path(db, revision, automation_id).await {
        Ok(Some(p)) => p,
        Ok(None) => return not_found(automation_id),
        Err(e) => return lookup_failed(&e),
    };
    let run = NewPreviewRun {
        run_id: Uuid::new_v4(),
        automation_id,
        automation_path: &path,
        revision_id: revision,
        branch: environment_branch(target.app.id, &target.environment),
        params,
    };
    start_preview_run(db, gate, &target, run).await
}

/// One preview run to register, built once from the request.
struct NewPreviewRun<'a> {
    run_id: Uuid,
    automation_id: &'a str,
    /// The automation's file path in `revision_id`.
    automation_path: &'a str,
    revision_id: Uuid,
    branch: String,
    params: Option<JsonValue>,
}

impl NewPreviewRun<'_> {
    /// `workspace_preview_runs.options`: the params as the run's variables,
    /// and which app environment asked.
    fn options(&self, target: &PreviewTarget) -> JsonValue {
        json!({
            "variables": self.params,
            "read_live_only": false,
            "custom_app": { "app_id": target.app.id, "environment": target.environment.name() },
        })
    }
}

/// Register the run: both rows in one transaction, then let the preview queue
/// start it if the workspace is free. `202` with the production start's body
/// plus `"mode": "preview"`.
async fn start_preview_run(
    db: &DatabaseConnection,
    gate: &CustomAppContext,
    target: &PreviewTarget,
    run: NewPreviewRun<'_>,
) -> Response {
    let run_id = run.run_id;
    if let Err(e) = insert_rows(db, gate, target, &run).await {
        error!(%run_id, error = %e, "preview automation run insert failed");
        return err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not register automation run",
        );
    }
    if let Err(e) = preview_runs::advance(db, gate.project_id).await {
        // The row is queued; the sweep's next tick starts it.
        warn!(%run_id, error = %e, "preview automation run queued but not started");
    }
    record_run_request(db, target, &gate.user, run.automation_id, PREVIEW_MODE).await;
    (
        StatusCode::ACCEPTED,
        Json(json!({ "run_id": run_id.to_string(), "mode": PREVIEW_MODE })),
    )
        .into_response()
}

async fn insert_rows(
    db: &DatabaseConnection,
    gate: &CustomAppContext,
    target: &PreviewTarget,
    run: &NewPreviewRun<'_>,
) -> Result<(), DbErr> {
    let txn = db.begin().await?;
    ProcRunActiveModel {
        id: ActiveValue::Set(run.run_id),
        workspace_id: ActiveValue::Set(gate.project_id),
        procedure_id: ActiveValue::Set(run.automation_id.to_string()),
        // `running` is the poll row's open state: there is no `queued` the
        // poll maps, and the preview queue's own state lives on its row.
        status: ActiveValue::Set("running".to_string()),
        params: ActiveValue::Set(run.params.clone()),
        started_at: ActiveValue::Set(chrono::Utc::now().into()),
        ..Default::default()
    }
    .insert(&txn)
    .await?;
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO workspace_preview_runs \
             (run_id, workspace_id, branch, preview_key, revision_id, kind, target_ref, \
              options, state, requested_by) \
         VALUES ($1, $2, $3, $4, $5, 'procedure', $6, $7, 'queued', $8)",
        [
            run.run_id.to_string().into(),
            gate.project_id.into(),
            run.branch.clone().into(),
            crate::server::previews::namespace::preview_key(gate.project_id, &run.branch).into(),
            run.revision_id.into(),
            run.automation_path.into(),
            run.options(target).into(),
            gate.user.id.into(),
        ],
    ))
    .await?;
    txn.commit().await
}

fn not_found(automation_id: &str) -> Response {
    err_with_hint(
        StatusCode::NOT_FOUND,
        format!("automation '{automation_id}' not found"),
        "automation_not_found",
        "Pass the automation's base name without the extension. This environment reads its \
         pinned semantic branch's revision, else the promoted one.",
    )
}

fn needs_recompile(automation_id: &str) -> Response {
    err_with_code(
        StatusCode::SERVICE_UNAVAILABLE,
        format!(
            "automation {automation_id} could not be read from a compiled revision; retry shortly"
        ),
        "automation_needs_recompile",
    )
}

fn lookup_failed(e: &DbErr) -> Response {
    error!(error = %e, "preview automation run lookup failed");
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "could not register automation run",
    )
}

/// Whether a preview run owns `run_id`. The orphan reconcile
/// (`settle::reconcile_abandoned`) leaves such a row to [`settle_custom_app_runs`].
/// A failed lookup says `true`: leave the row, the next poll asks again.
pub(super) async fn owns_run(db: &DatabaseConnection, run_id: Uuid) -> bool {
    match entity::workspace_preview_runs::Entity::find_by_id(run_id.to_string())
        .one(db)
        .await
    {
        Ok(row) => row.is_some(),
        Err(e) => {
            warn!(%run_id, error = %e, "preview ownership lookup failed");
            true
        }
    }
}

/// End the queued preview run of every poll row that is closed `failed` or
/// `cancelled` (`only`: just that run). A request the bundle was told ended
/// must not start later: used by the cancel endpoint and by the stuck-run
/// sweep (`sweep_terminal_runs`). A run already running is stopped by the
/// durable cancel flag the cancel endpoint writes.
pub(super) async fn end_queued_of_closed_polls(db: &DatabaseConnection, only: Option<Uuid>) {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE workspace_preview_runs p SET state = 'finished', finished_at = now() \
             FROM customer_app_automation_runs a \
             WHERE a.id::text = p.run_id AND p.state = 'queued' AND p.kind = 'procedure' \
               AND a.status IN ('failed', 'cancelled') \
               AND ($1::text IS NULL OR p.run_id = $1)",
            [only.map(|id| id.to_string()).into()],
        ))
        .await;
    if let Err(e) = result {
        warn!(error = %e, "could not end the queued preview runs of closed poll rows");
    }
}
