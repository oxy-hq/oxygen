//! Reset a pipeline's provisioned schema — or just its cursors.
//!
//! Three small, independently-composable steps the `agentic-pipeline`
//! executor stitches together to wipe one workspace's pipeline back to a clean
//! slate: read the stored schema's table names, drop those tables at the
//! destination, then tombstone the workspace's
//! `airway_workspace_pipeline_state` row (clearing its `PipelineState` cursors
//! and `Schema`). A later run then re-infers a fresh schema from scratch.
//!
//! A fourth step, [`cursors`], does the *non*-destructive half on its own:
//! rewind cursors — all of them, or one resource's — while the stored schema
//! and every landed row stay exactly where they are. It exists because the
//! three above only compose one way round. Clearing the cursors is the last of
//! them, so "re-pull from an earlier `default_start`" was reachable only
//! through "drop everything this pipeline has ever landed", which a pipeline
//! carrying an append-only resource cannot pay. Whether such a rewind is
//! *safe* is [`convergence`]'s question, and it is not the same question for
//! every resource.
//!
//! Every step is scoped to a `workspace_id`, so resetting a pipeline in one
//! workspace leaves a same-named pipeline elsewhere alone.
//!
//! Kept next to [`crate::state_store`] and [`crate::destination_factory`]
//! because all three lean on this crate's view of the airway engine
//! ([`StateStore`], [`airway::destination::Destination`]) plus the SeaORM
//! [`airway_workspace_pipeline_state`](crate::extension::workspace_pipeline_state)
//! entity.

pub mod convergence;
pub mod cursors;

pub use convergence::{
    CursorResetRefusal, CursorScope, NonConvergentReason, NonConvergentTable,
    cursor_reset_refusals, table_converges_on_repull,
};
pub use cursors::{
    ClearedCursors, clear_pipeline_cursors, stored_cursor_state, stored_resource_cursors,
};

use std::sync::Arc;

use airway::state::StateStore;
use sea_orm::sea_query::{Expr, OnConflict};
use sea_orm::{ActiveValue, DatabaseConnection, EntityTrait, ExprTrait};
use uuid::Uuid;

use crate::config::DestinationConfig;
use crate::destination_factory::CredentialProvider;
use crate::error::AirwayError;
use crate::extension::workspace_pipeline_state::{self, Entity as PipelineStateEntity};
use crate::state_store::AirwayPgStateStore;

/// Table names in a pipeline's *stored* schema — the set a reset must drop.
///
/// Loads the pipeline's `airway_pipeline_state` row via
/// [`AirwayPgStateStore`] and returns its schema's table-name keys. A
/// pipeline that never provisioned (no row, so `StateSnapshot::default`'s
/// `schema: None`) yields an empty vec — nothing to drop.
pub async fn stored_schema_table_names(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_name: &str,
) -> Result<Vec<String>, AirwayError> {
    // `AirwayPgStateStore::new` wants an owned `Arc<DatabaseConnection>`;
    // mirror the executor's `Arc::new(self.db.clone())` handoff.
    let store = AirwayPgStateStore::new(Arc::new(db.clone()), workspace_id, pipeline_name);
    let snapshot = store.load().await?;
    Ok(snapshot
        .schema
        .map(|s| s.tables.keys().cloned().collect())
        .unwrap_or_default())
}

/// Drop `tables` at the pipeline's destination.
///
/// Builds the concrete [`Destination`](airway::destination::Destination) from
/// `config` (threading the
/// optional airhouse credential provider so a managed destination re-mints
/// a fresh credential on connect) and issues a single `drop_tables`.
/// No-op-safe when `tables` is empty.
pub async fn drop_destination_tables(
    config: &DestinationConfig,
    provider: Option<Arc<dyn CredentialProvider>>,
    tables: &[String],
) -> Result<(), AirwayError> {
    if tables.is_empty() {
        return Ok(());
    }
    let dest = crate::destination_factory::build_destination(config, provider)?;
    dest.drop_tables(tables).await?;
    Ok(())
}

/// Clear this workspace's stored `PipelineState` (incremental cursors) and
/// `Schema` for the pipeline, so the next run starts from a default snapshot
/// and re-infers the schema.
///
/// Writes a **tombstone** — a row with a default state and no schema — rather
/// than deleting. Deleting would leave the workspace with no row, and the
/// store would then adopt the legacy shared row on the next load and hand back
/// exactly the cursor this reset was meant to discard. Idempotent: resetting
/// twice writes the same tombstone. Only this workspace is affected; a
/// same-named pipeline elsewhere keeps its own state.
///
/// The version bump happens **in SQL** (`version = version + 1`), not by
/// reading the row first. Reset takes no pipeline lease — the route goes
/// straight to `PipelineTaskExecutor::bare` — so a run can save between a read
/// and the write, and the tombstone would land at exactly the version that
/// run's next save still expects, handing it permission to write the discarded
/// cursor straight back.
pub async fn clear_pipeline_state(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_name: &str,
) -> Result<(), AirwayError> {
    let empty_state = serde_json::to_value(airway::state::PipelineState::default())
        .map_err(|e| AirwayError::Other(format!("serialize empty PipelineState: {e}")))?;
    let tombstone = workspace_pipeline_state::ActiveModel {
        workspace_id: ActiveValue::Set(workspace_id),
        pipeline_name: ActiveValue::Set(pipeline_name.to_string()),
        state: ActiveValue::Set(empty_state),
        schema_json: ActiveValue::Set(None),
        // Only the insert path (no row yet) uses this; the conflict path takes
        // the `value` expression below instead.
        version: ActiveValue::Set(0),
        updated_at: ActiveValue::Set(chrono::Utc::now()),
    };
    PipelineStateEntity::insert(tombstone)
        .on_conflict(
            OnConflict::columns([
                workspace_pipeline_state::Column::WorkspaceId,
                workspace_pipeline_state::Column::PipelineName,
            ])
            .update_columns([
                workspace_pipeline_state::Column::State,
                workspace_pipeline_state::Column::SchemaJson,
                workspace_pipeline_state::Column::UpdatedAt,
            ])
            // The stored value plus one, read and written in the same
            // statement. Qualified by table, so it is the EXISTING row's
            // version — `excluded.version` would be the proposed 0 above.
            .value(
                workspace_pipeline_state::Column::Version,
                Expr::col((
                    workspace_pipeline_state::Entity,
                    workspace_pipeline_state::Column::Version,
                ))
                .add(1),
            )
            .to_owned(),
        )
        .exec(db)
        .await
        .map_err(|e| AirwayError::Other(format!("tombstone airway pipeline state: {e}")))?;
    Ok(())
}
