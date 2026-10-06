//! The previews API end to end: staff only, the contract's shapes, what cannot
//! be previewed is refused, a preview compiles through the call custom-app
//! staging uses (`compile_request::compile`, `kind = 'staging'`, never promoted)
//! and reuses a ready revision of the same commit, and the listing reads its
//! status back off `revisions` and the task queue.
//!
//! Database-backed (`Schema::All`: the compile run and task land in the runtime
//! tables) over a real git repository as the workspace — the Factory's clone,
//! with the branch committed locally and no remote, which is the case the
//! working copy still answers. A branch that is on GitHub is covered by
//! `compile_request` and `previews_from_git`.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Router, middleware};
use entity::users::UserStatus;
use entity::workspaces::WorkspaceStatus;
use entity::{organizations, revisions, users, workspaces};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, Statement,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const BRANCH: &str = "feat/x";
const STAFF: &str = "staff@oxy.test";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, rel: &str, body: &str) -> String {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-qm", rel]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A workspace repository on `main`, with `feat/x` one commit ahead of it.
struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    feat_sha: String,
}

fn repo() -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.email", "a@a"]);
    git(&root, &["config", "user.name", "A"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    commit(&root, "config.yml", "models: []\ndatabases: []\n");
    git(&root, &["checkout", "-qb", BRANCH]);
    let feat_sha = commit(&root, "example_sql/branch_only.sql", "SELECT 1\n");
    git(&root, &["checkout", "-q", "main"]);
    Repo {
        _dir: dir,
        root,
        feat_sha,
    }
}

struct Fx {
    db: DatabaseConnection,
    ws: Uuid,
    staff: AuthenticatedUser,
    customer: AuthenticatedUser,
    repo: Repo,
}

async fn user(db: &DatabaseConnection, org: Uuid, email: &str) -> AuthenticatedUser {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(email.into())),
        name: ActiveValue::Set(format!("Name of {email}")),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    entity::org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(id),
        // An org ADMIN: the strongest tenant standing, and still not staff.
        role: ActiveValue::Set(entity::org_members::OrgRole::Admin),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed membership");
    AuthenticatedUser {
        id,
        email: Some(email.into()),
        name: email.into(),
        picture: None,
        status: UserStatus::Active,
        credential: None,
    }
}

async fn setup() -> Fx {
    let db = crate::common::test_db_with(crate::common::Schema::All).await;
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("OXY_OWNER", STAFF);
    }
    let repo = repo();
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
    let customer = user(&db, org, "admin@acme.test").await;
    Fx {
        db,
        ws,
        staff,
        customer,
        repo,
    }
}

fn api(user: AuthenticatedUser) -> Router {
    use oxy_app::server::api::middlewares::workspace_context::workspace_access_middleware;
    use oxy_app::server::api::workspace_previews as h;
    let previews = Router::new()
        .route(
            "/",
            post(h::create_preview)
                .get(h::list_previews)
                .delete(h::delete_preview),
        )
        .route("/refresh", post(h::refresh_preview))
        .layer(middleware::from_fn(workspace_access_middleware));
    Router::new()
        .nest("/{workspace_id}/previews", previews)
        .layer(Extension(user))
}

async fn send(
    user: &AuthenticatedUser,
    method: &str,
    uri: String,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let resp = api(user.clone())
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn base(fx: &Fx) -> String {
    format!("/{}/previews", fx.ws)
}

async fn create(fx: &Fx, branch: &str) -> (StatusCode, Value) {
    send(
        &fx.staff,
        "POST",
        base(fx),
        Some(json!({ "branch": branch })),
    )
    .await
}

/// The compile specs queued for this workspace, oldest first.
async fn queued_compiles(db: &DatabaseConnection, ws: Uuid) -> Vec<Value> {
    db.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT spec FROM agentic_task_queue \
         WHERE spec->>'type' = 'compile' AND spec->>'workspace_id' = $1 \
         ORDER BY created_at",
        [ws.to_string().into()],
    ))
    .await
    .unwrap()
    .into_iter()
    .map(|r| r.try_get::<Value>("", "spec").unwrap())
    .collect()
}

async fn revision(db: &DatabaseConnection, ws: Uuid, sha: &str, status: &str) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: ActiveValue::Set(id),
        workspace_id: ActiveValue::Set(ws),
        git_sha: ActiveValue::Set(sha.into()),
        branch: ActiveValue::Set(Some(BRANCH.into())),
        schema_version: ActiveValue::Set(oxy_compile::CURRENT_SCHEMA_VERSION),
        status: ActiveValue::Set(status.into()),
        kind: ActiveValue::Set("staging".into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set(oxy_compile::compiler_version()),
        started_at: ActiveValue::Set(now),
        finished_at: ActiveValue::Set(Some(now)),
        file_count_seen: ActiveValue::Set(1),
        file_count_compiled: ActiveValue::Set(1),
        file_count_failed: ActiveValue::Set(0),
        error_summary: ActiveValue::Set(
            (status == "failed").then(|| json!({ "fatal": "views/x.view.yml: bad yaml" })),
        ),
    }
    .insert(db)
    .await
    .expect("seed revision");
    id
}

fn assert_iso_utc(v: &Value) {
    let s = v.as_str().unwrap_or_else(|| panic!("not a timestamp: {v}"));
    assert!(
        s.ends_with('Z') && chrono::DateTime::parse_from_rfc3339(s).is_ok(),
        "ISO-8601 UTC: {s}"
    );
}

// ── Staff only ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn every_verb_is_refused_to_a_customer_even_an_org_admin() {
    let fx = setup().await;
    let b = base(&fx);
    for (method, uri, body) in [
        ("GET", b.clone(), None),
        ("POST", b.clone(), Some(json!({ "branch": BRANCH }))),
        ("POST", format!("{b}/refresh?branch=feat%2Fx"), None),
        ("DELETE", format!("{b}?branch=feat%2Fx"), None),
    ] {
        let (status, _) = send(&fx.customer, method, uri.clone(), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
    assert!(queued_compiles(&fx.db, fx.ws).await.is_empty());
}

// ── Create ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn what_cannot_be_previewed_is_a_400_with_a_reason() {
    let fx = setup().await;
    for (branch, code) in [
        ("main", "default_branch"),
        ("feat/nope", "unknown_branch"),
        ("../etc", "invalid_branch"),
        ("feat--x", "invalid_branch"),
    ] {
        let (status, body) = create(&fx, branch).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{branch}: {body}");
        assert_eq!(body["code"], code, "{branch}: {body}");
        assert!(body["message"].as_str().is_some(), "{branch}");
    }
    assert!(
        queued_compiles(&fx.db, fx.ws).await.is_empty(),
        "a refused request enqueues nothing"
    );
}

#[tokio::test]
async fn creating_queues_one_staging_compile_of_the_head_and_lists_it_compiling() {
    let fx = setup().await;
    let (status, body) = create(&fx, BRANCH).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let item = &body["item"];
    assert_eq!(item["branch"], BRANCH);
    assert_eq!(item["sha"], fx.repo.feat_sha.as_str());
    assert_eq!(item["status"], "compiling");
    assert_eq!(item["revision_id"], Value::Null);
    assert_eq!(item["error"], Value::Null);
    assert_eq!(item["compiled_at"], Value::Null);
    assert_eq!(item["created_by"]["id"], fx.staff.id.to_string());
    assert_eq!(item["created_by"]["name"], format!("Name of {STAFF}"));
    assert_iso_utc(&item["updated_at"]);

    let specs = queued_compiles(&fx.db, fx.ws).await;
    assert_eq!(specs.len(), 1);
    assert_eq!(
        specs[0]["kind"], "staging",
        "custom-app staging's compile path"
    );
    assert_eq!(specs[0]["branch"], BRANCH);
    assert_eq!(specs[0]["git_sha"], fx.repo.feat_sha.as_str());
    assert_ne!(
        specs[0]["promote"],
        Value::Bool(true),
        "a preview never promotes"
    );

    // Idempotent: the same head, still queued, is not compiled twice.
    let (status, _) = create(&fx, BRANCH).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(queued_compiles(&fx.db, fx.ws).await.len(), 1);
}

#[tokio::test]
async fn a_ready_revision_of_the_same_commit_is_reused_not_recompiled() {
    let fx = setup().await;
    let rev = revision(&fx.db, fx.ws, &fx.repo.feat_sha, "ready").await;
    let (status, body) = create(&fx, BRANCH).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["item"]["status"], "ready");
    assert_eq!(body["item"]["revision_id"], rev.to_string());
    assert_iso_utc(&body["item"]["compiled_at"]);
    assert!(queued_compiles(&fx.db, fx.ws).await.is_empty());
    // No compile will land to ask for the reused revision's Airway change
    // check, so the create queued it.
    assert_eq!(
        body["item"]["checks"],
        json!({ "status": "pending", "needs_reset": 0, "warnings": 0, "transforms": 0 })
    );
    let queued = fx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM workspace_preview_runs \
             WHERE workspace_id = $1 AND revision_id = $2 AND kind = 'analyze'",
            [fx.ws.into(), rev.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(queued, 1, "one check queued for the reused revision");
}

#[tokio::test]
async fn uncommitted_edits_on_the_branch_are_a_409_not_a_preview_of_the_wrong_content() {
    let fx = setup().await;
    // The staging compile reads the branch's worktree; dirty it.
    let (status, _) = create(&fx, BRANCH).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let worktree = fx.repo.root.join(".worktrees").join("feat--x");
    std::fs::write(worktree.join("config.yml"), "models: [dirty]\n").unwrap();
    let (status, body) = send(
        &fx.staff,
        "POST",
        format!("{}/refresh?branch=feat%2Fx", base(&fx)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "cannot_compile");
}

// ── List, refresh, delete ─────────────────────────────────────────────────────

#[tokio::test]
async fn the_listing_reads_each_previews_status_off_its_revisions() {
    let fx = setup().await;
    create(&fx, BRANCH).await;
    let list = || async { send(&fx.staff, "GET", base(&fx), None).await.1["items"][0].clone() };

    assert_eq!(
        list().await["status"],
        "compiling",
        "queued, no revision yet"
    );

    // The queued task is claimed and fails before it writes a revision: that
    // is a failure with a reason, not a preview still waiting for something.
    let set_task = |status: &'static str| {
        fx.db.execute_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            format!("UPDATE agentic_task_queue SET queue_status = '{status}'"),
        ))
    };
    set_task("failed").await.unwrap();
    let item = list().await;
    assert_eq!(item["status"], "failed", "{item}");
    assert!(item["error"].is_string(), "{item}");

    // The compile was cancelled, or retention took its revision.
    set_task("cancelled").await.unwrap();
    assert_eq!(
        list().await["status"],
        "stale",
        "no trace: refresh to recompile"
    );

    revision(&fx.db, fx.ws, &fx.repo.feat_sha, "failed").await;
    let item = list().await;
    assert_eq!(item["status"], "failed");
    assert!(
        item["error"].as_str().unwrap().contains("bad yaml"),
        "{item}"
    );

    let rev = revision(&fx.db, fx.ws, &fx.repo.feat_sha, "ready").await;
    let item = list().await;
    assert_eq!(item["status"], "ready");
    assert_eq!(item["revision_id"], rev.to_string());
    assert_eq!(item["error"], Value::Null);
}

#[tokio::test]
async fn refresh_moves_the_preview_to_the_new_head_and_releases_the_old_revision() {
    let fx = setup().await;
    let b = base(&fx);
    let (status, body) = send(
        &fx.staff,
        "POST",
        format!("{b}/refresh?branch=feat%2Fx"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "preview_not_found");

    let old = revision(&fx.db, fx.ws, &fx.repo.feat_sha, "ready").await;
    create(&fx, BRANCH).await;

    // A new commit on the branch, made where the staging compile reads it.
    let worktree = fx.repo.root.join(".worktrees").join("feat--x");
    let newer = commit(&worktree, "example_sql/second.sql", "SELECT 2\n");
    let (status, body) = send(
        &fx.staff,
        "POST",
        format!("{b}/refresh?branch=feat%2Fx"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["item"]["sha"], newer.as_str());
    assert_eq!(body["item"]["status"], "compiling");
    let specs = queued_compiles(&fx.db, fx.ws).await;
    assert_eq!(specs.last().unwrap()["git_sha"], newer.as_str());
    assert!(
        revisions::Entity::find_by_id(old)
            .one(&fx.db)
            .await
            .unwrap()
            .is_none(),
        "a refresh releases the revision it moved off: nothing previews that commit now"
    );
    let unfinished = fx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM workspace_preview_runs \
             WHERE workspace_id = $1 AND revision_id = $2 AND state <> 'finished'",
            [fx.ws.into(), old.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(
        unfinished, 0,
        "the superseded commit's queued change check was cancelled, not left to fail"
    );

    let (status, _) = send(&fx.staff, "DELETE", format!("{b}?branch=feat%2Fx"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = send(&fx.staff, "GET", b.clone(), None).await;
    assert!(body["items"].as_array().unwrap().is_empty());
    let (status, _) = send(&fx.staff, "DELETE", format!("{b}?branch=feat%2Fx"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "idempotent");
}
