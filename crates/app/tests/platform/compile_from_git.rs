//! A compile that names a commit, on a process with no working copy.
//!
//! Compile has needed the one pod that holds a checkout, for three reasons
//! stacked on each other: workers decline `compile` at selection, the compile
//! worker refuses a role that owns no files, and the dispatcher refuses a
//! workspace path that is not on disk. A `compile_git` task brings its own
//! tree — the commit, fetched from GitHub — so none of the three applies to it
//! (`internal-docs/factory-retirement.md`, phase 1).
//!
//! Every test here runs as `OXY_ROLE=worker`: the role is initialised for
//! real, so `process_can_compile()` is false and the workspace-path probe is
//! armed. GitHub is a `wiremock` server reached through `GITHUB_API_URL`, and
//! the workspace is connected through a PAT namespace so no GitHub App
//! credentials are involved. `workspaces.path` names a directory that does not
//! exist, exactly as it does on a worker.
//!
//! The queue test drives `recover_pending_global_runs` — what the latency
//! worker calls per workspace — with the policy a worker gets. It does not go
//! through `tick_cloud`, which first builds a context from the workspace's
//! promoted revision and so cannot drive a workspace that has never been
//! compiled; that precondition is recorded in the plan, not exercised here.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(compile_from_git)'`

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use agentic_core::delegation::TaskOutcome;
use agentic_pipeline::platform::{CompileDispatcher, PlatformContext};
use agentic_pipeline::recovery::{DrivePolicy, recover_pending_global_runs};
use agentic_runtime::coordinator::{COMPILE_GIT_SOURCE_TYPE, COMPILE_SOURCE_TYPE};
use agentic_runtime::state::RuntimeState;
use entity::workspaces::WorkspaceStatus;
use entity::{git_namespaces, revisions, semantic_views, users, workspaces};
use flate2::Compression;
use flate2::write::GzEncoder;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::workspace_fs_probe::{leaks, process_owns_workspace_files, reset_leaks};
use oxy_app::agentic_wiring::OxyProjectContext;
use oxy_app::agentic_wiring::compile_dispatcher::OxyCompileDispatcher;
use oxy_app::server::compile_git;
use oxy_app::server::previews::runtime::PreviewRunResolver;
use oxy_app::server::role_manifest::{init_process_role_from_env, process_can_compile};
use oxy_compile::RevisionKind;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, Statement,
};
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{Schema, test_db_with};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_SHA: &str = "89abcdef0123456789abcdef0123456789abcdef";
pub(super) const TOKEN: &str = "ghp_compile_from_git_test";
const CONFIG: &str = "databases: []\nmodels: []\n";
const VIEW: &str = "name: orders\ndatasource: pg\nsql: |\n  SELECT 1 AS label\n\
                    dimensions:\n- name: label\n  type: string\n  expr: label\n\
                    measures:\n- name: n\n  type: count\n";

/// A gzipped commit archive the way GitHub serves one: every path under a
/// single `<owner>-<repo>-<sha>` directory.
pub(super) fn tarball(files: &[(&str, &str)]) -> Vec<u8> {
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

pub(super) fn workspace_tree() -> Vec<u8> {
    tarball(&[
        ("config.yml", CONFIG),
        ("semantics/orders.view.yml", VIEW),
        ("README.md", "not compiled"),
    ])
}

struct Fx {
    db: DatabaseConnection,
    ws: Uuid,
    github: MockServer,
}

impl Fx {
    /// A worker process, a fake GitHub, and a remote-backed workspace whose
    /// directory is not on this node. `repo_subdir` is the stored column.
    async fn new(repo_subdir: Option<&str>) -> Self {
        // `test_db_with` also points the process-wide connection at this
        // database: the token lookup goes through it, not through `db`.
        let db = test_db_with(Schema::All).await;
        let github = MockServer::start().await;
        // SAFETY: nextest runs each test in its own process (`test_db_with`
        // asserts it), and nothing has read these yet.
        unsafe {
            std::env::set_var("OXY_ROLE", "worker");
            std::env::set_var("GITHUB_API_URL", github.uri());
            std::env::remove_var("OXY_COMPILE_BLOB_S3_BUCKET");
        }
        init_process_role_from_env();
        assert!(!process_can_compile(), "the process must be a worker");
        assert!(!process_owns_workspace_files());

        let ws = seed_workspace(&db, repo_subdir).await;
        reset_leaks();
        Self { db, ws, github }
    }

    /// Serve `body` as the archive of `sha`, to a caller carrying the token.
    async fn serve_commit(&self, sha: &str, body: Vec<u8>) {
        Mock::given(method("GET"))
            .and(path(format!("/repos/acme/analytics/tarball/{sha}")))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&self.github)
            .await;
    }

    async fn respond(&self, sha: &str, response: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path(format!("/repos/acme/analytics/tarball/{sha}")))
            .respond_with(response)
            .mount(&self.github)
            .await;
    }

    /// Run one compile the way the executor does once a task is claimed, and
    /// wait for its outcome.
    async fn compile(&self, sha: &str, from_git: bool) -> Result<TaskOutcome, String> {
        let task = OxyCompileDispatcher::new(Arc::new(self.db.clone()))
            .dispatch(
                self.ws,
                Some(sha.to_string()),
                Some("main".into()),
                true,
                Some("main".into()),
                None,
                from_git,
            )
            .await?;
        let mut outcomes = task.outcomes;
        Ok(outcomes.recv().await.expect("a compile outcome"))
    }

    async fn failure(&self, sha: &str) -> String {
        match self.compile(sha, true).await {
            Ok(TaskOutcome::Failed(message)) => message,
            other => panic!("expected the compile to fail, got {other:?}"),
        }
    }

    async fn revisions(&self) -> Vec<revisions::Model> {
        revisions::Entity::find()
            .filter(revisions::Column::WorkspaceId.eq(self.ws))
            .all(&self.db)
            .await
            .expect("read revisions")
    }

    async fn promoted(&self) -> Option<Uuid> {
        workspaces::Entity::find_by_id(self.ws)
            .one(&self.db)
            .await
            .expect("read workspace")
            .expect("workspace exists")
            .current_revision_id
    }
}

pub(super) async fn seed_workspace(db: &DatabaseConnection, repo_subdir: Option<&str>) -> Uuid {
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

async fn views_of(db: &DatabaseConnection, revision: Uuid) -> Vec<String> {
    semantic_views::Entity::find()
        .filter(semantic_views::Column::RevisionId.eq(revision))
        .all(db)
        .await
        .expect("read views")
        .into_iter()
        .map(|v| v.name)
        .collect()
}

/// `(queue_status, run source_type)` of every compile task for the workspace.
async fn tasks(fx: &Fx) -> Vec<(String, String)> {
    let rows = fx
        .db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT q.queue_status, r.source_type FROM agentic_task_queue q \
             JOIN agentic_runs r ON r.id = q.run_id \
             WHERE q.spec->>'type' = 'compile' AND q.spec->>'workspace_id' = $1 \
             ORDER BY r.source_type",
            [fx.ws.to_string().into()],
        ))
        .await
        .expect("read the queue");
    rows.iter()
        .map(|r| {
            (
                r.try_get_by_index(0).unwrap(),
                r.try_get_by_index(1).unwrap(),
            )
        })
        .collect()
}

/// A worker's bound platform. Compile is a Global task, so this is never the
/// workspace being compiled — the dispatcher resolves that from the database.
async fn bound_platform(db: &DatabaseConnection) -> (tempfile::TempDir, Arc<dyn PlatformContext>) {
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

/// Tick the worker's selection-and-drive loop until `done` holds.
async fn drive_queue_until<F, Fut>(fx: &Fx, done: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    if !drive_worker_queue(&fx.db, fx.ws, done).await {
        panic!("the queue did not settle; tasks: {:?}", tasks(fx).await);
    }
}

/// Drive `ws`'s Global runs the way a worker does, until `done` holds.
/// `false` when it never did.
pub(super) async fn drive_worker_queue<F, Fut>(
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
            None,
            None,
            None,
            None,
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

#[tokio::test]
async fn a_worker_claims_a_commit_compile_and_promotes_it() {
    let fx = Fx::new(None).await;
    fx.serve_commit(SHA, workspace_tree()).await;

    compile_git::enqueue(&fx.db, fx.ws, SHA, Some("main"), RevisionKind::Main, true)
        .await
        .expect("enqueue the commit compile");
    assert_eq!(
        tasks(&fx).await,
        [("queued".to_string(), COMPILE_GIT_SOURCE_TYPE.to_string())]
    );
    drive_queue_until(&fx, || async { fx.promoted().await.is_some() }).await;

    let revisions = fx.revisions().await;
    assert_eq!(revisions.len(), 1, "{revisions:?}");
    let revision = &revisions[0];
    assert_eq!(revision.status, "ready");
    assert_eq!(revision.kind, "main");
    assert_eq!(revision.git_sha, SHA);
    assert_eq!(revision.branch.as_deref(), Some("main"));
    assert_eq!(fx.promoted().await, Some(revision.revision_id));
    assert_eq!(views_of(&fx.db, revision.revision_id).await, ["orders"]);
    assert_eq!(
        leaks(),
        0,
        "a commit compile must not reach for a working copy"
    );
}

/// The same workspace, the same worker, the old kind: it is never selected,
/// and would be refused if it were.
#[tokio::test]
async fn a_working_copy_compile_is_still_refused_on_a_worker() {
    let fx = Fx::new(None).await;
    fx.serve_commit(SHA, workspace_tree()).await;

    compile_git::enqueue(&fx.db, fx.ws, SHA, Some("main"), RevisionKind::Main, true)
        .await
        .expect("enqueue the commit compile");
    let working_copy = Uuid::new_v4().to_string();
    agentic_runtime::crud::insert_run(
        &fx.db,
        &working_copy,
        "compile main (local)",
        None,
        COMPILE_SOURCE_TYPE,
        None,
        fx.ws,
    )
    .await
    .expect("insert the working-copy run");
    let spec = agentic_core::delegation::TaskSpec::Compile {
        workspace_id: fx.ws,
        git_sha: None,
        branch: Some("main".into()),
        promote: true,
        kind: Some("main".into()),
        owner_user_id: None,
        from_git: false,
    };
    agentic_runtime::crud::enqueue_task(
        &fx.db,
        &working_copy,
        &working_copy,
        None,
        &spec,
        None,
        agentic_runtime::orchestrator::crud::queue::TaskScope::Global,
    )
    .await
    .expect("enqueue the working-copy compile");

    // Until the queue row is closed, not until the revision is promoted: the
    // promote happens inside the compile, a moment before the worker reports
    // the task done.
    let done = ("completed".to_string(), COMPILE_GIT_SOURCE_TYPE.to_string());
    drive_queue_until(&fx, || async { tasks(&fx).await.contains(&done) }).await;

    // The commit compile ran; the working-copy compile was left for a node
    // that has the files.
    assert_eq!(
        tasks(&fx).await,
        [
            ("queued".to_string(), COMPILE_SOURCE_TYPE.to_string()),
            ("completed".to_string(), COMPILE_GIT_SOURCE_TYPE.to_string()),
        ]
    );
    // And the dispatcher refuses it outright, as before.
    let refused = fx.compile(SHA, false).await.expect_err("no working copy");
    assert!(
        refused.contains("does not exist on this worker"),
        "{refused}"
    );
}

#[tokio::test]
async fn the_same_commit_twice_yields_one_ready_revision() {
    let fx = Fx::new(None).await;
    fx.serve_commit(SHA, workspace_tree()).await;

    for _ in 0..2 {
        let outcome = fx.compile(SHA, true).await.expect("dispatch");
        assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");
    }

    let ready: Vec<_> = fx
        .revisions()
        .await
        .into_iter()
        .filter(|r| r.status == "ready" && r.kind == "main")
        .collect();
    assert_eq!(ready.len(), 1, "{ready:?}");
    assert_eq!(ready[0].git_sha, SHA);
    assert_eq!(fx.promoted().await, Some(ready[0].revision_id));
}

#[tokio::test]
async fn a_workspace_in_a_subdirectory_compiles_that_directory() {
    let fx = Fx::new(Some("data/oxy")).await;
    fx.serve_commit(
        SHA,
        tarball(&[
            (
                "config.yml",
                "this is the repository's, not the workspace's",
            ),
            ("semantics/elsewhere.view.yml", VIEW),
            ("data/oxy/config.yml", CONFIG),
            ("data/oxy/semantics/orders.view.yml", VIEW),
        ]),
    )
    .await;

    let outcome = fx.compile(SHA, true).await.expect("dispatch");
    assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");

    let promoted = fx.promoted().await.expect("promoted");
    assert_eq!(views_of(&fx.db, promoted).await, ["orders"]);
}

#[tokio::test]
async fn a_commit_github_does_not_have_fails_typed_and_promotes_nothing() {
    let fx = Fx::new(None).await;
    fx.respond(OTHER_SHA, ResponseTemplate::new(404)).await;

    let message = fx.failure(OTHER_SHA).await;

    assert!(
        message.starts_with("compile from git failed [commit_not_found]: "),
        "{message}"
    );
    assert!(message.contains(OTHER_SHA) && message.contains("acme/analytics"));
    assert_eq!(fx.revisions().await.len(), 0);
    assert_eq!(fx.promoted().await, None);
    assert_eq!(leaks(), 0);
}

#[tokio::test]
async fn an_outage_is_retryable_and_a_refused_token_is_not() {
    let fx = Fx::new(None).await;
    fx.respond(SHA, ResponseTemplate::new(503)).await;
    let outage = fx.failure(SHA).await;
    assert!(
        outage.starts_with("compile from git failed [github_unavailable, retryable]: "),
        "{outage}"
    );

    let limited = ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0");
    fx.github.reset().await;
    fx.respond(SHA, limited).await;
    let rate_limit = fx.failure(SHA).await;
    assert!(
        rate_limit.starts_with("compile from git failed [github_unavailable, retryable]: "),
        "a rate limit is an outage, not a refusal: {rate_limit}"
    );

    fx.github.reset().await;
    fx.respond(SHA, ResponseTemplate::new(403)).await;
    let denied = fx.failure(SHA).await;
    assert!(
        denied.starts_with("compile from git failed [github_denied]: "),
        "{denied}"
    );
    assert_eq!(fx.promoted().await, None);
}

#[tokio::test]
async fn a_task_that_names_no_commit_never_reaches_github() {
    let fx = Fx::new(None).await;

    for not_a_commit in ["main", "0123abc", "local-6f1c"] {
        let message = fx.failure(not_a_commit).await;
        assert!(
            message.starts_with("compile from git failed [not_a_commit]: "),
            "{message}"
        );
    }
    let requests = fx.github.received_requests().await.unwrap_or_default();
    assert!(requests.is_empty(), "{requests:?}");
}

#[tokio::test]
async fn a_workspace_with_no_remote_or_no_connection_fails_typed() {
    let fx = Fx::new(None).await;
    fx.serve_commit(SHA, workspace_tree()).await;
    let row = workspaces::Entity::find_by_id(fx.ws)
        .one(&fx.db)
        .await
        .unwrap()
        .unwrap();

    let mut unlinked: workspaces::ActiveModel = row.clone().into();
    unlinked.git_namespace_id = ActiveValue::Set(None);
    unlinked.update(&fx.db).await.expect("unlink");
    let no_token = fx.failure(SHA).await;
    assert!(
        no_token.starts_with("compile from git failed [no_token]: "),
        "{no_token}"
    );

    let mut local: workspaces::ActiveModel = row.into();
    local.git_remote_url = ActiveValue::Set(None);
    local.update(&fx.db).await.expect("drop remote");
    let no_remote = fx.failure(SHA).await;
    assert!(
        no_remote.starts_with("compile from git failed [no_remote]: "),
        "{no_remote}"
    );
    assert_eq!(fx.promoted().await, None);
}

/// The repository root has a `config.yml` here, so only the recorded
/// subdirectory can make this fail — and it names a directory the commit
/// does not have.
#[tokio::test]
async fn a_recorded_subdirectory_the_commit_does_not_have_is_refused() {
    let fx = Fx::new(Some("analytics")).await;
    fx.serve_commit(SHA, workspace_tree()).await;

    let message = fx.failure(SHA).await;

    assert!(
        message.starts_with("compile from git failed [subdir_missing]: "),
        "{message}"
    );
    assert_eq!(fx.revisions().await.len(), 0);
}

/// What a workspace in a subdirectory looks like before its row is
/// backfilled: `repo_subdir` is NULL, so the tree root is taken for the
/// workspace, and it has no `config.yml`. Compiling it would promote the
/// repository instead of the workspace.
#[tokio::test]
async fn a_tree_with_no_config_at_the_workspace_root_is_refused() {
    let fx = Fx::new(None).await;
    fx.serve_commit(
        SHA,
        tarball(&[
            ("docs/README.md", "the repository root"),
            ("analytics/config.yml", CONFIG),
            ("analytics/semantics/orders.view.yml", VIEW),
        ]),
    )
    .await;

    let message = fx.failure(SHA).await;

    assert!(
        message.starts_with("compile from git failed [no_config]: "),
        "{message}"
    );
    assert_eq!(fx.revisions().await.len(), 0);
    assert_eq!(fx.promoted().await, None);
}

/// The data is on the Factory's disk and was never committed. Promoting this
/// would leave the database pointing at nothing on every node.
#[tokio::test]
async fn a_duckdb_dataset_that_is_not_in_the_commit_fails_by_name() {
    let fx = Fx::new(None).await;
    let config = "databases:\n  - name: sales\n    type: duckdb\n    dataset: .db\nmodels: []\n";
    fx.serve_commit(SHA, tarball(&[("config.yml", config)]))
        .await;

    let message = fx.failure(SHA).await;

    assert!(
        message.starts_with("compile from git failed [duckdb_data_missing]: "),
        "{message}"
    );
    assert!(message.contains("\"sales\""), "{message}");
    assert_eq!(fx.revisions().await.len(), 0);
    assert_eq!(fx.promoted().await, None);
}

/// GitHub answers the archive request with a redirect to another host.
#[tokio::test]
async fn the_redirect_to_the_archive_host_is_followed() {
    let fx = Fx::new(None).await;
    let location = format!("{}/codeload/{SHA}", fx.github.uri());
    fx.respond(
        SHA,
        ResponseTemplate::new(302).insert_header("location", location.as_str()),
    )
    .await;
    Mock::given(method("GET"))
        .and(path(format!("/codeload/{SHA}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(workspace_tree()))
        .mount(&fx.github)
        .await;

    let outcome = fx.compile(SHA, true).await.expect("dispatch");
    assert!(matches!(outcome, TaskOutcome::Done { .. }), "{outcome:?}");
}
