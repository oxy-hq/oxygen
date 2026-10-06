//! A workspace row records the repository it was imported from — or nothing.
//!
//! The GitHub import is the one door that creates a workspace with a remote,
//! and the only moment the server knows the repository's default branch and
//! the chosen subdirectory without a checkout to ask. It writes both then, so
//! a process with no working copy can answer either later
//! (`internal-docs/factory-retirement.md`, phase 0). Every other door makes a
//! workspace with no remote, and all four repository columns stay NULL.
//!
//! Drives [`register_workspace`] — what `setup_github` calls — against a
//! per-test database, with the origin built the way the handler builds it.

use std::path::Path;

use entity::workspaces::{self, WorkspaceStatus};
use entity::{git_namespaces, users};
use oxy::github::GitHubRepository;
use oxy_api_tenancy::workspace_provisioning::{
    NewWorkspaceRow, RepositoryOrigin, register_workspace,
};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};

const CLONE_URL: &str = "https://github.com/acme/analytics.git";

/// A GitHub App namespace; `workspaces.git_namespace_id` is a real foreign key.
async fn seed_namespace(db: &DatabaseConnection) -> Uuid {
    let user = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(user),
        email: ActiveValue::Set(Some(format!("importer-{user}@example.com"))),
        name: ActiveValue::Set("Importer".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        status: ActiveValue::Set(users::UserStatus::Active),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");

    let namespace = Uuid::new_v4();
    git_namespaces::ActiveModel {
        id: ActiveValue::Set(namespace),
        installation_id: ActiveValue::Set(42),
        name: ActiveValue::Set("acme".into()),
        owner_type: ActiveValue::Set("Organization".into()),
        provider: ActiveValue::Set("github".into()),
        slug: ActiveValue::Set("acme".into()),
        oauth_token: ActiveValue::Set(String::new()),
        created_by: ActiveValue::Set(user),
        org_id: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed namespace");
    namespace
}

async fn register(
    db: &DatabaseConnection,
    repository: Option<RepositoryOrigin>,
) -> workspaces::Model {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = Uuid::new_v4();
    let row = NewWorkspaceRow {
        name: "Imported",
        created_by: None,
        org_id: None,
        status: WorkspaceStatus::Cloning,
        repository,
    };
    register_workspace(db, dir.path(), id, row)
        .await
        .expect("register workspace");
    workspaces::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("load workspace")
        .expect("workspace exists")
}

#[tokio::test]
async fn a_github_import_records_the_default_branch_and_the_subdirectory() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let namespace = seed_namespace(&db).await;
    let repo = GitHubRepository {
        id: 7,
        name: "analytics".into(),
        full_name: "acme/analytics".into(),
        default_branch: "trunk".into(),
        clone_url: CLONE_URL.into(),
    };

    let origin = RepositoryOrigin::from_github(namespace, &repo, Some(Path::new("data/oxy")));
    let row = register(&db, Some(origin)).await;

    assert_eq!(row.git_namespace_id, Some(namespace));
    assert_eq!(row.git_remote_url.as_deref(), Some(CLONE_URL));
    assert_eq!(row.default_branch.as_deref(), Some("trunk"));
    assert_eq!(row.repo_subdir.as_deref(), Some("data/oxy"));
}

#[tokio::test]
async fn a_workspace_with_no_remote_records_none_of_the_four() {
    let (db, _url) = fresh_db(Schema::Central).await;

    let row = register(&db, None).await;

    assert_eq!(row.git_namespace_id, None);
    assert_eq!(row.git_remote_url, None);
    assert_eq!(row.default_branch, None);
    assert_eq!(row.repo_subdir, None);
}
