//! Airway's single-flight leases, over HTTP: what holds each pipeline, and the
//! force-release that recovers one whose holder will never let go.
//!
//! - `GET    /{workspace_id}/agentic-airway/leases`
//! - `DELETE /{workspace_id}/agentic-airway/leases/{pipeline_name_b64}?run_id=<id>`
//! - `DELETE /{workspace_id}/agentic-airway/leases/{pipeline_name_b64}?force=true`
//!
//! These were `oxy airway leases` / `release-lease`, which read and deleted the
//! rows with `OXY_DATABASE_URL` from a laptop. They are routes now so the
//! recovery needs a token rather than the production database URL, and `oxyc
//! airway leases | release-lease` is the client (`internal-docs/oxy-cli-to-oxyc.md`).
//!
//! **Keyed by pipeline NAME, not ref.** `airway_pipeline_leases` is keyed by
//! `(workspace_id, pipeline_name)`, the `name:` in the `.airway.yml`, and the
//! listing prints that. Taking a ref would mean resolving the compiled spec to
//! find its name, and a stranded lease is exactly the case where the spec may no
//! longer be served. The name rides as URL-safe base64 (no padding), as
//! `/agentic-workflows/files/{path_b64}` does.
//!
//! **Here, not in `agentic-http`.** Both take `WorkspaceAdmin`, which lives
//! above `agentic-http` (the schedule routes moved to `oxy-api-automations` for
//! the same reason). Mounted beside the `/agentic-airway` nest through the
//! workspace seam, and declared FleetOk in [`crate::route_roles`]: one SELECT
//! and one DELETE on `airway_pipeline_leases`, nothing on disk, nothing left
//! running.
//!
//! **The release never widens by default.** A bare DELETE is refused: the caller
//! names either the run it reviewed (`run_id`, a delete scoped to that run, so a
//! release confirmed after the holder finished cannot free whichever run the next
//! cron tick started) or `force=true` (whatever holds it). When nothing was
//! removed, the answer says why — see [`ReleaseOutcome`].

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use sea_orm::{DatabaseConnection, DbErr};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use agentic_airway::extension::pipeline_lease::Model as Lease;
use agentic_pipeline::airway_run::{
    force_release_airway_lease, list_airway_leases, release_airway_lease_scoped,
};
use oxy_app_core::admin_db::connect;
use oxy_server_authz::role_guards::WorkspaceAdmin;

/// One lease as the routes answer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LeaseView {
    pub pipeline_name: String,
    pub run_id: String,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Past `expires_at`: it no longer blocks anything, because the next acquire
    /// takes it over. Decided on the server's clock, not the caller's, so a
    /// laptop with a skewed clock cannot send someone reaching for a release
    /// they do not need.
    pub lapsed: bool,
}

impl LeaseView {
    fn of(lease: &Lease, now: DateTime<Utc>) -> Self {
        Self {
            pipeline_name: lease.pipeline_name.clone(),
            run_id: lease.run_id.clone(),
            acquired_at: lease.acquired_at,
            expires_at: lease.expires_at,
            lapsed: lease.expires_at <= now,
        }
    }
}

/// Which delete the caller asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseMode {
    /// Only if this run still holds it.
    Scoped { run_id: String },
    /// Whatever holds it.
    Force,
}

/// What a release did, decided after the delete.
///
/// Zero rows removed means three different things, and an operator acts
/// differently on each, so they are distinct answers rather than one error:
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseOutcome {
    /// Rows were deleted; the pipeline can run again.
    Released { removed: u64 },
    /// Nothing held it, before or after: the state the caller wanted. Not an
    /// error, so `release-lease … && restart-something` proceeds.
    AlreadyFree,
    /// The run named in `run_id` released, was retried, and holds the lease
    /// again under the same id (`retry` re-acquires under the original run id
    /// on purpose). It is live: leave it alone.
    ReheldBySameRun,
    /// A different run holds it now. Never silently widened to: the caller has
    /// not been shown this run.
    HeldByAnotherRun(LeaseView),
}

/// The decision after the delete, separate from the I/O so every branch is
/// tested without a database. `holder_now` is read only when nothing was
/// removed.
pub fn classify_release(
    mode: &ReleaseMode,
    removed: u64,
    holder_now: Option<LeaseView>,
) -> ReleaseOutcome {
    if removed > 0 {
        return ReleaseOutcome::Released { removed };
    }
    match (mode, holder_now) {
        (_, None) => ReleaseOutcome::AlreadyFree,
        (ReleaseMode::Scoped { run_id }, Some(next)) if &next.run_id == run_id => {
            ReleaseOutcome::ReheldBySameRun
        }
        (_, Some(next)) => ReleaseOutcome::HeldByAnotherRun(next),
    }
}

/// Every lease in `workspace_id`, newest first, lapsed ones included (a lapsed
/// row is still what the table holds).
pub async fn list(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Vec<LeaseView>, DbErr> {
    Ok(list_airway_leases(db, workspace_id)
        .await?
        .iter()
        .map(|l| LeaseView::of(l, now))
        .collect())
}

/// Delete as `mode` says, then classify. Workspace-scoped on both the delete and
/// the re-read: the nil workspace is never substituted for the one named.
pub async fn release(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_name: &str,
    mode: &ReleaseMode,
    now: DateTime<Utc>,
) -> Result<ReleaseOutcome, DbErr> {
    let removed = match mode {
        ReleaseMode::Scoped { run_id } => {
            release_airway_lease_scoped(db, workspace_id, pipeline_name, run_id).await?
        }
        ReleaseMode::Force => force_release_airway_lease(db, workspace_id, pipeline_name).await?,
    };
    let holder_now = if removed == 0 {
        list(db, workspace_id, now)
            .await?
            .into_iter()
            .find(|l| l.pipeline_name == pipeline_name)
    } else {
        None
    };
    Ok(classify_release(mode, removed, holder_now))
}

#[derive(Debug, Deserialize)]
pub struct ReleaseQuery {
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub force: bool,
}

/// `run_id` xor `force=true`. A bare DELETE is refused rather than read as
/// either: the narrow one would surprise a caller who meant force, and the wide
/// one is the hazard this contract exists to keep opt-in.
pub fn mode_of(q: &ReleaseQuery) -> Result<ReleaseMode, &'static str> {
    match (q.run_id.as_deref().map(str::trim), q.force) {
        (Some(id), false) if !id.is_empty() => Ok(ReleaseMode::Scoped {
            run_id: id.to_string(),
        }),
        (None, true) => Ok(ReleaseMode::Force),
        (Some(_), true) => Err("pass run_id or force=true, not both"),
        _ => Err(
            "name the holder you reviewed (run_id=<id>, released only if it still holds the \
             lease) or pass force=true to release whatever holds it",
        ),
    }
}

/// The path segment back to a pipeline name.
pub fn decode_name(name_b64: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD.decode(name_b64.as_bytes()).ok()?;
    let name = String::from_utf8(bytes).ok()?;
    (!name.trim().is_empty()).then_some(name)
}

fn bad_request(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "bad_request", "message": message })),
    )
        .into_response()
}

fn db_failure(e: DbErr) -> Response {
    tracing::error!(%e, "airway leases: database error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "db_error", "message": "the lease table could not be read" })),
    )
        .into_response()
}

/// The response for an outcome. `200` for the two states the caller wanted
/// (released, already free), `409` for the two where the pipeline is still
/// blocked by a run the caller should leave alone or review first.
pub fn outcome_response(pipeline_name: &str, outcome: ReleaseOutcome) -> Response {
    match outcome {
        ReleaseOutcome::Released { removed } => Json(json!({
            "outcome": "released",
            "pipeline_name": pipeline_name,
            "removed": removed,
        }))
        .into_response(),
        ReleaseOutcome::AlreadyFree => Json(json!({
            "outcome": "already_free",
            "pipeline_name": pipeline_name,
            "removed": 0,
        }))
        .into_response(),
        ReleaseOutcome::ReheldBySameRun => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "lease_reheld_by_same_run",
                "message": "the run released the lease and was retried, so it holds it again \
                            and is live; leave it alone",
                "pipeline_name": pipeline_name,
            })),
        )
            .into_response(),
        ReleaseOutcome::HeldByAnotherRun(holder) => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "lease_held_by_another_run",
                "message": "a different run holds the lease now; review it before releasing",
                "pipeline_name": pipeline_name,
                "holder": holder,
            })),
        )
            .into_response(),
    }
}

/// `GET /{workspace_id}/agentic-airway/leases`.
pub async fn list_leases(_: WorkspaceAdmin, Path(workspace_id): Path<Uuid>) -> Response {
    let db = match connect().await {
        Ok(db) => db,
        Err(resp) => return resp,
    };
    match list(&db, workspace_id, Utc::now()).await {
        Ok(leases) => Json(leases).into_response(),
        Err(e) => db_failure(e),
    }
}

/// `DELETE /{workspace_id}/agentic-airway/leases/{pipeline_name_b64}`.
pub async fn release_lease(
    _: WorkspaceAdmin,
    Path((workspace_id, name_b64)): Path<(Uuid, String)>,
    Query(q): Query<ReleaseQuery>,
) -> Response {
    let Some(pipeline_name) = decode_name(&name_b64) else {
        return bad_request("the path segment is not a URL-safe base64 pipeline name");
    };
    let mode = match mode_of(&q) {
        Ok(mode) => mode,
        Err(message) => return bad_request(message),
    };
    let db = match connect().await {
        Ok(db) => db,
        Err(resp) => return resp,
    };
    match release(&db, workspace_id, &pipeline_name, &mode, Utc::now()).await {
        Ok(outcome) => outcome_response(&pipeline_name, outcome),
        Err(e) => db_failure(e),
    }
}

#[cfg(test)]
#[path = "leases_tests.rs"]
mod tests;
