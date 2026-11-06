//! The one compensation of a publish that fails part way.
//!
//! What it undoes grows with the pipeline: the `apps` row mutation once the
//! row is upserted, the stored bytes once they are written, the build row once
//! it is recorded. A failure at any later stage runs the whole undo in the
//! order the invariants require — the orphan build row first, so a same-id
//! retry does not `409`; then the bytes, and only once the row is gone (best
//! effort; a leaked prefix is a GC concern, not a publish failure), because a
//! row whose bytes were deleted is a phantom build that listings show and
//! retention counts; then the app row, so a live app is never left repointed
//! at a different workspace while still serving the old build. `gc_builds`
//! deletes in the same order. A successful publish disarms it.

use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::{AppMutationRollback, discard_build_row, store};

/// A publish's undo, armed once the app row is touched.
pub(super) struct OrphanGuard<'a> {
    db: &'a DatabaseConnection,
    app_id: Uuid,
    build_id: &'a str,
    rollback: Option<AppMutationRollback>,
    bytes_stored: bool,
    build_pk: Option<Uuid>,
}

impl<'a> OrphanGuard<'a> {
    pub(super) fn armed(
        db: &'a DatabaseConnection,
        app_id: Uuid,
        build_id: &'a str,
        rollback: AppMutationRollback,
    ) -> Self {
        Self {
            db,
            app_id,
            build_id,
            rollback: Some(rollback),
            bytes_stored: false,
            build_pk: None,
        }
    }

    /// The bundle's bytes are in the store under this build's prefix.
    pub(super) fn bytes_stored(&mut self) {
        self.bytes_stored = true;
    }

    /// The build row exists; its functions cascade with it.
    pub(super) fn recorded(&mut self, build_pk: Uuid) {
        self.build_pk = Some(build_pk);
    }

    /// Undo everything armed so far. The errors are swallowed, never
    /// silently: a rollback must not mask the failure that caused it.
    pub(super) async fn undo(mut self) {
        let row_gone = match self.build_pk.take() {
            Some(build_pk) => discard_build_row(self.db, build_pk, self.build_id).await,
            None => true,
        };
        if self.bytes_stored && !row_gone {
            tracing::warn!(
                "publish rollback: kept the bytes of build {:?} for {} because its row survived",
                self.build_id,
                self.app_id
            );
        } else if self.bytes_stored
            && let Err(cleanup) = store::delete_build(self.app_id, self.build_id).await
        {
            tracing::warn!(
                "publish rollback: orphan prefix left for {}: {cleanup}",
                self.app_id
            );
        }
        if let Some(rollback) = self.rollback.take() {
            rollback.apply(self.db, self.app_id).await;
        }
    }

    /// The publish stands: nothing is undone.
    pub(super) fn disarm(self) {}
}
