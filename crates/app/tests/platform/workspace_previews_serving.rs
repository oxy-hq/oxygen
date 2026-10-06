//! Serving a workspace preview: who gets pinned to it, what a pinned request may
//! do, and that nothing outside a request ever reads one.
//!
//! Drives the real `workspace_middleware` over a per-test database, as a
//! DISKLESS process (a serve replica): the only place today's behaviour for
//! `?branch=` is "the promoted revision, and the dropped-hint counter ticks".
//! A probe handler reports which revision the request read by resolving a
//! compiled semantic view whose body differs between the promoted revision
//! (`main`) and the staging revision a preview pins (`staging`).

use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use entity::users::UserStatus;
use entity::workspaces::WorkspaceStatus;
use entity::{organizations, revisions, semantic_views, users, workspaces};
use oxy_app::server::api::compiled_reader::{branch_hints_dropped, resolve_semantic_view};
use oxy_app::server::api::custom_apps_staging_pin::with_staging_pin;
use oxy_app::server::api::middlewares::workspace_context::workspace_middleware;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const VIEW: &str = "semantics/views/probe.view.yml";
const BRANCH: &str = "feat/x";
const STAFF: &str = "staff@oxy.test";
const CUSTOMER: &str = "customer@acme.test";

struct Fx {
    org: Uuid,
    ws: Uuid,
    /// Ready, `kind = 'staging'`, compiled from `feat/x` — the commit the live
    /// preview of `feat/x` is at.
    staging_rev: Uuid,
    /// Still compiling.
    compiling_rev: Uuid,
    /// A ready staging revision of ANOTHER workspace.
    foreign_rev: Uuid,
    /// A ready `draft` revision — not a kind a preview may pin.
    draft_rev: Uuid,
    staff: AuthenticatedUser,
    customer: AuthenticatedUser,
}

async fn setup() -> (DatabaseConnection, Fx) {
    let db = crate::common::test_db().await;
    // SAFETY: nextest runs each test in its own process; set before any request.
    unsafe {
        std::env::set_var("OXY_OWNER", STAFF);
    }
    // A serve replica: no working copy, so `?branch=` cannot be honoured from disk.
    oxy::workspace_fs_probe::set_process_owns_workspace_files(false);
    let fx = seed(&db).await;
    (db, fx)
}

async fn user(db: &DatabaseConnection, org: Uuid, email: &str) -> AuthenticatedUser {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(email.into())),
        name: ActiveValue::Set(email.into()),
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
        role: ActiveValue::Set(entity::org_members::OrgRole::Member),
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

async fn revision(
    db: &DatabaseConnection,
    ws: Uuid,
    kind: &str,
    status: &str,
    which: &str,
) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: ActiveValue::Set(id),
        workspace_id: ActiveValue::Set(ws),
        git_sha: ActiveValue::Set(format!("{:0>40}", id.simple().to_string())),
        branch: ActiveValue::Set(Some(if kind == "main" { "main" } else { BRANCH }.into())),
        schema_version: ActiveValue::Set(1),
        status: ActiveValue::Set(status.into()),
        kind: ActiveValue::Set(kind.into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set("test".into()),
        started_at: ActiveValue::Set(now),
        finished_at: ActiveValue::Set(Some(now)),
        file_count_seen: ActiveValue::Set(1),
        file_count_compiled: ActiveValue::Set(1),
        file_count_failed: ActiveValue::Set(0),
        error_summary: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");
    semantic_views::ActiveModel {
        revision_id: ActiveValue::Set(id),
        name: ActiveValue::Set("probe".into()),
        file_path: ActiveValue::Set(VIEW.into()),
        definition: ActiveValue::Set(json!({ "which": which })),
        compiled_sql_blob_key: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed view");
    id
}

async fn workspace(db: &DatabaseConnection, org: Uuid) -> Uuid {
    let ws = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws),
        name: ActiveValue::Set("ws".into()),
        org_id: ActiveValue::Set(Some(org)),
        // No path: the middleware authorizes and pins, and builds no manager.
        path: ActiveValue::Set(None),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    ws
}

async fn seed(db: &DatabaseConnection) -> Fx {
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
    .insert(db)
    .await
    .expect("seed org");
    let ws = workspace(db, org).await;
    let main_rev = revision(db, ws, "main", "ready", "main").await;
    let mut row: workspaces::ActiveModel = workspaces::Entity::find_by_id(ws)
        .one(db)
        .await
        .unwrap()
        .unwrap()
        .into();
    row.current_revision_id = ActiveValue::Set(Some(main_rev));
    row.update(db).await.expect("promote main");

    let other_ws = workspace(db, org).await;
    let fx = Fx {
        org,
        ws,
        staging_rev: revision(db, ws, "staging", "ready", "staging").await,
        compiling_rev: revision(db, ws, "staging", "compiling", "staging").await,
        foreign_rev: revision(db, other_ws, "staging", "ready", "foreign").await,
        draft_rev: revision(db, ws, "draft", "ready", "draft").await,
        staff: user(db, org, STAFF).await,
        customer: user(db, org, CUSTOMER).await,
    };
    // Every revision is at a commit a live preview is at, so each refusal below
    // is the revision's own, never "no preview names it" (that is `release`).
    preview(db, ws, BRANCH, fx.staging_rev, fx.staff.id).await;
    preview(db, ws, "feat/compiling", fx.compiling_rev, fx.staff.id).await;
    preview(db, other_ws, BRANCH, fx.foreign_rev, fx.staff.id).await;
    preview(db, ws, "feat/draft", fx.draft_rev, fx.staff.id).await;
    fx
}

/// Register a preview of `branch` at the commit `rev` was compiled from.
async fn preview(db: &DatabaseConnection, ws: Uuid, branch: &str, rev: Uuid, by: Uuid) {
    let sha = revisions::Entity::find_by_id(rev)
        .one(db)
        .await
        .unwrap()
        .expect("revision")
        .git_sha;
    oxy_app::server::previews::store::upsert(db, ws, branch, &sha, by)
        .await
        .expect("register preview");
}

/// Which revision this request read: `main`, `staging`, or `none`.
async fn which(ws: Uuid) -> String {
    match resolve_semantic_view(ws, None, VIEW).await {
        Ok(Some(v)) => v.definition["which"].as_str().unwrap_or("?").to_string(),
        _ => "none".to_string(),
    }
}

async fn probe(Extension(ws): Extension<entity::workspaces::Model>) -> Json<Value> {
    Json(json!({ "which": which(ws.id).await }))
}

/// Starts work the way a handler hands something to the background, and
/// reports what THAT work read.
async fn spawn_probe(Extension(ws): Extension<entity::workspaces::Model>) -> Json<Value> {
    let bg = tokio::spawn(async move { which(ws.id).await })
        .await
        .unwrap();
    Json(json!({ "which": which(ws.id).await, "background": bg }))
}

static ACTIONS_RUN: AtomicUsize = AtomicUsize::new(0);

async fn action() -> Json<Value> {
    ACTIONS_RUN.fetch_add(1, Ordering::SeqCst);
    Json(json!({ "ran": true }))
}

fn app(user: AuthenticatedUser) -> Router {
    let state = oxy_app::server::router::bare_app_state();
    let inner = Router::new()
        .route("/probe", get(probe))
        .route("/spawn-probe", get(spawn_probe))
        .route("/agentic-schedules", post(action))
        .route("/files/{path}", post(action))
        .route("/sql/query", post(action))
        .layer(middleware::from_fn_with_state(state, workspace_middleware));
    Router::new()
        .nest("/{workspace_id}", inner)
        .layer(Extension(user))
}

struct Reply {
    status: StatusCode,
    preview_header: Option<String>,
    body: Value,
}

async fn call(
    user: &AuthenticatedUser,
    method: &str,
    uri: String,
    revision: Option<String>,
) -> Reply {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(h) = revision {
        req = req.header("x-oxy-preview-revision", h);
    }
    let resp = app(user.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let preview_header = resp
        .headers()
        .get("x-oxy-preview")
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Reply {
        status,
        preview_header,
        body,
    }
}

fn uri(fx: &Fx, route: &str) -> String {
    format!("/{}/{route}?branch={}", fx.ws, urlencoding::encode(BRANCH))
}

// ── Header + staff + ready revision of this workspace → pinned ───────────────

#[tokio::test]
async fn a_staff_preview_request_reads_the_pinned_revision() {
    let (_db, fx) = setup().await;
    let before = branch_hints_dropped();

    let r = call(
        &fx.staff,
        "GET",
        uri(&fx, "probe"),
        Some(fx.staging_rev.to_string()),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["which"], "staging");
    assert_eq!(
        r.preview_header.as_deref(),
        Some(format!("{BRANCH}@{}", fx.staging_rev).as_str()),
        "a response served from a preview says so, and which revision"
    );
    assert_eq!(
        branch_hints_dropped(),
        before,
        "a pinned preview is not a dropped branch hint"
    );
}

// ── No header → exactly today's behaviour ─────────────────────────────────────

#[tokio::test]
async fn without_the_header_a_branch_request_is_served_as_today() {
    let (_db, fx) = setup().await;
    let before = branch_hints_dropped();

    // The IDE on a previewed branch, from staff: never pinned, never stamped.
    let r = call(&fx.staff, "GET", uri(&fx, "probe"), None).await;
    assert_eq!(r.body["which"], "main");
    assert_eq!(r.preview_header, None);
    assert_eq!(
        branch_hints_dropped(),
        before + 1,
        "today's replica behaviour: the hint is dropped and counted"
    );

    // …and never refused.
    let r = call(&fx.staff, "POST", uri(&fx, "agentic-schedules"), None).await;
    assert_eq!(r.status, StatusCode::OK, "no header, no read-only guard");
}

// ── Header, but not staff / not ready / not this workspace's → ignored ────────

#[tokio::test]
async fn a_customer_sending_the_header_gets_todays_behaviour() {
    let (_db, fx) = setup().await;
    let before = branch_hints_dropped();

    let r = call(
        &fx.customer,
        "GET",
        uri(&fx, "probe"),
        Some(fx.staging_rev.to_string()),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["which"], "main", "the pin is ignored for a customer");
    assert_eq!(r.preview_header, None);
    assert_eq!(branch_hints_dropped(), before + 1);

    let r = call(
        &fx.customer,
        "POST",
        uri(&fx, "agentic-schedules"),
        Some(fx.staging_rev.to_string()),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "not in a preview, so not read-only either"
    );
}

#[tokio::test]
async fn a_revision_this_workspace_cannot_serve_is_ignored() {
    let (_db, fx) = setup().await;
    for (label, header) in [
        ("still compiling", fx.compiling_rev.to_string()),
        ("another workspace's", fx.foreign_rev.to_string()),
        ("a draft", fx.draft_rev.to_string()),
        ("unknown", Uuid::new_v4().to_string()),
        ("not an id", BRANCH.to_string()),
    ] {
        let r = call(&fx.staff, "GET", uri(&fx, "probe"), Some(header)).await;
        assert_eq!(r.status, StatusCode::OK, "{label}");
        assert_eq!(r.body["which"], "main", "{label}");
        assert_eq!(r.preview_header, None, "{label}");
    }
}

// ── Read-only in a preview ────────────────────────────────────────────────────

#[tokio::test]
async fn a_preview_refuses_actions_before_they_run_and_still_answers_queries() {
    let (_db, fx) = setup().await;
    let ran_before = ACTIONS_RUN.load(Ordering::SeqCst);
    let pin = Some(fx.staging_rev.to_string());

    for (route, what) in [
        ("agentic-schedules", "Creating a schedule"),
        ("files/Zm9v", "Editing files"),
    ] {
        let r = call(&fx.staff, "POST", uri(&fx, route), pin.clone()).await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{route}");
        assert_eq!(r.body["code"], "preview_read_only", "{route}");
        assert_eq!(
            r.body["message"],
            format!("{what} isn't available in a preview; merge the branch to run it"),
        );
        assert!(
            r.preview_header.is_some(),
            "a refusal is a preview response too"
        );
    }
    assert_eq!(
        ACTIONS_RUN.load(Ordering::SeqCst),
        ran_before,
        "a refused action never reaches its handler"
    );

    let r = call(&fx.staff, "POST", uri(&fx, "sql/query"), pin).await;
    assert_eq!(r.status, StatusCode::OK, "a query only reads");
}

// ── Background work never reads a preview ────────────────────────────────────

#[tokio::test]
async fn background_work_resolves_the_promoted_revision_even_from_a_pinned_request() {
    let (_db, fx) = setup().await;

    // Work a preview request hands to the background reads main, while the
    // request itself reads the pinned revision.
    let r = call(
        &fx.staff,
        "GET",
        uri(&fx, "spawn-probe"),
        Some(fx.staging_rev.to_string()),
    )
    .await;
    assert_eq!(r.body["which"], "staging");
    assert_eq!(r.body["background"], "main");

    // The same, with the pin set directly.
    let ws = fx.ws;
    let spawned = with_staging_pin(Some(fx.staging_rev), async move {
        tokio::spawn(async move { which(ws).await }).await.unwrap()
    })
    .await;
    assert_eq!(spawned, "main");

    // A schedule tick, a monitor scan, a health pass: no request at all, and it
    // may even carry the previewed branch as a hint. Still the promoted
    // revision — `current_revision_id` never points at a staging revision, and
    // nothing outside a request reads the preview header.
    for hint in [None, Some(BRANCH)] {
        let v = resolve_semantic_view(fx.ws, hint, VIEW)
            .await
            .unwrap()
            .expect("the promoted revision has the view");
        assert_eq!(v.definition["which"], "main", "hint {hint:?}");
    }
}

// ── Deleting a preview releases its revision ─────────────────────────────────

mod release;
