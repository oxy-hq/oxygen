//! Reading runs back: the workspace listing and one run with everything it
//! has produced.
//!
//! Every read carries its own `workspace_id` filter — `simulation_runs` is
//! keyed by `run_id` alone, so the filter is a correctness invariant rather
//! than an optimisation. Runs must not leak across tenants.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use entity::{simulation_run_fits, simulation_run_periods, simulation_runs};
use sea_orm::{ColumnTrait, EntityTrait, ModelTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::Serialize;
use uuid::Uuid;

use super::super::{ApiError, connect, internal};
use super::limits::RunPage;
use oxy_app::surface::WorkspaceManagerReadOnly;

/// `GET /simulations/runs?limit=&offset=` — this workspace's runs, newest
/// first by enqueue time.
///
/// The `workspace_id` filter is a correctness invariant, not an optimisation:
/// runs must not leak across tenants.
pub async fn list_runs(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Query(page): Query<RunPage>,
) -> Result<Json<Vec<simulation_runs::Model>>, ApiError> {
    list_workspace_runs_page(workspace_manager.workspace_id, page)
        .await
        .map(Json)
}

/// The first page at the default size — what a caller who names only a
/// workspace gets.
pub async fn list_workspace_runs(
    workspace_id: Uuid,
) -> Result<Vec<simulation_runs::Model>, ApiError> {
    list_workspace_runs_page(workspace_id, RunPage::default()).await
}

/// Split from the handler for the same reason as [`super::super::list_worlds`]: the logic is
/// reachable from a test, and the transport layer stays extract-call-serialize.
///
/// Ordered by `queued_at`, which is the order a reader means by "newest":
/// `started_at` moves when a worker claims the run, so a run that waited
/// behind a busy fleet would otherwise jump the list the moment it started.
pub async fn list_workspace_runs_page(
    workspace_id: Uuid,
    page: RunPage,
) -> Result<Vec<simulation_runs::Model>, ApiError> {
    let db = connect().await?;
    let rows = simulation_runs::Entity::find()
        .filter(simulation_runs::Column::WorkspaceId.eq(workspace_id))
        .order_by_desc(simulation_runs::Column::QueuedAt)
        .offset(page.offset())
        .limit(page.limit())
        .all(&db)
        .await
        .map_err(internal("list runs"))?;
    Ok(rows)
}

/// One run, with every period and every scored edge.
#[derive(Debug, Serialize)]
pub struct RunDetail {
    pub run: simulation_runs::Model,
    pub periods: Vec<simulation_run_periods::Model>,
    /// β̂ against β_true, per edge per period. The convergence chart is this
    /// list; nothing is recomputed at render time.
    pub fits: Vec<simulation_run_fits::Model>,
}

/// `GET /simulations/runs/{run_id}` — a run and everything it has produced.
///
/// Readable while the run is still going: periods are persisted as they land,
/// so polling this is how "watch it happen" works before there is a UI.
pub async fn get_run(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path((_workspace_id, run_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<RunDetail>, ApiError> {
    // From the extractor, never from the path segment: the URL is caller-
    // supplied, and scoping a tenant read to a number the caller chose is not
    // scoping it at all.
    read_run(workspace_manager.workspace_id, run_id)
        .await
        .map(Json)
}

pub async fn read_run(workspace_id: Uuid, run_id: Uuid) -> Result<RunDetail, ApiError> {
    let db = connect().await?;
    let run = simulation_runs::Entity::find_by_id(run_id)
        .filter(simulation_runs::Column::WorkspaceId.eq(workspace_id))
        .one(&db)
        .await
        .map_err(internal("load run"))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "no such run".to_string()))?;

    let periods = run
        .find_related(simulation_run_periods::Entity)
        .order_by_asc(simulation_run_periods::Column::Period)
        .all(&db)
        .await
        .map_err(internal("load periods"))?;
    let fits = run
        .find_related(simulation_run_fits::Entity)
        .order_by_asc(simulation_run_fits::Column::Period)
        .all(&db)
        .await
        .map_err(internal("load fits"))?;

    Ok(RunDetail { run, periods, fits })
}
