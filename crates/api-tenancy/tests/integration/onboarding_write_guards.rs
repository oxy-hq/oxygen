//! Who may undo an onboarding run, and who may drop files into a workspace.
//!
//! `POST /{ws}/onboarding/reset` removes databases and models from `config.yml`
//! and deletes their secrets. `POST /{ws}/onboarding/upload-warehouse-files`
//! writes files into the working copy. `workspace_middleware` proves a caller
//! may *open* the workspace, not that they may change it, so each handler has to
//! name a role guard itself. Neither did (#3754), so any member could do either,
//! a read-only token included.
//!
//! Both are `WorkspaceAdmin`. The reset rewrites databases and secrets, and every
//! other route that does (`POST /databases`, `/secrets/*`) is `WorkspaceAdmin`.
//! The upload exists only to feed the next step of the same flow, `POST
//! /databases` with `dataset` pointed at the upload, which is `WorkspaceAdmin`
//! too: an Editor who could upload could never use what they uploaded.
//!
//! Drives the mounted `workspace_routes()` behind the real `workspace_middleware`
//! over a per-test database and a working copy in a temp dir. Each role is
//! derived the way a request derives it (org role to workspace role, a token's
//! ceiling capping it), and each refusal is checked on disk as well as by
//! status: a 403 that still wrote would pass a status check.

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Router, middleware};
use entity::org_members::OrgRole;
use entity::workspace_members::WorkspaceRole;
use entity::workspaces::WorkspaceStatus;
use entity::{org_members, organizations, users, workspace_members, workspaces};
use oxy_api_tenancy::onboarding::workspace_routes;
use oxy_app::surface::bare_app_state;
use oxy_app::surface::testing::workspace_context::workspace_middleware;
use oxy_auth::token::{CredentialContext, StoredKind};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{RoleCeiling, TokenGrant};
use oxy_server_authz::workspace_role::EffectiveWorkspaceRole;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

/// One warehouse for the reset to strip, pointed where the upload writes.
const CONFIG: &str =
    "databases:\n  - name: uploads\n    type: duckdb\n    dataset: .db/\nmodels: []\n";
const UPLOADED: &str = ".db/orders.csv";
const BOUNDARY: &str = "oxy-onboarding-guard-test";

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
        name: ActiveValue::Set("onboarding".into()),
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
/// A token's ceiling is the only way a person resolves below Member — a
/// workspace override can only raise the org-derived role.
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

/// The role `workspace_middleware` resolved for this request — what every
/// workspace guard reads.
async fn role_probe(EffectiveWorkspaceRole(role): EffectiveWorkspaceRole) -> String {
    role.as_str().to_string()
}

/// The surface's workspace routes inside the `/{workspace_id}` nest, behind
/// `workspace_middleware` — where `oxy-server` merges them — plus a probe that
/// reports the resolved role, so the fixture can show each caller is who its
/// name says.
fn app(who: &Who) -> Router {
    let state = bare_app_state();
    let workspace = workspace_routes()
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
    app.with_state(state)
}

async fn send(who: &Who, request: Request<Body>) -> (StatusCode, Value) {
    let response = app(who).oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("read body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn reset(fx: &Fx, who: &Who) -> (StatusCode, Value) {
    let body = json!({ "database_names": ["uploads"] }).to_string();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/{}/onboarding/reset", fx.ws))
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    send(who, request).await
}

async fn upload(fx: &Fx, who: &Who) -> StatusCode {
    let body = format!(
        "--{BOUNDARY}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"orders.csv\"\r\n\
         Content-Type: text/csv\r\n\r\n\
         id,amount\n1,10\n\r\n\
         --{BOUNDARY}--\r\n"
    );
    let request = Request::builder()
        .method("POST")
        .uri(format!("/{}/onboarding/upload-warehouse-files", fx.ws))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap();
    send(who, request).await.0
}

async fn resolved_role(fx: &Fx, who: &Who) -> String {
    let request = Request::builder()
        .uri(format!("/{}/role-probe", fx.ws))
        .body(Body::empty())
        .unwrap();
    let response = app(who).oneshot(request).await.expect("oneshot");
    assert_eq!(response.status(), StatusCode::OK, "the probe must resolve");
    let bytes = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .expect("read body");
    String::from_utf8(bytes.to_vec()).expect("utf-8 role")
}

fn config_lists_uploads(fx: &Fx) -> bool {
    std::fs::read_to_string(fx.root.path().join("config.yml"))
        .expect("read config.yml")
        .contains("name: uploads")
}

fn uploaded(fx: &Fx) -> bool {
    fx.root.path().join(UPLOADED).exists()
}

/// Every refusal below is only as good as the caller it was made against: a
/// "viewer" that actually resolved as a Member would make the viewer cases
/// repeat the member cases and prove nothing about a read-only token.
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

// ── reset ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_viewer_cannot_reset_onboarding() {
    let fx = setup().await;
    let who = viewer(&fx).await;
    let (status, body) = reset(&fx, &who).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
    assert!(
        config_lists_uploads(&fx),
        "a refused reset must leave config.yml alone"
    );
}

#[tokio::test]
async fn a_workspace_member_cannot_reset_onboarding() {
    let fx = setup().await;
    let who = person(&fx, OrgRole::Member).await;
    let (status, body) = reset(&fx, &who).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the reset deletes databases and secrets, which is WorkspaceAdmin \
         everywhere else; an Editor-level member must not reach it. body: {body}"
    );
    assert!(
        config_lists_uploads(&fx),
        "a refused reset must leave config.yml alone"
    );
}

#[tokio::test]
async fn a_workspace_admin_still_resets_onboarding() {
    let fx = setup().await;
    let who = person(&fx, OrgRole::Admin).await;
    let (status, body) = reset(&fx, &who).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["databases_removed"],
        json!(["uploads"]),
        "body: {body}"
    );
    assert!(
        !config_lists_uploads(&fx),
        "the admin's reset must actually remove the database"
    );
}

/// An org Member a `workspace_members` row raises to workspace Admin. The org
/// Admin above passes the model's org-officer term; this caller can only pass
/// through the per-workspace elevation the loader reads from that row, so it
/// is the case where the two would part if the loader stopped loading it.
#[tokio::test]
async fn a_member_raised_to_workspace_admin_resets_onboarding() {
    let fx = setup().await;
    let who = person(&fx, OrgRole::Member).await;
    let now = chrono::Utc::now().fixed_offset();
    workspace_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        workspace_id: ActiveValue::Set(fx.ws),
        user_id: ActiveValue::Set(who.user.id),
        role: ActiveValue::Set(WorkspaceRole::Admin),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(&fx.db)
    .await
    .expect("seed workspace override");
    assert_eq!(resolved_role(&fx, &who).await, "admin");

    let (status, body) = reset(&fx, &who).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["databases_removed"],
        json!(["uploads"]),
        "body: {body}"
    );
    assert!(
        !config_lists_uploads(&fx),
        "the elevated member's reset must actually remove the database"
    );
}

// ── upload ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_viewer_cannot_upload_warehouse_files() {
    let fx = setup().await;
    let who = viewer(&fx).await;
    assert_eq!(upload(&fx, &who).await, StatusCode::FORBIDDEN);
    assert!(!uploaded(&fx), "a refused upload must write nothing");
}

#[tokio::test]
async fn a_workspace_member_cannot_upload_warehouse_files() {
    let fx = setup().await;
    let who = person(&fx, OrgRole::Member).await;
    assert_eq!(
        upload(&fx, &who).await,
        StatusCode::FORBIDDEN,
        "the upload feeds `POST /databases`, which is WorkspaceAdmin",
    );
    assert!(!uploaded(&fx), "a refused upload must write nothing");
}

#[tokio::test]
async fn a_workspace_admin_still_uploads_warehouse_files() {
    let fx = setup().await;
    let who = person(&fx, OrgRole::Admin).await;
    assert_eq!(upload(&fx, &who).await, StatusCode::CREATED);
    assert!(
        uploaded(&fx),
        "the admin's upload must land in the working copy"
    );
}
