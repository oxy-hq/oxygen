//! Rewind a pipeline's cursors **without dropping what it has landed**.
//!
//! The fourth composable step, next to [the three](super) a schema reset
//! stitches together. Those exist to wipe a pipeline back to a clean slate;
//! this one exists because the clean slate is sometimes exactly what you cannot
//! afford.
//!
//! The gap it closes: `airway_workspace_pipeline_state` holds a pipeline's
//! cursors and its schema in one row, and the only sanctioned way to clear the
//! cursors was [`clear_pipeline_state`](super::clear_pipeline_state), which the
//! executor only ever calls *after* dropping the destination tables. So
//! "re-pull from an earlier `default_start`" and "destroy every row this
//! pipeline has ever landed" were the same button. On a pipeline whose
//! resources are not equally re-fetchable that is not a trade-off, it is a
//! refusal: one append-only resource holding history the source no longer
//! serves makes the whole pipeline un-rewindable.
//!
//! Two things make this cheap rather than a migration. The cursors are
//! **already per-resource** — `PipelineState::resource_states` is a
//! `HashMap<String, ResourceState>` keyed by the raw resource name — so
//! narrowing a reset to one resource is deleting one key from a JSON object.
//! And the safety oracle is already in the same row: the stored `Schema` says
//! per table how the destination writes it, which is what decides whether a
//! re-pull converges or duplicates. See [`super::convergence`].
//!
//! ## Two details that are load-bearing
//!
//! **The edit happens in SQL.** `state = state #- '{resource_states,<name>}'`
//! and `version = version + 1` in one statement, never read-modify-write. Same
//! reasoning [`clear_pipeline_state`](super::clear_pipeline_state) documents
//! for its own in-SQL bump: this primitive takes no pipeline lease, so a run
//! can save between a read and a write, and a read-modify-write would either
//! clobber that run's cursor or hand it permission to write the discarded one
//! back.
//!
//! The bump is what makes the reset stick: a run still holding the pre-reset
//! version fails `save`'s `WHERE version = $expected` and writes nothing. That
//! protects the **reset** from being undone. It does not protect the **run** —
//! it is the mechanism of the harm to it. `Pipeline::persist` is best-effort,
//! so the losing run drops *every* resource's advanced cursor, out-of-scope
//! append-only ones included, and its next run re-pulls those windows as
//! duplicate rows (`state_store.rs`). What prevents that is not landing
//! mid-run at all: the executor's `reset_airway_cursors` holds the pipeline
//! lease around this call. A pipeline with `allow_concurrent_runs: true` takes
//! no lease, so for it nothing does — a reset there is **unguarded**, and the
//! executor logs a warning saying so.
//!
//! **An absent row is not an absent cursor.** `airway_pipeline_state` (name-
//! keyed, legacy) is adopted into this workspace's row the first time the
//! workspace loads. So an `UPDATE` that matches no row leaves a pipeline that
//! *will* resume from the legacy cursor looking successfully reset. Every entry
//! point here that writes goes through [`AirwayPgStateStore::load`], which
//! performs that adoption, first. The one that only reads —
//! [`stored_resource_cursors`] — must not: see its docs.

use std::sync::Arc;

use airway::state::{PipelineState, StateSnapshot, StateStore};
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Statement, Value,
};
use uuid::Uuid;

use crate::error::AirwayError;
use crate::extension::pipeline_state::Entity as LegacyPipelineStateEntity;
use crate::extension::workspace_pipeline_state::Entity as PipelineStateEntity;
use crate::state_store::AirwayPgStateStore;

use super::convergence::CursorScope;

/// What a cursor reset actually did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClearedCursors {
    /// Resources whose cursor was removed. Sorted, so a caller can render it
    /// without reshuffling between calls over a `HashMap`.
    pub cleared: Vec<String>,
    /// Resources the caller named that held no cursor. Not an error — a
    /// resource that never ran has nothing to rewind — but worth returning,
    /// because a typo'd resource name is indistinguishable from a
    /// never-run one at the call site and this is where it surfaces.
    pub not_held: Vec<String>,
}

/// This workspace's state snapshot for `pipeline_name`, adopting the legacy
/// name-keyed row if this workspace has none yet.
///
/// The adoption is the point: it is how a later load would get its cursor, so
/// judging or clearing without it reasons about state the pipeline will not
/// actually use.
pub async fn stored_cursor_state(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_name: &str,
) -> Result<StateSnapshot, AirwayError> {
    let store = AirwayPgStateStore::new(Arc::new(db.clone()), workspace_id, pipeline_name);
    Ok(store.load().await?)
}

/// Resources holding a cursor for this pipeline, sorted — the names a
/// [`CursorScope::Resources`] may name. **Read-only: never adopts.**
///
/// It reports the same cursors a load would resume from — this workspace's
/// row if it has one (a tombstone included, which holds none), else the legacy
/// name-keyed row — but reads the legacy row where it lies instead of copying
/// it. [`stored_cursor_state`] copies, which the writers need and a listing
/// must not do: it serves a picker an operator opens just to look, and
/// adopting mid-deploy is the store's one documented cost — an old pod that
/// runs afterwards advances the legacy row, that progress never reaches the
/// adopted one, and the window is re-read as duplicate rows. Opening a dialog
/// must not be what spends that.
pub async fn stored_resource_cursors(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_name: &str,
) -> Result<Vec<String>, AirwayError> {
    let own = PipelineStateEntity::find_by_id((workspace_id, pipeline_name.to_string()))
        .one(db)
        .await
        .map_err(|e| AirwayError::Other(format!("read airway pipeline state: {e}")))?;
    let state_json = match own {
        Some(row) => Some(row.state),
        None => LegacyPipelineStateEntity::find_by_id(pipeline_name.to_string())
            .one(db)
            .await
            .map_err(|e| AirwayError::Other(format!("read legacy airway pipeline state: {e}")))?
            .map(|legacy| legacy.state),
    };
    let Some(state_json) = state_json else {
        return Ok(Vec::new());
    };
    let state: PipelineState = serde_json::from_value(state_json)
        .map_err(|e| AirwayError::Other(format!("deserialize PipelineState: {e}")))?;
    let mut names: Vec<String> = state.resource_states.into_keys().collect();
    names.sort();
    Ok(names)
}

/// Clear incremental cursors for `scope`, **leaving the stored schema and the
/// destination tables untouched**.
///
/// The next run re-pulls those resources from their configured
/// `default_start`; every other resource resumes where it was, and nothing is
/// dropped. Whether that re-pull converges is
/// [`convergence`](super::convergence)'s question, deliberately not asked here
/// — this is the primitive, and the caller that owns the refusal is the one
/// that owns the error mapping. `reset_airway_cursors` on the executor is that
/// caller.
///
/// Idempotent: clearing a cursor that is already absent removes nothing and
/// reports it under [`ClearedCursors::not_held`].
pub async fn clear_pipeline_cursors(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    pipeline_name: &str,
    scope: &CursorScope,
) -> Result<ClearedCursors, AirwayError> {
    // Adopt-then-write. See the module docs: skipping this makes a reset that
    // matches no row look successful while the legacy cursor survives.
    let snapshot = stored_cursor_state(db, workspace_id, pipeline_name).await?;
    let held: Vec<String> = snapshot.state.resource_states.keys().cloned().collect();

    let (mut cleared, mut not_held) = match scope {
        CursorScope::AllResources => (held.clone(), Vec::new()),
        CursorScope::Resources(names) => {
            let (c, n): (Vec<String>, Vec<String>) =
                names.iter().cloned().partition(|r| held.contains(r));
            (c, n)
        }
    };
    cleared.sort();
    cleared.dedup();
    not_held.sort();
    not_held.dedup();

    if cleared.is_empty() {
        // Nothing to remove. Deliberately NOT a version bump: bumping would
        // invalidate a concurrently-running load's `expected_version` and make
        // it drop a cursor it legitimately advanced — a cost with no
        // corresponding effect.
        return Ok(ClearedCursors { cleared, not_held });
    }

    let statement = match scope {
        // Replace the whole map rather than chaining a `#-` per name: "all"
        // must also drop a key no resource advertises any more, which a list
        // built from `held` would happen to cover and a list built from a spec
        // would not. `create_missing` (the 4th arg, default true) makes this
        // safe on a state that predates the field.
        CursorScope::AllResources => Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE airway_workspace_pipeline_state \
             SET state = jsonb_set(state, '{resource_states}', '{}'::jsonb, true), \
                 version = version + 1, \
                 updated_at = $3 \
             WHERE workspace_id = $1 AND pipeline_name = $2",
            [
                workspace_id.into(),
                pipeline_name.into(),
                chrono::Utc::now().into(),
            ],
        ),
        CursorScope::Resources(_) => {
            // One `#-` per resource, each path a bound parameter — the resource
            // name reaches Postgres as a value, never as SQL text.
            let mut params: Vec<Value> = vec![
                workspace_id.into(),
                pipeline_name.into(),
                chrono::Utc::now().into(),
            ];
            let mut expr = String::from("state");
            for name in &cleared {
                params.push(name.clone().into());
                expr.push_str(&format!(" #- ARRAY['resource_states', ${}]", params.len()));
            }
            Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                format!(
                    "UPDATE airway_workspace_pipeline_state \
                     SET state = {expr}, version = version + 1, updated_at = $3 \
                     WHERE workspace_id = $1 AND pipeline_name = $2"
                ),
                params,
            )
        }
    };

    let result = db
        .execute_raw(statement)
        .await
        .map_err(|e| AirwayError::Other(format!("clear airway pipeline cursors: {e}")))?;

    if result.rows_affected() == 0 {
        // `stored_cursor_state` reported cursors, so a row was there (or was
        // just adopted) a moment ago. Zero rows means it vanished between the
        // two statements — a concurrent reset, or a workspace teardown. Report
        // it rather than claim a reset that did not land.
        return Err(AirwayError::Other(format!(
            "clear airway pipeline cursors: no `{pipeline_name}` state row for workspace \
             {workspace_id} at write time, though it held cursors at read time — the row \
             was removed concurrently and the cursors were NOT cleared"
        )));
    }

    Ok(ClearedCursors { cleared, not_held })
}
