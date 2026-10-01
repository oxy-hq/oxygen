//! `queue_check`: a create or refresh queues the check of a revision that is
//! ready, whether it was ready when staged or became ready by the time the
//! preview row was written. Skips when `OXY_DATABASE_URL` is unset.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::*;
use crate::server::previews::analyze::tests::{seed_revision_at, seed_workspace};
use crate::server::test_support::{SKIP_MSG, test_db};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn staged(ws: Uuid, status: &str, revision_id: Option<Uuid>) -> StagingCompileResponse {
    StagingCompileResponse {
        workspace_id: ws,
        git_sha: SHA.into(),
        status: status.into(),
        revision_id,
        task_id: None,
        error: None,
    }
}

async fn checks_queued(db: &DatabaseConnection, ws: Uuid) -> Vec<Uuid> {
    entity::workspace_preview_runs::Entity::find()
        .filter(entity::workspace_preview_runs::Column::WorkspaceId.eq(ws))
        .filter(entity::workspace_preview_runs::Column::Kind.eq("analyze"))
        .all(db)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.revision_id)
        .collect()
}

#[tokio::test]
async fn a_revision_ready_when_staged_queues_one_check() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    let rev = seed_revision_at(&db, ws, "staging", Uuid::new_v4(), SHA).await;
    let ready = staged(ws, "ready", Some(rev));

    // A create, then a refresh at the same commit.
    queue_check(&db, ws, "feat/x", &ready).await;
    queue_check(&db, ws, "feat/x", &ready).await;

    assert_eq!(checks_queued(&db, ws).await, vec![rev]);
}

#[tokio::test]
async fn a_revision_that_became_ready_after_staging_is_queued() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    // Staged while compiling; the compile landed before the preview row did,
    // so the worker found no preview to queue a check for.
    let compiling = staged(ws, "compiling", None);
    let rev = seed_revision_at(&db, ws, "staging", Uuid::new_v4(), SHA).await;

    queue_check(&db, ws, "feat/x", &compiling).await;

    assert_eq!(checks_queued(&db, ws).await, vec![rev]);
}

/// The control: nothing ready at that commit yet, nothing queued — the compile
/// worker asks when it lands.
#[tokio::test]
async fn a_revision_still_compiling_queues_nothing() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;

    queue_check(&db, ws, "feat/x", &staged(ws, "pending", None)).await;

    assert!(checks_queued(&db, ws).await.is_empty());
}
