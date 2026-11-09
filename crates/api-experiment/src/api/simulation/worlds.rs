//! The declared grid: which worlds a revision carries, and where to read one.
//!
//! Boundary first, working copy second — but not decided here. A world is a
//! compiled `simulation_definitions` row on any node that has one, and a file
//! on disk only where a working copy exists, and which of those a read uses is
//! `ConfigManager`'s single `Origin` match rather than a three-arm match
//! repeated at every call site. Both arms read persisted rows or the node's own
//! working copy, which is why these routes stay `FleetOk`.
//!
//! Branch semantics come with the manager: `workspace_middleware` pins the
//! request to one revision and yields none for a non-default branch on a node
//! that HAS a working copy, so the IDE previewing uncommitted edits on a
//! feature branch still reads the files. That is why nothing here takes a
//! `branch` of its own — `?branch=` is read once, by the middleware.

use axum::Json;
use axum::http::StatusCode;
use oxy::config::{ArtifactError, ConfigManager, DiskSlot};
use serde::Serialize;

use super::{ApiError, internal};
use oxy_app::surface::WorkspaceManagerReadOnly;

/// One declared world, as the workspace compiled it.
#[derive(Debug, Serialize)]
pub struct SimulationSummary {
    pub name: String,
    pub file_path: String,
    pub definition: serde_json::Value,
}

/// `GET /simulations` — the grid this revision declares.
pub async fn list_simulations(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
) -> Result<Json<Vec<SimulationSummary>>, ApiError> {
    list_worlds(&workspace_manager.config_manager)
        .await
        .map(Json)
}

/// Split from the handler so it is reachable from a test without standing up
/// a `WorkspaceManager`, and so the transport layer stays what it should be:
/// extract, call, serialize.
pub async fn list_worlds<S: DiskSlot>(
    config_manager: &ConfigManager<S>,
) -> Result<Vec<SimulationSummary>, ApiError> {
    Ok(config_manager
        .list_simulations()
        .await
        .map_err(no_source_to_read)?
        .into_iter()
        .map(|world| SimulationSummary {
            name: world.name,
            file_path: world.file_path,
            definition: world.definition,
        })
        .collect())
}

/// Why a read could not answer, as a status.
///
/// `ArtifactError` already draws the line this needs: a fault in the
/// workspace's own YAML is the workspace's problem and a 500, and everything
/// else is "this node could not find out" and retryable — the boundary query
/// FAILED and there is no working copy to fall through to, the root is not on
/// this node yet, Postgres hiccuped.
///
/// Note what is deliberately not in that list: a boundary that answered. A
/// promoted revision carrying no worlds lists as `[]` behind a 200, because an
/// authoritatively empty grid is an answer. So on the listing path a 503 only
/// ever means "could not look", never "looked and found nothing" — collapsing
/// the two is what would make a replica's "no worlds" indistinguishable from a
/// workspace that declares none.
///
/// [`resolve_world`] is the one place where a boundary MISS still routes here,
/// and that is `ConfigManager`'s policy rather than this module's: on a node
/// holding no working copy an absent compiled row is
/// `ArtifactError::NoSource`, not `Ok(None)`, on the documented grounds that
/// the compile may simply not have promoted yet. On a node that HAS a working
/// copy the same miss is a plain 404. Both are pinned in
/// `crates/app/tests/platform/simulation_routes.rs`.
fn no_source_to_read(e: ArtifactError) -> ApiError {
    if !e.retryable() {
        return internal("read worlds")(e);
    }
    tracing::warn!(error = %e, "declared worlds are not readable here; asking for a retry");
    not_compiled_yet()
}

/// This node could not read the declared worlds, and the reason is transient.
///
/// Retryable, not a 500: either the compile has not promoted yet, or the
/// boundary read failed and this instance holds no working copy to fall
/// through to. Which of those it was is in the `warn!` above, not in the body
/// — the caller's only useful move is the same either way.
fn not_compiled_yet() -> ApiError {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "could not read this workspace's declared worlds here — the compiled \
         revision is not readable and this instance holds no working copy — \
         retry shortly"
            .to_string(),
    )
}

/// Resolve one world's body.
///
/// Shared with [`super::runs`] so the listing and the run agree about what
/// exists: both go through the same manager, so a world listed off the working
/// copy is one the run can resolve.
pub(super) async fn resolve_world<S: DiskSlot>(
    config_manager: &ConfigManager<S>,
    name: &str,
) -> Result<serde_json::Value, ApiError> {
    config_manager
        .simulation_definition(name)
        .await
        .map_err(no_source_to_read)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("no simulation named '{name}' in this workspace"),
            )
        })
}
