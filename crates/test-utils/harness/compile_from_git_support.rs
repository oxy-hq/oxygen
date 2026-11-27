//! The commit-compile world `compile_from_git`'s tests build on, shared with
//! the tests that drive a preview's create and refresh through it
//! (`oxy-api-previews`' `previews_from_git`, which includes this file and
//! `compile_request_fixture.rs` by `#[path]`): a GitHub commit archive, a
//! workspace connected through a PAT namespace, and a worker's
//! selection-and-drive loop.

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use agentic_pipeline::platform::PlatformContext;
use agentic_pipeline::recovery::{DrivePolicy, recover_pending_global_runs};
use agentic_runtime::coordinator::COMPILE_SOURCE_TYPE;
use agentic_runtime::state::RuntimeState;
use entity::workspaces::WorkspaceStatus;
use entity::{git_namespaces, users, workspaces};
use flate2::Compression;
use flate2::write::GzEncoder;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::workspace_fs_probe::reset_leaks;
use oxy_workspace_runtime::preview_runtime::PreviewRunResolver;
use oxy_workspace_runtime::project_ctx::OxyProjectContext;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use uuid::Uuid;

pub(crate) const TOKEN: &str = "ghp_compile_from_git_test";

pub(crate) const CONFIG: &str = "databases: []\nmodels: []\n";

pub(crate) const VIEW: &str = "name: orders\ndatasource: pg\nsql: |\n  SELECT 1 AS label\n\
                    dimensions:\n- name: label\n  type: string\n  expr: label\n\
                    measures:\n- name: n\n  type: count\n";

/// A gzipped commit archive the way GitHub serves one: every path under a
/// single `<owner>-<repo>-<sha>` directory.
pub(crate) fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
    let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    for (rel, body) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        tar.append_data(
            &mut header,
            format!("acme-analytics-0123456/{rel}"),
            body.as_bytes(),
        )
        .expect("append file");
    }
    let mut gz = tar.into_inner().expect("finish tar");
    gz.flush().expect("flush gzip");
    gz.finish().expect("finish gzip")
}

pub(crate) fn workspace_tree() -> Vec<u8> {
    tarball(&[
        ("config.yml", CONFIG),
        ("semantics/orders.view.yml", VIEW),
        ("README.md", "not compiled"),
    ])
}

pub(crate) async fn seed_workspace(db: &DatabaseConnection, repo_subdir: Option<&str>) -> Uuid {
    let user = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(user),
        email: ActiveValue::Set(Some(format!("owner-{user}@example.com"))),
        name: ActiveValue::Set("Owner".into()),
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
        installation_id: ActiveValue::Set(0),
        name: ActiveValue::Set("acme".into()),
        owner_type: ActiveValue::Set("User".into()),
        provider: ActiveValue::Set("github".into()),
        // A PAT namespace: its stored token is used as-is.
        slug: ActiveValue::Set("pat".into()),
        oauth_token: ActiveValue::Set(TOKEN.into()),
        created_by: ActiveValue::Set(user),
        org_id: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed namespace");

    let ws = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws),
        name: ActiveValue::Set("From git".into()),
        path: ActiveValue::Set(Some(format!("/nonexistent/workspaces/{ws}"))),
        git_namespace_id: ActiveValue::Set(Some(namespace)),
        git_remote_url: ActiveValue::Set(Some("https://github.com/acme/analytics.git".into())),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        default_branch: ActiveValue::Set(Some("main".into())),
        repo_subdir: ActiveValue::Set(repo_subdir.map(str::to_string)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    ws
}

/// A worker's bound platform. Compile is a Global task, so this is never the
/// workspace being compiled — the dispatcher resolves that from the database.
pub(crate) async fn bound_platform(
    db: &DatabaseConnection,
) -> (tempfile::TempDir, Arc<dyn PlatformContext>) {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(root.path().join("config.yml"), CONFIG).expect("stub config");
    let manager = WorkspaceBuilder::new(Uuid::new_v4())
        .with_working_copy(root.path(), None, oxy::config::OnMissing::Empty)
        .await
        .expect("bound config")
        .build()
        .await
        .expect("bound manager");
    let ctx = OxyProjectContext::new(manager).with_db(Arc::new(db.clone()));
    (root, Arc::new(ctx))
}

/// Drive `ws`'s Global runs the way a worker does, until `done` holds.
/// `false` when it never did.
pub(crate) async fn drive_worker_queue<F, Fut>(
    db: &DatabaseConnection,
    ws: Uuid,
    mut done: F,
) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let (_root, platform) = bound_platform(db).await;
    reset_leaks();
    let state = Arc::new(RuntimeState::new());
    let resolver = PreviewRunResolver::shared(db);
    // What `drive_policy_for(Role::Worker, _)` returns; pinned by its own
    // unit tests, spelled here because that function is private to the router.
    let policy = DrivePolicy::Except(&[COMPILE_SOURCE_TYPE]);
    for _ in 0..200 {
        recover_pending_global_runs(
            db.clone(),
            state.clone(),
            platform.clone(),
            resolver.clone(),
            Arc::new(agentic_runtime::router::NoopTaskRouter),
            Some(ws),
            None,
            policy,
        )
        .await;
        if done().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}
