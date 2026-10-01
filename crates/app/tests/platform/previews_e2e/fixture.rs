//! A workspace as staff find it: a real git repository whose `main` was
//! compiled and promoted by the real compile worker, a branch that edits it,
//! an org admin who is not staff, and one router mounting the previews API and
//! a workspace page the way `build_protected_routes` mounts them — the
//! previews tree beside the workspace tree, behind `workspace_access_middleware`,
//! the page behind the full `workspace_middleware` (where the preview pin and
//! the read-only guard live).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentic_core::delegation::TaskOutcome;
use agentic_pipeline::platform::CompileDispatcher;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::{Router, middleware};
use entity::workspaces::WorkspaceStatus;
use entity::{organizations, workspaces};
use oxy_app::agentic_wiring::compile_dispatcher::OxyCompileDispatcher;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::fakes::FakeClickHouse;
use crate::preview_routes::fixture::{STAFF, common_db, user};

pub(super) const BRANCH: &str = "feat/x";
/// `BRANCH` as a query value.
pub(super) const BRANCH_Q: &str = "feat%2Fx";
/// On the branch only: read, write a warehouse table, POST to a webhook.
pub(super) const PROCEDURE: &str = "workflows/je.procedure.yml";
pub(super) const HOOK: &str = "https://hooks.example.test/journal";
pub(super) const WRITE_SQL: &str = "INSERT INTO analytics.journal SELECT 1";

pub(super) struct Fx {
    pub db: DatabaseConnection,
    pub ws: Uuid,
    pub main_revision: Uuid,
    pub staff: AuthenticatedUser,
    pub customer: AuthenticatedUser,
    pub repo: Repo,
    pub warehouse: FakeClickHouse,
}

pub(super) struct Repo {
    _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub feat_sha: String,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, files: &[(&str, String)]) -> String {
    for (rel, body) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", files[0].0]);
    git(dir, &["rev-parse", "HEAD"])
}

fn clickhouse(name: &str, url: &str) -> String {
    format!(
        "  - name: {name}\n    type: clickhouse\n    host: {url}\n    user: default\n    database: default\n"
    )
}

fn procedure() -> String {
    format!(
        "name: je\ntasks:\n\
         \x20 - name: load_orders\n    type: execute_sql\n    database: warehouse\n    sql_query: SELECT count() AS n FROM analytics.orders\n\
         \x20 - name: post_journal\n    type: execute_sql\n    database: warehouse\n    sql_query: {WRITE_SQL}\n\
         \x20 - name: notify\n    type: http_request\n    method: post\n    url: {HOOK}\n    body: '{{\"posted\": true}}'\n"
    )
}

/// `main` knows one warehouse; `feat/x` adds a second and the procedure.
fn repo(warehouse: &str) -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "a@a"]);
    git(&root, &["config", "user.name", "A"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    let main_config = format!(
        "models: []\ndatabases:\n{}",
        clickhouse("warehouse", warehouse)
    );
    commit(&root, &[("config.yml", main_config.clone())]);
    git(&root, &["checkout", "-qb", BRANCH]);
    let branch_config = format!("{main_config}{}", clickhouse("branch_warehouse", warehouse));
    let mut branch_files = vec![("config.yml", branch_config), (PROCEDURE, procedure())];
    // The data apps and `.sql` file `read_only_exec` renders and runs.
    branch_files.extend(super::read_only_exec::branch_files());
    let feat_sha = commit(&root, &branch_files);
    git(&root, &["checkout", "-q", "main"]);
    Repo {
        _dir: dir,
        root,
        feat_sha,
    }
}

pub(super) async fn setup() -> Fx {
    let db = common_db().await;
    let warehouse = FakeClickHouse::start().await;
    let repo = repo(&warehouse.url);
    let now = chrono::Utc::now().fixed_offset();
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("acme".into()),
        slug: ActiveValue::Set(format!("acme-{}", org.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(&db)
    .await
    .expect("seed org");
    let ws = workspaces::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        name: ActiveValue::Set("ws".into()),
        org_id: ActiveValue::Set(Some(org)),
        path: ActiveValue::Set(Some(repo.root.to_string_lossy().into_owned())),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed workspace")
    .id;
    let staff = user(&db, org, STAFF).await;
    // An org ADMIN: the strongest tenant standing, and still not staff.
    let customer = user(&db, org, "admin@acme.test").await;
    let main_revision = compile_main(&db, ws).await;
    Fx {
        db,
        ws,
        main_revision,
        staff,
        customer,
        repo,
        warehouse,
    }
}

/// Compile `main` and promote it, through the compile worker a queued
/// compile task reaches. The promoted revision.
async fn compile_main(db: &DatabaseConnection, ws: Uuid) -> Uuid {
    let task = OxyCompileDispatcher::new(Arc::new(db.clone()))
        .dispatch(
            ws,
            None,
            Some("main".into()),
            true,
            Some("main".into()),
            None,
        )
        .await
        .expect("dispatch the main compile");
    let mut outcomes = task.outcomes;
    let outcome = outcomes.recv().await.expect("a compile outcome");
    assert!(
        matches!(outcome, TaskOutcome::Done { .. }),
        "main compiles: {outcome:?}"
    );
    workspace(db, ws)
        .await
        .current_revision_id
        .expect("main is promoted")
}

pub(super) async fn workspace(db: &DatabaseConnection, ws: Uuid) -> workspaces::Model {
    workspaces::Entity::find_by_id(ws)
        .one(db)
        .await
        .unwrap()
        .unwrap()
}

/// Process environment variables set for one test, put back as they were when
/// the guard drops.
///
/// Set them **before** [`super::worker::start`], never while the worker runs:
/// `set_var` racing another thread's `getenv` is undefined behaviour. And put
/// them back: outside nextest's process-per-test, a leftover `HTTPS_PROXY`
/// pointing at a dead fake proxy breaks every later HTTPS call in the process.
#[must_use = "the variables are restored when the guard drops"]
pub(super) struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
    /// `Some(value)` sets a variable, `None` removes it.
    pub(super) fn set(vars: &[(&'static str, Option<&str>)]) -> Self {
        let saved = vars
            .iter()
            .map(|(name, _)| (*name, std::env::var_os(name)))
            .collect();
        for (name, value) in vars {
            put(name, value.map(std::ffi::OsStr::new));
        }
        Self(saved)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in self.0.drain(..).rev() {
            put(name, value.as_deref());
        }
    }
}

fn put(name: &str, value: Option<&std::ffi::OsStr>) {
    // SAFETY: nextest runs each test in its own process, and an `EnvGuard` is
    // made before the test starts the worker (see its docs), so no other
    // thread of this test reads the environment while it changes.
    unsafe {
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
}

/// Preview runs on or off for this test, until the guard drops. Before
/// [`super::worker::start`].
pub(super) fn enable_runs(on: bool) -> EnvGuard {
    EnvGuard::set(&[("OXY_PREVIEW_RUNS", on.then_some("1"))])
}

/// The two trees as `build_protected_routes` mounts them, with the real
/// handlers and middleware; `/databases` is the page (a FleetOk read every
/// page load makes) and its POST the write.
fn app(user: AuthenticatedUser) -> Router {
    use oxy_app::server::api::database;
    use oxy_app::server::api::middlewares::workspace_context::{
        workspace_access_middleware, workspace_middleware,
    };
    use oxy_app::server::api::workspace_previews as p;
    let state = oxy_app::server::router::bare_app_state();
    let previews: Router = Router::<oxy_app::server::router::IdeState>::new()
        .route(
            "/",
            post(p::create_preview)
                .get(p::list_previews)
                .delete(p::delete_preview),
        )
        .route("/refresh", post(p::refresh_preview))
        .route("/checks", get(p::get_checks))
        .route("/runs", post(p::start_run).get(p::list_runs))
        .route("/runs/{run_id}", get(p::get_run))
        .route("/sources", get(p::list_sources).put(p::put_source))
        .with_state(oxy_app::server::router::IdeState(state.clone()))
        .layer(middleware::from_fn(workspace_access_middleware));
    // `route_split`: the write needs a working copy (`IdeState`), the list
    // does not (`FleetState`).
    let databases = post(database::create_database_config)
        .with_state(oxy_app::server::router::IdeState(state.clone()))
        .merge(
            get(database::list_databases)
                .with_state(oxy_app::server::router::FleetState(state.clone())),
        );
    let workspace: Router = Router::new()
        .route("/databases", databases)
        .merge(super::read_only_exec::routes(&state))
        .layer(middleware::from_fn_with_state(state, workspace_middleware));
    Router::new()
        .nest("/{workspace_id}", workspace)
        .nest("/{workspace_id}/previews", previews)
        .layer(Extension(user))
}

pub(super) struct Reply {
    pub status: StatusCode,
    /// The `x-oxy-preview` response header.
    pub preview: Option<String>,
    pub body: Value,
}

/// `method uri` as `user`, with an optional JSON body and an optional preview
/// pin (`x-oxy-preview-revision`).
pub(super) async fn call(
    user: &AuthenticatedUser,
    method: &str,
    uri: String,
    body: Option<Value>,
    pin: Option<Uuid>,
) -> Reply {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(rev) = pin {
        req = req.header("x-oxy-preview-revision", rev.to_string());
    }
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let resp = app(user.clone())
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let preview = resp
        .headers()
        .get("x-oxy-preview")
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(resp.into_body(), 256 * 1024)
        .await
        .unwrap();
    Reply {
        status,
        preview,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

pub(super) fn previews(fx: &Fx) -> String {
    format!("/{}/previews", fx.ws)
}

/// The database names a `GET /databases` answered with.
pub(super) fn names(reply: &Reply) -> Vec<String> {
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    reply
        .body
        .as_array()
        .expect("a list")
        .iter()
        .map(|d| d["name"].as_str().unwrap().to_string())
        .collect()
}

/// Poll `GET uri` as staff until `done` holds of the body; the body.
pub(super) async fn eventually(fx: &Fx, uri: String, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        let reply = call(&fx.staff, "GET", uri.clone(), None, None).await;
        if reply.status == StatusCode::OK && done(&reply.body) {
            return reply.body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "GET {uri} never got there: {} {}",
            reply.status,
            reply.body
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Staff create the preview; the worker compiles it and runs its change
/// check. The listed item once both are done.
pub(super) async fn ready_preview(fx: &Fx) -> Value {
    let created = call(
        &fx.staff,
        "POST",
        previews(fx),
        Some(json!({ "branch": BRANCH })),
        None,
    )
    .await;
    assert_eq!(created.status, StatusCode::ACCEPTED, "{}", created.body);
    assert_eq!(
        created.body["item"]["status"], "compiling",
        "{}",
        created.body
    );
    let list = eventually(fx, previews(fx), |b| {
        b["items"][0]["status"] == "ready" && b["items"][0]["checks"]["status"] == "done"
    })
    .await;
    list["items"][0].clone()
}

pub(super) fn revision_of(item: &Value) -> Uuid {
    item["revision_id"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no revision: {item}"))
}
