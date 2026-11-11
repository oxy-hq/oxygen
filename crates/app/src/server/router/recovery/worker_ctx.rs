//! The driver's per-workspace context, through the [`WorkspaceContextCache`]:
//! tagged with the promoted revision, and able to re-resolve after a promote.
//!
//! Driver code, so it lives with recovery and not in `workspace_cache`, which is
//! base (`tests/routing/base_boundary.rs`) and may not reach into recovery.

use std::sync::Arc;

use crate::agentic_wiring::OxyProjectContext;
use crate::agentic_wiring::project_ctx::revision_rebuild::RevisionRebuilder;
use crate::server::api::middlewares::workspace_context::PreaggCacheCtx;
use crate::server::router::workspace_cache::WorkspaceContextCache;

/// The background driver's context for a workspace, through `cache` — the one
/// way the tick (and the boot-time recovery sweep) obtains it.
///
/// `promoted` is the workspace row's `current_revision_id` as the caller just
/// read it, and it tags the entry: a promote since the entry was built is a
/// miss, so the next tick builds at the new revision instead of serving the old
/// one for up to the cache TTL. The tag is the row's value, NOT the context's
/// own `compiled_revision()`: the last-known-good walk can pin a revision other
/// than `current_revision_id`, and comparing against that would rebuild on
/// every tick for as long as the promoted revision is broken.
///
/// The context carries a [`WorkerRevisionRebuilder`], so a drive that holds it
/// past a promote can still re-resolve (`at_current_promoted_revision`).
#[doc(hidden)]
pub async fn worker_project_ctx(
    cache: &Arc<WorkspaceContextCache>,
    workspace_id: uuid::Uuid,
    path: &str,
    promoted: Option<uuid::Uuid>,
    db: &sea_orm::DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Option<Arc<OxyProjectContext>> {
    cache
        .get_or_build_tagged(workspace_id, promoted, || async {
            let ctx = super::build_cloud_project_ctx_inner(workspace_id, path, db, preagg).await?;
            let rebuilder = Arc::new(WorkerRevisionRebuilder {
                cache: Arc::downgrade(cache),
                workspace_id,
                path: path.to_string(),
                db: db.clone(),
                preagg: preagg.clone(),
            });
            Some(Arc::new(ctx.with_revision_rebuilder(rebuilder)))
        })
        .await
}

/// Rebuilds a driver context at the current promoted revision, through the same
/// cache, so the entry it leaves behind is the one the next tick reuses.
///
/// Holds the cache weakly: the cache holds this context, which holds this, so a
/// strong reference would be a cycle that keeps every entry alive.
struct WorkerRevisionRebuilder {
    cache: std::sync::Weak<WorkspaceContextCache>,
    workspace_id: uuid::Uuid,
    path: String,
    db: sea_orm::DatabaseConnection,
    preagg: PreaggCacheCtx,
}

#[async_trait::async_trait]
impl RevisionRebuilder for WorkerRevisionRebuilder {
    async fn rebuild_unless_at(&self, pinned: uuid::Uuid) -> Option<Arc<OxyProjectContext>> {
        use sea_orm::EntityTrait;
        let cache = self.cache.upgrade()?;
        // One fresh read of the row: it gives both the revision a resolve pins
        // now and the tag the tick will compare against.
        let row = match entity::workspaces::Entity::find_by_id(self.workspace_id)
            .one(&self.db)
            .await
        {
            Ok(Some(row)) => row,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!(
                    target: "recovery",
                    workspace_id = %self.workspace_id,
                    error = %e,
                    "revision re-resolve: workspace lookup failed"
                );
                return None;
            }
        };
        let current = super::driver_revision_for(&row).await?;
        if current == pinned {
            return None;
        }
        worker_project_ctx(
            &cache,
            self.workspace_id,
            &self.path,
            row.current_revision_id,
            &self.db,
            &self.preagg,
        )
        .await
    }
}
