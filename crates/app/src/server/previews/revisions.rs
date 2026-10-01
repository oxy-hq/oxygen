//! A preview's staging revisions, released when the preview stops using them.
//!
//! A preview is at one commit (`workspace_previews.git_sha`), and its revision is
//! the staging compile of that commit. When the preview is deleted, or refreshed
//! to a newer commit, the old commit's staging revisions are released at once
//! rather than left to the 30-day retention sweep — a deleted preview's revision
//! must be gone for preview purposes, not merely unlisted.
//!
//! Released means deleted through the retention rule
//! (`compile_maintenance::REVISION_IN_USE`: never a workspace's current revision,
//! never one an app build pins), plus what only previews know keeps one alive:
//!
//! * **another live preview at the same commit** — two branches at one commit
//!   share one staging revision (the staging compile reuses a ready revision of
//!   the SHA), so deleting one preview must not take the other's;
//! * **a preview run that has not finished** reads it — a procedure dry run, a
//!   transform build, a sample or a change check. Deleting the revision under a
//!   running run would fail it mid-flight; a delete has already cancelled the
//!   preview's *queued* runs, so what is left is work in flight. Such a revision
//!   stays for retention to reclaim.
//!
//! Only `kind = 'staging'`: a preview that reused a **main** revision of the same
//! commit never deletes it — main's history belongs to promotion and rollback.
//! Only finished rows: a compile still writing its revision is left to finish.
//! The cascade on `revisions` removes every compiled `*_definitions` row.

use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};
use uuid::Uuid;

use crate::server::compile_maintenance::REVISION_IN_USE;

/// What keeps a staging revision of a preview's commit alive beyond the
/// retention rule. A SQL predicate over `revisions r`.
const PREVIEW_STILL_USES: &str = "(EXISTS ( \
         SELECT 1 FROM workspace_previews p \
         WHERE p.workspace_id = r.workspace_id AND p.git_sha = r.git_sha \
     ) OR EXISTS ( \
         SELECT 1 FROM workspace_preview_runs x \
         WHERE x.revision_id = r.revision_id AND x.state <> 'finished' \
     ))";

/// Delete `workspace_id`'s finished staging revisions of `git_sha` that nothing
/// uses any more. Call it after the preview row stopped naming `git_sha` (deleted,
/// or moved to a newer commit), on the same connection or transaction. Returns
/// how many revisions were deleted. Idempotent.
pub async fn release<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    git_sha: &str,
) -> Result<u64, DbErr> {
    let sql = format!(
        "DELETE FROM revisions r \
         WHERE r.workspace_id = $1 AND r.git_sha = $2 \
           AND r.kind = 'staging' AND r.finished_at IS NOT NULL \
           AND NOT {REVISION_IN_USE} \
           AND NOT {PREVIEW_STILL_USES}"
    );
    let deleted = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [workspace_id.into(), git_sha.into()],
        ))
        .await?
        .rows_affected();
    if deleted > 0 {
        tracing::info!(%workspace_id, %git_sha, deleted,
            "previews: released the staging revisions a preview stopped using");
    }
    Ok(deleted)
}
