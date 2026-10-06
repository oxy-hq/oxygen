//! The compile worker's hand-off to workspace previews: a `Ready` staging
//! compile of a previewed branch queues that revision's Airway change check.
//! Drives the real worker over a real (tiny) workspace; skips when
//! `OXY_DATABASE_URL` is unset, per `test_support::test_db`.

use std::time::Duration;

use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use super::*;
use crate::server::test_support::{SKIP_MSG, test_db};

const BRANCH: &str = "feat/x";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

async fn seed_workspace(db: &DatabaseConnection, root: &std::path::Path) -> Uuid {
    let id = Uuid::new_v4();
    entity::workspaces::ActiveModel {
        id: Set(id),
        name: Set(format!("compile-worker-previews-{id}")),
        path: Set(Some(root.to_string_lossy().into_owned())),
        status: Set(entity::workspaces::WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

fn working_copy() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("workspace dir");
    std::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
        .expect("write config.yml");
    dir
}

/// Run one staging compile of `branch` at [`SHA`] through the worker and wait
/// for its terminal outcome.
async fn compile_staging(db: &DatabaseConnection, ws: Uuid, root: &std::path::Path) {
    let worker = CompileWorker::new(Arc::new(db.clone()));
    let mut task = worker.execute(CompileSpec {
        workspace_id: ws,
        workspace_path: root.to_path_buf(),
        from_git: false,
        git_sha: Some(SHA.into()),
        branch: Some(BRANCH.into()),
        promote: false,
        kind: RevisionKind::Staging,
        owner_user_id: None,
    });
    let outcome = tokio::time::timeout(Duration::from_secs(60), task.outcomes.recv())
        .await
        .expect("the compile finishes")
        .expect("an outcome");
    assert!(
        matches!(outcome, TaskOutcome::Done { .. }),
        "the fixture compiles clean: {outcome:?}"
    );
}

async fn analyze_rows(
    db: &DatabaseConnection,
    ws: Uuid,
) -> Vec<entity::workspace_preview_runs::Model> {
    entity::workspace_preview_runs::Entity::find()
        .filter(entity::workspace_preview_runs::Column::WorkspaceId.eq(ws))
        .all(db)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_ready_staging_compile_with_a_preview_enqueues_analyze() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let dir = working_copy();
    let ws = seed_workspace(&db, dir.path()).await;
    let staff = Uuid::new_v4();
    entity::users::ActiveModel {
        id: Set(staff),
        email: Set(Some(format!("{staff}@oxy.test"))),
        name: Set("Staff".into()),
        picture: Set(None),
        email_verified: Set(true),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed user");
    crate::server::previews::store::upsert(&db, ws, BRANCH, SHA, staff)
        .await
        .expect("preview the branch");

    compile_staging(&db, ws, dir.path()).await;

    let revision = entity::revisions::Entity::find()
        .filter(entity::revisions::Column::WorkspaceId.eq(ws))
        .one(&db)
        .await
        .unwrap()
        .expect("the staging revision");
    assert_eq!(revision.kind, "staging");
    let rows = analyze_rows(&db, ws).await;
    assert_eq!(rows.len(), 1, "one check queued for the previewed revision");
    assert_eq!(rows[0].kind, "analyze");
    assert_eq!(rows[0].revision_id, revision.revision_id);
    assert_eq!(rows[0].branch, BRANCH);
}

/// The control: the same compile of a branch nobody previews queues nothing,
/// so the test above is about the preview row, not about staging compiles.
#[tokio::test]
async fn a_ready_staging_compile_of_an_unpreviewed_branch_queues_no_check() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let dir = working_copy();
    let ws = seed_workspace(&db, dir.path()).await;

    compile_staging(&db, ws, dir.path()).await;

    assert!(analyze_rows(&db, ws).await.is_empty());
}
