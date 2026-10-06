//! End-to-end integration test for the Airhouse onboarding lifecycle.
//!
//! Exercises (post Phase-5, ephemeral-only):
//!   1. pre-provision: GET /connection returns `is_provisioned: false`.
//!   2. POST /provision provisions tenant + service account (no per-user
//!      airhouse_users row).
//!   3. GET /connection returns `is_provisioned: true` with role + dbname.
//!   4. GET /credentials mints a fresh ephemeral via the broker — username,
//!      password, expires_at. The broker cache returns the same credential
//!      on a subsequent in-window call.
//!   5. DELETE /tokens/{username} revokes the ephemeral.
//!   6. TenantProvisioner.deprovision tears down tenant + SA.
//!
//! Plus an error-mapping test: 409 on create-tenant → provisioner adopts the
//! existing remote tenant.
//!
//! Postgres is provided by testcontainers; the Airhouse admin API is stubbed
//! with wiremock and matchers verify the auth header, path, and body shape.
//!
//! Run with:
//!   cargo nextest run -p oxy-app --test airhouse -E 'test(airhouse_lifecycle)'

use airhouse::api::handlers as airhouse_me;
use airhouse::entity::Tenants as AirhouseTenants;
use airhouse::entity::tenants as airhouse_tenants;
use airhouse::{AirhouseAdminClient, TenantProvisioner};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, post};
use base64::Engine as _;
use base64::engine::general_purpose;
use chrono::Utc;
use entity::org_members::{self, OrgRole};
use entity::organizations;
use entity::users::{self, UserStatus};
use entity::workspaces::{self, WorkspaceStatus};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{header, method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ADMIN_TOKEN: &str = "test-admin-token";

// ── shared bootstrap ────────────────────────────────────────────────────────

/// A fresh per-test database carrying the central + airhouse schema, with the
/// global `establish_connection()` pool pointed at it.
///
/// The migration chain runs once per `cargo nextest run` into a template that
/// this clones; see `tests/common/mod.rs`.
async fn test_db() -> DatabaseConnection {
    let (db, test_url) = crate::common::fresh_db(crate::common::Schema::CentralAirhouse).await;
    // SAFETY: single-threaded test setup before any other env access. nextest
    // runs each test in its own process, so this cannot race a sibling.
    unsafe { std::env::set_var("OXY_DATABASE_URL", &test_url) };
    db
}

fn set_test_encryption_key() {
    unsafe {
        std::env::set_var(
            "OXY_ENCRYPTION_KEY",
            general_purpose::STANDARD.encode([3u8; 32]),
        );
    }
}

/// Set the Airhouse env vars so `AirhouseConfig::cached()` (used by the HTTP
/// handlers) resolves to `Enabled`. The cached config is a process-wide
/// `OnceLock`, so this must be called before the first handler invocation —
/// nextest runs each test in its own process so the cache starts empty.
fn set_airhouse_env(server: &MockServer) {
    unsafe {
        std::env::set_var("AIRHOUSE_BASE_URL", server.uri());
        std::env::set_var("AIRHOUSE_ADMIN_TOKEN", ADMIN_TOKEN);
        std::env::set_var("AIRHOUSE_WIRE_HOST", "airhouse.test");
        std::env::set_var("AIRHOUSE_WIRE_PORT", "5445");
    }
}

fn admin_client(server: &MockServer) -> AirhouseAdminClient {
    AirhouseAdminClient::new(server.uri(), ADMIN_TOKEN)
}

/// Seed a user, org, workspace, and org membership row.
/// Returns `(workspace_id, user_id, AuthenticatedUser)`.
async fn seed_user_and_workspace(
    db: &DatabaseConnection,
    email_prefix: &str,
) -> (Uuid, Uuid, AuthenticatedUser) {
    seed_user_workspace_with_role(db, email_prefix, Some(OrgRole::Owner)).await
}

async fn seed_user_workspace_with_role(
    db: &DatabaseConnection,
    email_prefix: &str,
    role: Option<OrgRole>,
) -> (Uuid, Uuid, AuthenticatedUser) {
    let user_id = Uuid::new_v4();
    let now = Utc::now().fixed_offset();
    let email = format!("{email_prefix}-{user_id}@example.com");

    let user = users::ActiveModel {
        id: ActiveValue::Set(user_id),
        email: ActiveValue::Set(Some(email.clone())),
        name: ActiveValue::Set("Test User".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        magic_link_token: ActiveValue::Set(None),
        magic_link_token_expires_at: ActiveValue::Set(None),
        status: ActiveValue::Set(UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("insert user");

    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("Acme".into()),
        slug: ActiveValue::Set(format!("acme-{}", &org_id.simple().to_string()[..8])),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("insert org");

    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set("Acme Workspace".into()),
        git_namespace_id: ActiveValue::Set(None),
        git_remote_url: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        path: ActiveValue::Set(None),
        last_opened_at: ActiveValue::Set(None),
        created_by: ActiveValue::Set(None),
        org_id: ActiveValue::Set(Some(org_id)),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        error: ActiveValue::Set(None),
        monthly_vlm_budget_micros: ActiveValue::Set(None),
        current_revision_id: ActiveValue::Set(None),
        default_branch: ActiveValue::Set(None),
        repo_subdir: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("insert workspace");

    if let Some(r) = role {
        org_members::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            org_id: ActiveValue::Set(org_id),
            user_id: ActiveValue::Set(user_id),
            role: ActiveValue::Set(r),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(db)
        .await
        .expect("insert membership");
    }

    let auth = AuthenticatedUser {
        id: user.id,
        email: user.email,
        name: user.name,
        picture: user.picture,
        status: user.status,
        credential: None,
    };
    (workspace_id, user_id, auth)
}

// ── HTTP test harness ───────────────────────────────────────────────────────

fn auth_inject_layer(
    user: AuthenticatedUser,
) -> impl Clone + Fn(Request<Body>, Next) -> futures::future::BoxFuture<'static, Response> {
    move |mut req: Request<Body>, next: Next| {
        let user = user.clone();
        Box::pin(async move {
            req.extensions_mut().insert(user);
            next.run(req).await
        })
    }
}

fn build_router(user: AuthenticatedUser) -> Router {
    Router::new()
        .route("/airhouse/me/connection", get(airhouse_me::get_connection))
        .route(
            "/airhouse/me/credentials",
            get(airhouse_me::get_credentials),
        )
        .route("/airhouse/me/provision", post(airhouse_me::provision))
        .route(
            "/airhouse/me/tokens/{username}",
            delete(airhouse_me::revoke_token),
        )
        .layer(middleware::from_fn(auth_inject_layer(user)))
}

async fn delete_request(router: &Router, path_str: &str) -> StatusCode {
    let req = Request::builder()
        .method("DELETE")
        .uri(path_str)
        .body(Body::empty())
        .unwrap();
    router.clone().oneshot(req).await.expect("oneshot").status()
}

async fn send_json(
    router: &Router,
    method_name: &str,
    path_str: &str,
    body: &str,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method_name)
        .uri(path_str)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = router.clone().oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .expect("read body");
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("parse json")
    };
    (status, json)
}

async fn get_json(router: &Router, path_str: &str) -> (StatusCode, Value) {
    send_json(router, "GET", path_str, "").await
}

async fn post_json(router: &Router, path_str: &str, tenant_name: &str) -> (StatusCode, Value) {
    let body = json!({ "tenant_name": tenant_name }).to_string();
    send_json(router, "POST", path_str, &body).await
}

// ── happy-path lifecycle ────────────────────────────────────────────────────

#[tokio::test]
async fn full_lifecycle_provision_to_deprovision() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;
    set_airhouse_env(&server);
    let (workspace_id, _user_id, auth) = seed_user_and_workspace(&db, "alice").await;

    // POST /admin/v1/tenants — body must carry only {id}; airhouse rejects
    // bucket/prefix in the request and resolves them server-side from
    // [storage] config.
    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants"))
        .and(header("authorization", &*format!("Bearer {ADMIN_TOKEN}")))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            assert!(
                body.get("bucket").is_none() && body.get("prefix").is_none(),
                "create-tenant request must not include bucket/prefix (got {body})"
            );
            let id = body["id"].as_str().unwrap().to_string();
            let prefix = format!("tenants/{id}");
            ResponseTemplate::new(201).set_body_json(json!({
                "id": id,
                "pg_url": "postgres://internal",
                "bucket": "test-bucket",
                "prefix": prefix,
                "role": format!("airhouse_tenant_{id}"),
                "status": "active",
                "created_at": "2026-04-29T10:00:00Z",
            }))
        })
        .expect(1)
        .mount(&server)
        .await;

    // GET /admin/v1/tenants/{tenant} — reconcile on re-provision.
    Mock::given(method("GET"))
        .and(path_regex(r"^/admin/v1/tenants/[^/]+$"))
        .and(header("authorization", &*format!("Bearer {ADMIN_TOKEN}")))
        .respond_with(move |req: &wiremock::Request| {
            let id = req.url.path().rsplit('/').next().unwrap().to_string();
            ResponseTemplate::new(200).set_body_json(json!({
                "id": id,
                "pg_url": "postgres://internal",
                "bucket": "test-bucket",
                "prefix": format!("tenants/{id}"),
                "role": format!("airhouse_tenant_{id}"),
                "status": "active",
                "created_at": "2026-04-29T10:00:00Z",
            }))
        })
        .mount(&server)
        .await;

    // DELETE /admin/v1/tenants/{tenant} — final deprovision.
    Mock::given(method("DELETE"))
        .and(path_regex(r"^/admin/v1/tenants/[^/]+$"))
        .and(header("authorization", &*format!("Bearer {ADMIN_TOKEN}")))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    // GET /admin/v1/service-accounts — orphan check on first provision; the
    // second provision call short-circuits via the local row's SA fields.
    Mock::given(method("GET"))
        .and(path("/admin/v1/service-accounts"))
        .and(header("authorization", &*format!("Bearer {ADMIN_TOKEN}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;

    // POST /admin/v1/service-accounts — provisioner mints exactly one SA per
    // tenant on first provision.
    Mock::given(method("POST"))
        .and(path("/admin/v1/service-accounts"))
        .and(header("authorization", &*format!("Bearer {ADMIN_TOKEN}")))
        .respond_with(move |req: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            ResponseTemplate::new(201).set_body_json(json!({
                "id": format!("sa_{}", Uuid::new_v4().simple()),
                "name": body["name"],
                "tenant_id": body["tenant_id"],
                "max_role": body["max_role"],
                "max_ttl_secs": body["max_ttl_secs"],
                "created_at": "2026-05-07T10:00:00Z",
                "revoked_at": null,
                "last_used_at": null,
                "bearer": format!("ahsa_{}", Uuid::new_v4().simple()),
            }))
        })
        .expect(1)
        .mount(&server)
        .await;

    // DELETE /admin/v1/service-accounts/{id} — deprovision revokes the SA
    // before deleting the tenant.
    Mock::given(method("DELETE"))
        .and(path_regex(r"^/admin/v1/service-accounts/[^/]+$"))
        .and(header("authorization", &*format!("Bearer {ADMIN_TOKEN}")))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    // POST /admin/v1/tenants/{tenant}/tokens — broker mints a fresh
    // ephemeral. We expect exactly one mint despite two GET /credentials
    // calls because the broker cache is fresh on the second call (TTL is
    // well past the 60s refresh buffer).
    let mint_username = format!("eph_{}", Uuid::new_v4().simple());
    let mint_username_for_resp = mint_username.clone();
    Mock::given(method("POST"))
        .and(path_regex(r"^/admin/v1/tenants/[^/]+/tokens$"))
        .respond_with(move |req: &wiremock::Request| {
            let tenant = req
                .url
                .path()
                .trim_start_matches("/admin/v1/tenants/")
                .trim_end_matches("/tokens")
                .to_string();
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            ResponseTemplate::new(201).set_body_json(json!({
                "username": mint_username_for_resp,
                "password": format!("tk_{}", Uuid::new_v4().simple()),
                "tenant": tenant,
                "role": body["role"],
                "expires_at": (chrono::Utc::now() + chrono::Duration::hours(24)).to_rfc3339(),
                "service_account_id": "sa_test",
            }))
        })
        .expect(1)
        .mount(&server)
        .await;

    // DELETE /admin/v1/tenants/{tenant}/tokens/{username} — user-initiated
    // revoke of the minted ephemeral.
    Mock::given(method("DELETE"))
        .and(path_regex(r"^/admin/v1/tenants/[^/]+/tokens/[^/]+$"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let router = build_router(auth);

    // ── 1. pre-provision: /connection returns is_provisioned=false ───────
    let (status, body) = get_json(
        &router,
        &format!("/airhouse/me/connection?workspace_id={workspace_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["is_provisioned"], false);
    assert_eq!(body["host"], "airhouse.test");
    assert_eq!(body["port"], 5445);
    assert_eq!(body["role"], "admin");
    assert!(
        body.get("username").is_none() || body["username"].is_null(),
        "ephemeral-only flow must not surface a stable username on /connection"
    );

    // ── 2. provision tenant + SA via the HTTP endpoint ──────────────────
    let (status, body) = post_json(
        &router,
        &format!("/airhouse/me/provision?workspace_id={workspace_id}"),
        "acme",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["host"], "airhouse.test");
    assert_eq!(body["port"], 5445);
    assert_eq!(body["is_provisioned"], true);
    let tenant_id = body["dbname"].as_str().expect("dbname").to_string();

    let tenant_row = AirhouseTenants::find()
        .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
        .one(&db)
        .await
        .unwrap()
        .expect("local tenant row written by provision");
    assert_eq!(tenant_row.airhouse_tenant_id, tenant_id);
    assert!(
        tenant_row.service_account_id.is_some(),
        "SA fields populated on provision"
    );

    // Idempotency: a second POST must NOT call Airhouse for tenant-create
    // again (the .expect(1) on /tenants would catch it).
    let (status, body) = post_json(
        &router,
        &format!("/airhouse/me/provision?workspace_id={workspace_id}"),
        "acme",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["is_provisioned"], true);

    // ── 3. /connection now reports provisioned ───────────────────────────
    let (status, body) = get_json(
        &router,
        &format!("/airhouse/me/connection?workspace_id={workspace_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["dbname"], tenant_id);
    assert_eq!(body["is_provisioned"], true);

    // ── 4. /credentials mints fresh; cache hits on second call ───────────
    let (status, creds1) = get_json(
        &router,
        &format!("/airhouse/me/credentials?workspace_id={workspace_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let first_username = creds1["username"]
        .as_str()
        .filter(|s| s.starts_with("eph_"))
        .expect("ephemeral username on first call")
        .to_string();
    let first_pw = creds1["password"]
        .as_str()
        .filter(|p| p.starts_with("tk_"))
        .expect("ephemeral password on first call")
        .to_string();
    assert!(creds1["expires_at"].is_string(), "expires_at on response");
    assert_eq!(creds1["dbname"], tenant_id);

    // Second call within the broker's freshness window MUST return the same
    // cached credential — wiremock's .expect(1) on /tokens enforces it.
    let (status, creds2) = get_json(
        &router,
        &format!("/airhouse/me/credentials?workspace_id={workspace_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        creds2["username"].as_str().unwrap(),
        first_username,
        "broker cache should return same credential within freshness window"
    );
    assert_eq!(creds2["password"].as_str().unwrap(), first_pw);

    // ── 5. user-initiated revoke ─────────────────────────────────────────
    let status = delete_request(
        &router,
        &format!("/airhouse/me/tokens/{first_username}?workspace_id={workspace_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // ── 6. tenant deprovision ────────────────────────────────────────────
    let tenant_prov = TenantProvisioner::new(db.clone(), admin_client(&server));
    tenant_prov
        .deprovision(workspace_id)
        .await
        .expect("deprovision tenant");
    let local_tenants = AirhouseTenants::find()
        .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
        .all(&db)
        .await
        .unwrap();
    assert!(local_tenants.is_empty(), "local tenant row must be cleared");
}

// ── error-mapping: 409 on create-tenant ─────────────────────────────────────

/// HTTP `POST /airhouse/me/provision` against an already-taken tenant
/// name surfaces 409 Conflict to the user. Silent adoption was the
/// previous behaviour; it could have granted cross-workspace data
/// access if two users picked the same name. Operators recover the
/// rare legitimate "this workspace's own remote tenant exists but the
/// local row was wiped" case via the runbook (delete remote, retry).
#[tokio::test]
async fn create_tenant_409_returns_conflict() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;
    set_airhouse_env(&server);
    let (workspace_id, _user_id, auth) = seed_user_and_workspace(&db, "preexisting").await;

    Mock::given(method("POST"))
        .and(path("/admin/v1/tenants"))
        .and(header("authorization", &*format!("Bearer {ADMIN_TOKEN}")))
        .respond_with(ResponseTemplate::new(409).set_body_string("tenant already exists"))
        .expect(1)
        .mount(&server)
        .await;

    // No SA mocks — the provisioner must reject before
    // ensure_service_account_for_workspace fires. wiremock would log
    // unmatched requests if the rejection slipped.

    let router = build_router(auth);
    let (status, _body) = post_json(
        &router,
        &format!("/airhouse/me/provision?workspace_id={workspace_id}"),
        "preexisting",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "name collision must surface as 409 Conflict"
    );

    // We must NOT write a Failed local row that points at the colliding
    // tenant name. If we did, the next provision call would hit
    // `reconcile_existing` first, see the stale row pointing at the
    // foreign tenant, and adopt it — exactly the cross-workspace data
    // leak that returning 409 was designed to prevent.
    let local = AirhouseTenants::find()
        .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
        .one(&db)
        .await
        .unwrap();
    assert!(
        local.is_none(),
        "no local row should be written on a name-collision 409; \
         leaving one would let a future provision call silently adopt \
         the foreign tenant"
    );
}

// (404-on-delete-user adoption test removed in Phase 5 — there is no
// per-user airhouse user anymore. UserProvisioner-specific behaviour is
// covered by `airhouse_user_provisioner.rs` until Phase 6 deletes the
// provisioner entirely.)

// ── provision endpoint: 503 when Airhouse is not configured ─────────────────

#[tokio::test]
async fn provision_returns_503_when_airhouse_disabled() {
    set_test_encryption_key();
    let db = test_db().await;
    let (workspace_id, _user_id, auth) = seed_user_and_workspace(&db, "no-airhouse").await;

    let router = build_router(auth);
    let (status, _) = post_json(
        &router,
        &format!("/airhouse/me/provision?workspace_id={workspace_id}"),
        "testname",
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

    let (status, _) = get_json(
        &router,
        &format!("/airhouse/me/connection?workspace_id={workspace_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

// ── provision endpoint: 403 for callers who are not org members ─────────────

#[tokio::test]
async fn provision_returns_403_for_non_members() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;
    set_airhouse_env(&server);
    // Pass `None` for the role so no `org_members` row is written.
    let (workspace_id, _user_id, auth) = seed_user_workspace_with_role(&db, "stranger", None).await;

    let router = build_router(auth);
    let (status, _) = post_json(
        &router,
        &format!("/airhouse/me/provision?workspace_id={workspace_id}"),
        "stranger",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ── provision endpoint: 422 for invalid tenant names ────────────────────────

#[tokio::test]
async fn provision_returns_422_for_invalid_tenant_name() {
    set_test_encryption_key();
    let db = test_db().await;
    let server = MockServer::start().await;
    set_airhouse_env(&server);
    let (workspace_id, _user_id, auth) = seed_user_and_workspace(&db, "badname").await;

    let router = build_router(auth);
    let (status, _) = post_json(
        &router,
        &format!("/airhouse/me/provision?workspace_id={workspace_id}"),
        "1-starts-with-digit",
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
