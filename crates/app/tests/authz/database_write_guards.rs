//! Who may test a warehouse connection, and who may rebuild a workspace's index.
//!
//! `POST /{ws}/databases/test-connection` builds a throwaway database config from
//! the caller's request. For BigQuery that writes `<name>.key` into the working
//! copy with the caller's content; for a password warehouse it may store a
//! temporary secret. `POST /{ws}/databases/build` runs a
//! reindex. `workspace_middleware` proves a caller may *open* the workspace, not
//! that they may change it, and neither handler named a role guard, so any
//! member could do either, a read-only token included.
//!
//! Both are `WorkspaceAdmin`, like their siblings on the same router
//! (`POST /databases`, `/databases/sync`, `/databases/clean`). The connection
//! test is step one of Add database, whose save is `WorkspaceAdmin`. The only
//! caller of build is the Build button in Settings → Databases, which renders
//! inside `CanWorkspaceAdmin` next to Clean.
//!
//! The handlers are mounted at their real paths behind the real
//! `workspace_middleware`, over a per-test database and a working copy in a
//! temp dir, with `IdeState` as the router state (both handlers ask for a
//! working copy, which resolves only there). Each role is derived the way a
//! request derives it: org role to workspace role, a token's ceiling capping it.

use std::time::Duration;

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::{Router, middleware};
use entity::org_members::OrgRole;
use entity::workspaces::WorkspaceStatus;
use entity::{org_members, organizations, users, workspaces};
use oxy_app::api::data::build_embeddings;
use oxy_app::api::database::test_database_connection;
use oxy_app::api::middlewares::workspace_context::{EffectiveWorkspaceRole, workspace_middleware};
use oxy_app::server::router::{IdeState, bare_app_state};
use oxy_auth::token::{CredentialContext, StoredKind};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{RoleCeiling, TokenGrant};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

/// One local warehouse, so the working copy loads; no integrations.
const CONFIG: &str =
    "databases:\n  - name: local\n    type: duckdb\n    dataset: .db/\nmodels: []\n";

struct Fx {
    db: DatabaseConnection,
    org: Uuid,
    ws: Uuid,
    root: tempfile::TempDir,
}

/// A caller as the auth layer hands it on: the user, plus the token's
/// credential beside it when the request came in on one.
struct Who {
    user: AuthenticatedUser,
    credential: Option<CredentialContext>,
}

async fn setup() -> Fx {
    let db = crate::common::test_db().await;
    let root = tempfile::tempdir().expect("working copy");
    std::fs::write(root.path().join("config.yml"), CONFIG).expect("write config.yml");
    std::fs::create_dir_all(root.path().join(".db")).expect("create .db");

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

    let ws = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws),
        name: ActiveValue::Set("warehouses".into()),
        org_id: ActiveValue::Set(Some(org)),
        // A path, and no promoted revision: the middleware builds the manager
        // from this working copy, as the ide node does.
        path: ActiveValue::Set(Some(root.path().to_string_lossy().into_owned())),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed workspace");

    Fx { db, org, ws, root }
}

/// A person whose org role is `role`. Org Admin derives workspace Admin, org
/// Member derives workspace Member (the role `WorkspaceEditor` admits).
async fn person(fx: &Fx, role: OrgRole) -> Who {
    let id = Uuid::new_v4();
    let email = format!("{}@acme.test", id.simple());
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(email.clone())),
        name: ActiveValue::Set(email.clone()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed user");
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(fx.org),
        user_id: ActiveValue::Set(id),
        role: ActiveValue::Set(role),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed membership");
    Who {
        user: AuthenticatedUser {
            id,
            email: Some(email.clone()),
            name: email,
            picture: None,
            status: users::UserStatus::Active,
            credential: None,
        },
        credential: None,
    }
}

/// A workspace Viewer: an org member on a read-only token for this workspace.
/// A token's ceiling is the only way a person resolves below Member.
async fn viewer(fx: &Fx) -> Who {
    let who = person(fx, OrgRole::Member).await;
    let credential = CredentialContext {
        token_id: Uuid::new_v4(),
        kind: StoredKind::Personal,
        principal_user_id: who.user.id,
        all_access: false,
        platform: false,
        partner: false,
        name: "read-only".into(),
        display_prefix: "oxy_pat_test".into(),
        legacy_api_key_id: None,
        blocked_orgs: Vec::new(),
        expires_at: None,
        service_account: None,
        grants: vec![TokenGrant {
            org_id: fx.org,
            workspace_id: Some(fx.ws),
            ceiling: RoleCeiling::Viewer,
        }],
        app_publish: Vec::new(),
        app_sandbox: Vec::new(),
    };
    Who {
        user: who.user.with_credential(Some(credential.clone())),
        credential: Some(credential),
    }
}

/// The role `workspace_middleware` resolved for this request.
async fn role_probe(EffectiveWorkspaceRole(role): EffectiveWorkspaceRole) -> String {
    role.as_str().to_string()
}

/// The two handlers at the paths `build_database_routes` mounts them on, inside
/// the `/{workspace_id}` nest, behind `workspace_middleware`.
fn app(who: &Who) -> Router {
    let state = bare_app_state();
    let workspace = Router::new()
        .route("/databases/test-connection", post(test_database_connection))
        .route("/databases/build", post(build_embeddings))
        .route("/role-probe", get(role_probe))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            workspace_middleware,
        ));
    let mut app = Router::new()
        .nest("/{workspace_id}", workspace)
        .layer(Extension(who.user.clone()));
    if let Some(credential) = &who.credential {
        app = app.layer(Extension(credential.clone()));
    }
    app.with_state(IdeState(state))
}

/// Status and the whole body. The connection test answers with an SSE stream
/// that ends when its probe task finishes, so reading to the end also waits
/// for that task (and its secret cleanup) instead of leaving it to the
/// runtime's shutdown.
async fn send(who: &Who, request: Request<Body>) -> (StatusCode, String) {
    let response = app(who).oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = tokio::time::timeout(
        Duration::from_secs(60),
        axum::body::to_bytes(response.into_body(), 256 * 1024),
    )
    .await
    .expect("the response body must finish")
    .expect("read body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn test_connection(fx: &Fx, who: &Who, warehouse: Value) -> (StatusCode, String) {
    let request = Request::builder()
        .method("POST")
        .uri(format!("/{}/databases/test-connection", fx.ws))
        .header("content-type", "application/json")
        .body(Body::from(json!({ "warehouse": warehouse }).to_string()))
        .unwrap();
    send(who, request).await
}

async fn build(fx: &Fx, who: &Who) -> (StatusCode, String) {
    let request = Request::builder()
        .method("POST")
        .uri(format!("/{}/databases/build", fx.ws))
        .body(Body::empty())
        .unwrap();
    send(who, request).await
}

async fn resolved_role(fx: &Fx, who: &Who) -> String {
    let request = Request::builder()
        .uri(format!("/{}/role-probe", fx.ws))
        .body(Body::empty())
        .unwrap();
    let (status, body) = send(who, request).await;
    assert_eq!(status, StatusCode::OK, "the probe must resolve: {body}");
    body
}

/// A BigQuery warehouse: building its config writes the caller's key into the
/// working copy as `<name>.key` before any connection is tried.
fn bigquery() -> Value {
    json!({ "type": "bigquery", "config": { "key": "not-a-real-key", "dataset": "d" } })
}

/// Key files in the working copy root.
fn key_files(fx: &Fx) -> Vec<String> {
    std::fs::read_dir(fx.root.path())
        .expect("list working copy")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".key"))
        .collect()
}

/// Each refusal is only as good as the caller it was made against: a "viewer"
/// that resolved as a Member would make the viewer cases repeat the member ones.
#[tokio::test]
async fn each_caller_resolves_to_the_workspace_role_it_is_named_for() {
    let fx = setup().await;
    let admin = person(&fx, OrgRole::Admin).await;
    let member = person(&fx, OrgRole::Member).await;
    let viewer = viewer(&fx).await;
    assert_eq!(resolved_role(&fx, &admin).await, "admin");
    assert_eq!(resolved_role(&fx, &member).await, "member");
    assert_eq!(resolved_role(&fx, &viewer).await, "viewer");
}

// ── test-connection ──────────────────────────────────────────────────────────
//
// Multi-threaded: the handler's secret cleanup uses `block_in_place`, which
// panics on a current-thread runtime.

#[tokio::test(flavor = "multi_thread")]
async fn a_viewer_cannot_test_a_warehouse_connection() {
    let fx = setup().await;
    let who = viewer(&fx).await;
    let (status, body) = test_connection(&fx, &who, bigquery()).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "key files written: {:?}; body: {body}",
        key_files(&fx)
    );
    assert_eq!(
        key_files(&fx),
        Vec::<String>::new(),
        "a refused test must write nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_member_cannot_test_a_warehouse_connection() {
    let fx = setup().await;
    let who = person(&fx, OrgRole::Member).await;
    let (status, body) = test_connection(&fx, &who, bigquery()).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the connection test is step one of Add database, which is WorkspaceAdmin. \
         key files written: {:?}; body: {body}",
        key_files(&fx)
    );
    assert_eq!(
        key_files(&fx),
        Vec::<String>::new(),
        "a refused test must write nothing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_workspace_admin_still_tests_a_warehouse_connection() {
    let fx = setup().await;
    let who = person(&fx, OrgRole::Admin).await;
    let duckdb = json!({ "type": "duckdb", "config": { "file_search_path": ".db/" } });
    let (status, body) = test_connection(&fx, &who, duckdb).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        body.contains("\"type\":\"complete\"") && body.contains("\"success\":true"),
        "the admin's test must run to a successful result: {body}"
    );
}

// ── build ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_viewer_cannot_build_the_workspace_index() {
    let fx = setup().await;
    let who = viewer(&fx).await;
    let (status, body) = build(&fx, &who).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
}

#[tokio::test]
async fn a_workspace_member_cannot_build_the_workspace_index() {
    let fx = setup().await;
    let who = person(&fx, OrgRole::Member).await;
    let (status, body) = build(&fx, &who).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "build sits beside `/databases/clean` and `/databases/sync`, both \
         WorkspaceAdmin, and its only UI is admin-only. body: {body}"
    );
}

#[tokio::test]
async fn a_workspace_admin_still_builds_the_workspace_index() {
    let fx = setup().await;
    let who = person(&fx, OrgRole::Admin).await;
    let (status, body) = build(&fx, &who).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let body: Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(body["success"], json!(true), "body: {body}");
}
