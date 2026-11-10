//! An org has exactly one workspace.
//!
//! [`register_workspace`] is the one insert every creating door goes through
//! (onboarding's new/demo/GitHub, both org-creation doors), so the rule lives
//! there: a second workspace in an org is `409 org_has_workspace`. The GitHub
//! import connects its repository to the org's blank workspace instead of
//! creating one beside it, keeping the workspace id, and only one of two
//! concurrent imports can.
//!
//! Drives the provisioning functions the handlers call against a per-test
//! database.

use std::path::Path;

use entity::organizations;
use entity::workspaces::{self, WorkspaceStatus};
use oxy::github::GitHubRepository;
use oxy_api_tenancy::workspace_provisioning::adopt::{self, ImportTarget};
use oxy_api_tenancy::workspace_provisioning::{
    NewWorkspaceRow, ProvisionError, RepositoryOrigin, register_workspace,
};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};
use crate::workspace_repository_columns::seed_namespace;

async fn seed_org(db: &DatabaseConnection) -> Uuid {
    let org = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("acme".into()),
        slug: ActiveValue::Set(format!("acme-{}", org.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed org");
    org
}

async fn register(
    db: &DatabaseConnection,
    org: Option<Uuid>,
    name: &str,
) -> Result<Uuid, ProvisionError> {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = Uuid::new_v4();
    let row = NewWorkspaceRow {
        name,
        created_by: None,
        org_id: org,
        status: WorkspaceStatus::Ready,
        repository: None,
    };
    register_workspace(db, dir.path(), id, row)
        .await
        .map(|r| r.id)
}

const REMOTE: &str = "https://github.com/acme/analytics.git";

fn origin(namespace: Uuid) -> RepositoryOrigin {
    origin_at(namespace, REMOTE)
}

fn origin_at(namespace: Uuid, clone_url: &str) -> RepositoryOrigin {
    let repo = GitHubRepository {
        id: 7,
        name: "analytics".into(),
        full_name: "acme/analytics".into(),
        default_branch: "main".into(),
        clone_url: clone_url.into(),
    };
    RepositoryOrigin::from_github(namespace, &repo, None)
}

async fn load(db: &DatabaseConnection, id: Uuid) -> workspaces::Model {
    workspaces::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("load workspace")
        .expect("workspace exists")
}

#[tokio::test]
async fn a_second_workspace_in_an_org_is_refused() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let acme = seed_org(&db).await;
    let other = seed_org(&db).await;

    register(&db, Some(acme), "Default").await.expect("first");
    let second = register(&db, Some(acme), "Staging").await.unwrap_err();
    assert!(
        matches!(second, ProvisionError::OrgHasWorkspace),
        "refused as a second workspace, not as a name clash: {second}"
    );
    assert_eq!(second.status_code(), axum::http::StatusCode::CONFLICT);

    // Another org has its own one.
    register(&db, Some(other), "Default")
        .await
        .expect("other org");
}

#[tokio::test]
async fn an_import_connects_the_blank_workspace_and_keeps_its_id() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;
    let namespace = seed_namespace(&db).await;
    let ws = register(&db, Some(org), "Default").await.expect("default");

    assert_eq!(
        adopt::import_target(&db, org).await.unwrap(),
        ImportTarget::Adopt(ws)
    );
    let dir = Path::new("/tmp/oxy-adopt-test");
    let claim = adopt::claim(&db, ws, dir, origin(namespace), None)
        .await
        .expect("claim");
    assert_eq!(claim.workspace_id(), ws);

    let row = load(&db, ws).await;
    assert_eq!(row.name, "Default", "an adopted workspace keeps its name");
    assert_eq!(row.git_namespace_id, Some(namespace));
    assert_eq!(
        row.git_remote_url.as_deref(),
        Some("https://github.com/acme/analytics.git")
    );
    assert_eq!(row.default_branch.as_deref(), Some("main"));
    assert_eq!(row.path.as_deref(), Some("/tmp/oxy-adopt-test"));
    assert_eq!(row.status, WorkspaceStatus::Cloning);

    // Once it has a repository, another import is refused rather than making
    // "Default 2", and so is a racing claim on the same row.
    assert!(matches!(
        adopt::import_target(&db, org).await.unwrap_err(),
        ProvisionError::OrgHasWorkspace
    ));
    assert!(matches!(
        adopt::claim(&db, ws, dir, origin(namespace), None).await,
        Err(ProvisionError::OrgHasWorkspace)
    ));
}

#[tokio::test]
async fn a_released_claim_puts_the_blank_workspace_back() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;
    let namespace = seed_namespace(&db).await;
    let ws = register(&db, Some(org), "Default").await.expect("default");
    let before = load(&db, ws).await;

    let claim = adopt::claim(
        &db,
        ws,
        Path::new("/tmp/elsewhere"),
        origin(namespace),
        Some("Analytics"),
    )
    .await
    .expect("claim");
    assert_eq!(load(&db, ws).await.name, "Analytics");
    adopt::release(&db, claim).await.expect("release");

    let after = load(&db, ws).await;
    assert_eq!(after.name, before.name);
    assert_eq!(after.path, before.path);
    assert_eq!(after.status, before.status);
    assert_eq!(after.git_remote_url, None);
    assert_eq!(after.git_namespace_id, None);
    assert_eq!(
        adopt::import_target(&db, org).await.unwrap(),
        ImportTarget::Adopt(ws),
        "the import can be retried"
    );
}

/// A clone that fails, or finds no Oxygen project, must not leave the org's
/// only workspace refusing every import: the next import takes it over.
#[tokio::test]
async fn an_import_after_a_failed_clone_takes_the_workspace_again() {
    use sea_orm::sea_query::Expr;
    use sea_orm::{ColumnTrait, QueryFilter};

    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;
    let namespace = seed_namespace(&db).await;
    let ws = register(&db, Some(org), "Default").await.expect("default");
    let dir = Path::new("/tmp/oxy-adopt-test");
    adopt::claim(&db, ws, dir, origin(namespace), None)
        .await
        .expect("first claim");

    for failed in [WorkspaceStatus::Failed, WorkspaceStatus::NotOxyProject] {
        // What the background clone writes when it gives up.
        workspaces::Entity::update_many()
            .col_expr(workspaces::Column::Status, Expr::value(failed.clone()))
            .filter(workspaces::Column::Id.eq(ws))
            .exec(&db)
            .await
            .expect("mark failed");

        assert_eq!(
            adopt::import_target(&db, org).await.unwrap(),
            ImportTarget::Adopt(ws),
            "{failed:?}"
        );
        adopt::claim(&db, ws, dir, origin(namespace), None)
            .await
            .expect("a failed import's workspace can be claimed again");
        assert_eq!(load(&db, ws).await.status, WorkspaceStatus::Cloning);
    }
}

/// A clone whose pod died never writes a final status. Once the row has sat
/// in `Cloning` past `STALE_CLONE`, the next import takes it over; before
/// that, a concurrent import is still refused.
#[tokio::test]
async fn an_import_takes_over_a_stale_clone_but_not_a_live_one() {
    use sea_orm::sea_query::Expr;
    use sea_orm::{ColumnTrait, QueryFilter};

    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;
    let namespace = seed_namespace(&db).await;
    let ws = register(&db, Some(org), "Default").await.expect("default");
    let dir = Path::new("/tmp/oxy-adopt-test");
    adopt::claim(&db, ws, dir, origin(namespace), None)
        .await
        .expect("first claim");
    assert!(matches!(
        adopt::claim(&db, ws, dir, origin(namespace), None).await,
        Err(ProvisionError::OrgHasWorkspace)
    ));

    let long_ago =
        chrono::Utc::now().fixed_offset() - adopt::STALE_CLONE - chrono::Duration::minutes(5);
    workspaces::Entity::update_many()
        .col_expr(workspaces::Column::UpdatedAt, Expr::value(long_ago))
        .filter(workspaces::Column::Id.eq(ws))
        .exec(&db)
        .await
        .expect("age the clone");
    assert_eq!(
        adopt::import_target(&db, org).await.unwrap(),
        ImportTarget::Adopt(ws)
    );
    let other = "https://github.com/acme/analytics-v2.git";
    adopt::claim(&db, ws, dir, origin_at(namespace, other), None)
        .await
        .expect("a stale clone can be taken over");

    // The old clone finishes late: its result is dropped, and the newer
    // attempt still settles.
    let old = adopt::settle(&db, ws, REMOTE, WorkspaceStatus::Ready, None)
        .await
        .expect("settle old");
    assert!(!old, "a superseded clone must not write its status");
    assert_eq!(load(&db, ws).await.status, WorkspaceStatus::Cloning);
    let new = adopt::settle(&db, ws, other, WorkspaceStatus::Ready, None)
        .await
        .expect("settle new");
    assert!(new);
    assert_eq!(load(&db, ws).await.status, WorkspaceStatus::Ready);
}

/// A rename or an org transfer moves `updated_at` while the clone runs; the
/// clone's result must still land, or the workspace would sit in `Cloning`
/// with a finished checkout and refuse every import.
#[tokio::test]
async fn a_rename_during_the_clone_does_not_drop_its_result() {
    use sea_orm::sea_query::Expr;
    use sea_orm::{ColumnTrait, QueryFilter};

    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;
    let namespace = seed_namespace(&db).await;
    let ws = register(&db, Some(org), "Default").await.expect("default");
    adopt::claim(
        &db,
        ws,
        Path::new("/tmp/oxy-adopt-test"),
        origin(namespace),
        None,
    )
    .await
    .expect("claim");

    // What the rename handlers write.
    workspaces::Entity::update_many()
        .col_expr(workspaces::Column::Name, Expr::value("Renamed"))
        .col_expr(
            workspaces::Column::UpdatedAt,
            Expr::value(chrono::Utc::now().fixed_offset()),
        )
        .filter(workspaces::Column::Id.eq(ws))
        .exec(&db)
        .await
        .expect("rename");

    assert!(
        adopt::settle(&db, ws, REMOTE, WorkspaceStatus::Ready, None)
            .await
            .expect("settle")
    );
    assert_eq!(load(&db, ws).await.status, WorkspaceStatus::Ready);
}

/// A live clone keeps its row fresh, so it is never taken over however long
/// it runs; only a clone whose pod died goes stale.
#[tokio::test]
async fn a_heartbeat_keeps_a_slow_clone_from_being_taken_over() {
    use sea_orm::sea_query::Expr;
    use sea_orm::{ColumnTrait, QueryFilter};

    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;
    let namespace = seed_namespace(&db).await;
    let ws = register(&db, Some(org), "Default").await.expect("default");
    adopt::claim(
        &db,
        ws,
        Path::new("/tmp/oxy-adopt-test"),
        origin(namespace),
        None,
    )
    .await
    .expect("claim");

    let long_ago =
        chrono::Utc::now().fixed_offset() - adopt::STALE_CLONE - chrono::Duration::minutes(5);
    workspaces::Entity::update_many()
        .col_expr(workspaces::Column::UpdatedAt, Expr::value(long_ago))
        .filter(workspaces::Column::Id.eq(ws))
        .exec(&db)
        .await
        .expect("age the clone");
    adopt::heartbeat(&db, ws, REMOTE).await.expect("heartbeat");

    assert!(matches!(
        adopt::import_target(&db, org).await.unwrap_err(),
        ProvisionError::OrgHasWorkspace
    ));
    assert!(matches!(
        adopt::claim(
            &db,
            ws,
            Path::new("/tmp/oxy-adopt-test"),
            origin(namespace),
            None
        )
        .await,
        Err(ProvisionError::OrgHasWorkspace)
    ));
}
