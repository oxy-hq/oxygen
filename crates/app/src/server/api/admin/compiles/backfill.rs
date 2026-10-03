//! POST `/admin/compiles/backfill` — compile every workspace that has never
//! been compiled, a bounded batch at a time.

use axum::Json;
use axum::response::Response;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;

use super::{connect, db_err, insert_run_and_enqueue_compile, listing_scope};

/// Max workspaces enqueued per backfill call. Bounds the in-memory load, the
/// per-request wall-clock (sequential enqueues), and the resulting compile
/// herd. The endpoint reports `remaining: true` when more uncompiled
/// workspaces exist; the operator (or UI) re-invokes until it's false.
const BACKFILL_BATCH: u64 = 500;

#[derive(Serialize, Debug)]
pub struct BackfillResponse {
    pub enqueued: usize,
    /// True when the batch cap was hit and more uncompiled workspaces remain —
    /// call again to continue.
    pub remaining: bool,
    pub task_ids: Vec<String>,
}

/// Enqueue a promoting compile for up to `BACKFILL_BATCH` workspaces that have
/// a configured path but no promoted revision (`current_revision_id IS NULL`)
/// — the one-time backfill for projects that predate the compile boundary.
/// Bounded + re-runnable: it only ever targets workspaces still uncompiled, so
/// repeated calls drain the backlog a batch at a time without double-enqueuing
/// (the rows it just promoted drop out of the next query).
///
/// A bounded grant backfills the uncompiled workspaces of the orgs it names, and
/// `remaining` counts only those — it is not told how much of the rest of the
/// deployment is uncompiled.
pub async fn backfill_uncompiled(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<BackfillResponse>, Response> {
    let db = connect().await?;
    let reach = listing_scope(&db, &actor).await?;

    let mut find = entity::workspaces::Entity::find()
        .filter(entity::workspaces::Column::Path.is_not_null())
        .filter(entity::workspaces::Column::CurrentRevisionId.is_null());
    if let Some(orgs) = reach {
        find = find.filter(entity::workspaces::Column::OrgId.is_in(orgs));
    }
    let uncompiled = find
        .limit(BACKFILL_BATCH + 1)
        .all(&db)
        .await
        .map_err(db_err)?;

    // One extra row tells us whether a further batch remains without a second
    // COUNT query.
    let remaining = uncompiled.len() as u64 > BACKFILL_BATCH;

    let mut task_ids = Vec::new();
    for ws in uncompiled.into_iter().take(BACKFILL_BATCH as usize) {
        match insert_run_and_enqueue_compile(&db, ws.id, None, None, true).await {
            Ok(task_id) => task_ids.push(task_id),
            // One bad workspace shouldn't abort the whole backfill — log and
            // keep going so the rest still get queued.
            Err(e) => {
                tracing::error!(?e, workspace_id = %ws.id, "backfill: enqueue failed; skipping");
            }
        }
    }

    Ok(Json(BackfillResponse {
        enqueued: task_ids.len(),
        remaining,
        task_ids,
    }))
}
