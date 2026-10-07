//! API-token authentication against a real database — the first tests API keys
//! have ever had (design `2026-09-30-api-tokens-design.md`, §8 Phase 1).
//!
//! The hard constraint under test: **every existing key keeps working**, with
//! the same reach and precedence, on `/api`, `/external/api`, the custom-app
//! path and `?api_key=` — including a key an older pod minted or revoked.
//!
//! Requests go through the served auth stacks themselves
//! (`api_auth_layers` / `external_auth_layers`), not a copy, around a probe
//! handler that reports who the request authenticated as. Each test owns a
//! database cloned from the per-run template (`common::test_db`), so these run
//! in nextest's `db-per-test` group, not `serial-db`.

mod account_discovery;
mod activity;
mod agent_token;
mod agent_token_rules;
mod agent_token_standing;
mod app_admin_ceiling;
mod assume_binding;
mod audit_actions;
mod audit_per_org;
mod blocked_reach;
mod browser_session;
mod cli_login;
mod deactivated_owner;
mod discovery;
mod email_identity;
mod extend;
mod grant_reach;
mod hygiene;
mod legacy_keys;
mod legacy_reach;
mod mint_locks;
mod new_tokens;
mod oidc;
mod org_inventory;
mod personal_app_publish;
mod policy;
mod publish_token_mint;
mod sandbox_agent;
mod sandbox_agent_cli;
mod sandbox_agent_leak;
mod sandbox_agent_reach;
mod sandbox_agent_staff;
mod sandbox_agent_sweep;
mod serve_tree_usage;
mod service_accounts;
mod session_key;
mod stack;
mod standing_tokens;
mod token_api;
mod trust_policies;
mod trusted_access;
mod trusted_publish;
mod usage;
mod workspace_inventory;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Utc};
use entity::users::UserStatus;
use entity::workspace_members::WorkspaceRole;
use entity::workspaces::WorkspaceStatus;
use entity::{api_keys, api_tokens, organizations, users, workspaces};
use oxy_app::api::api_keys::{delete_api_key, extend_api_key, get_api_key_activity};
use oxy_app::api::middlewares::workspace_context::EffectiveWorkspaceRole;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_auth::token::CredentialContext;
use oxy_auth::types::AuthenticatedUser;
use oxy_auth::{ApiKeyConfig, ApiKeyService, CreateApiKeyParams};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::common;

pub(crate) struct Fixture {
    pub db: DatabaseConnection,
    pub user: users::Model,
    pub org_id: Uuid,
    pub workspace_id: Uuid,
    /// A valid browser session for `user`.
    pub cookie: String,
}

/// A migrated database of this test's own, auth switched on, the credential
/// cache empty, one user in one org's workspace, and a session for them.
pub(crate) async fn fixture() -> Fixture {
    let db = common::test_db().await;
    oxy_auth::built_in::set_auth_configured(true);
    oxy_auth::token::cache::clear();
    let user = seed_user(&db, "owner").await;
    let (org_id, workspace_id) = seed_workspace(&db).await;
    let jwt = oxy_app::server::api::auth::create_auth_token(user.clone())
        .await
        .expect("mint a session");
    Fixture {
        db,
        user,
        org_id,
        workspace_id,
        cookie: format!("oxy_session={jwt}"),
    }
}

pub(crate) async fn seed_user(db: &DatabaseConnection, prefix: &str) -> users::Model {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(format!("{prefix}-{id}@example.com"))),
        name: ActiveValue::Set("Token Test".into()),
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
    .expect("seed user")
}

async fn seed_workspace(db: &DatabaseConnection) -> (Uuid, Uuid) {
    let org_id = seed_org(db).await;
    (org_id, seed_workspace_in(db, org_id).await)
}

pub(crate) async fn seed_org(db: &DatabaseConnection) -> Uuid {
    let now = Utc::now().fixed_offset();
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("Acme".into()),
        slug: ActiveValue::Set(format!("acme-{}", org_id.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed org");
    org_id
}

/// Another workspace in `org_id`.
pub(crate) async fn seed_workspace_in(db: &DatabaseConnection, org_id: Uuid) -> Uuid {
    let now = Utc::now().fixed_offset();
    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set(format!("Acme Workspace {}", workspace_id.simple())),
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
    .expect("seed workspace");
    workspace_id
}

/// A key exactly as every release before this one wrote it: `oxy_<32 hex>`, its
/// plaintext in `api_keys.key_hash`, and no `api_tokens` row.
pub(crate) async fn legacy_key(fx: &Fixture, expires_at: Option<DateTime<Utc>>) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let key = format!("oxy_{}", Uuid::new_v4().simple());
    let now = Utc::now().fixed_offset();
    api_keys::ActiveModel {
        id: ActiveValue::Set(id),
        user_id: ActiveValue::Set(fx.user.id),
        key_hash: ActiveValue::Set(key.clone()),
        name: ActiveValue::Set("legacy".into()),
        expires_at: ActiveValue::Set(expires_at.map(|t| t.fixed_offset())),
        last_used_at: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        is_active: ActiveValue::Set(true),
        project_id: ActiveValue::Set(fx.workspace_id),
        app_id: ActiveValue::Set(None),
    }
    .insert(&fx.db)
    .await
    .expect("seed legacy key");
    (id, key)
}

/// A personal access token (`oxy_pat_`), minted the way the token routes mint
/// one: all-access, with both standings. A token is never a legacy key.
pub(crate) async fn minted_pat(fx: &Fixture, expires_at: Option<DateTime<Utc>>) -> (Uuid, String) {
    let minted = oxy_auth::token::personal::create(
        &fx.db,
        oxy_auth::token::personal::NewToken {
            user_id: fx.user.id,
            name: "minted".into(),
            all_access: true,
            platform: true,
            partner: true,
            grants: Vec::new(),
            expires_at,
            source: oxy_auth::token::credential::source::UI,
        },
    )
    .await
    .expect("mint a pat");
    (minted.row.id, minted.secret)
}

/// A legacy API key minted through the legacy endpoint's own path: an
/// `oxy_<hex>` key, written to `api_keys` and mirrored into `api_tokens`.
pub(crate) async fn endpoint_key(
    fx: &Fixture,
    expires_at: Option<DateTime<Utc>>,
) -> (Uuid, String) {
    let created = ApiKeyService::create_api_key(
        &fx.db,
        CreateApiKeyParams {
            user_id: fx.user.id,
            name: "minted".into(),
            expires_at,
            project_id: fx.workspace_id,
        },
        &ApiKeyConfig::default(),
    )
    .await
    .expect("mint a legacy key through the endpoint");
    (created.id, created.key)
}

/// N-1 revokes by writing `api_keys` alone.
pub(crate) async fn revoke_in_api_keys_only(db: &DatabaseConnection, id: Uuid) {
    let row = api_keys::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .expect("key row");
    let mut active: api_keys::ActiveModel = row.into();
    active.is_active = ActiveValue::Set(false);
    active.update(db).await.expect("revoke in api_keys");
}

/// A token's own row, by its own id (a personal token mirrors no `api_keys` row).
pub(crate) async fn pat_row(db: &DatabaseConnection, id: Uuid) -> api_tokens::Model {
    api_tokens::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .expect("the token's row")
}

pub(crate) async fn token_row(db: &DatabaseConnection, id: Uuid) -> Option<api_tokens::Model> {
    api_tokens::Entity::find()
        .filter(api_tokens::Column::LegacyApiKeyId.eq(id))
        .one(db)
        .await
        .unwrap()
}

pub(crate) async fn key_row(db: &DatabaseConnection, id: Uuid) -> api_keys::Model {
    api_keys::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .expect("key row")
}

/// Reports who the request authenticated as, and with which credential.
async fn probe(
    Extension(user): Extension<AuthenticatedUser>,
    credential: Option<Extension<CredentialContext>>,
) -> Json<Value> {
    let credential = credential.map(|Extension(c)| c);
    Json(json!({
        "user_id": user.id,
        "token_id": credential.as_ref().map(|c| c.token_id),
        "kind": credential.as_ref().map(|c| c.kind.as_str()),
    }))
}

/// The handlers under test sit behind `WorkspaceAdmin`, which reads the role
/// `workspace_middleware` resolves. That middleware is not what is under test.
async fn as_workspace_admin(mut req: Request<Body>, next: Next) -> Response {
    req.extensions_mut()
        .insert(EffectiveWorkspaceRole(WorkspaceRole::Admin));
    next.run(req).await
}

fn workspace_routes() -> Router {
    Router::new()
        .route("/probe", get(probe))
        .route("/teapot", get(|| async { StatusCode::IM_A_TEAPOT }))
        .route("/boom", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
        .route("/api-keys/{id}", delete(delete_api_key))
        .route("/api-keys/{id}/extend", post(extend_api_key))
        .route("/api-keys/{id}/activity", get(get_api_key_activity))
        .layer(middleware::from_fn(as_workspace_admin))
}

/// The action every audited handler performs, reduced to its shape: take the
/// `RequestActor`, build the entry with `for_request`, record it. Multi-key
/// metadata on purpose — the hash chain has to survive `jsonb` reordering it.
pub(crate) const AUDITED_ACTION: &str = "test.token_auth.audited";

/// `/api` plus `GET /{workspace_id}/audited`, which writes one audit row in
/// the fixture's org as whoever the request authenticated as.
pub(crate) fn audited_surface(fx: &Fixture) -> Router {
    let (db, org_id) = (fx.db.clone(), fx.org_id);
    let audited = get(move |actor: RequestActor| {
        let db = db.clone();
        async move {
            let entry = AuditEntry::for_request(&actor, AUDITED_ACTION)
                .org(org_id)
                .target("thing", "t-1", "Thing")
                .metadata(json!({ "surface": "test", "zz": 1, "a": { "bb": 2, "c": 3 } }));
            match audit::record(&db, entry).await {
                Ok(id) => (StatusCode::OK, id.to_string()),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            }
        }
    });
    oxy_app::server::router::api_auth_layers(Router::new().nest(
        "/{workspace_id}",
        workspace_routes().route("/audited", audited),
    ))
}

/// Audit rows for one action, oldest first.
pub(crate) async fn audit_rows(
    db: &DatabaseConnection,
    action: &str,
) -> Vec<entity::audit_events::Model> {
    use sea_orm::QueryOrder;
    entity::audit_events::Entity::find()
        .filter(entity::audit_events::Column::Action.eq(action))
        .order_by_asc(entity::audit_events::Column::Seq)
        .all(db)
        .await
        .unwrap()
}

/// `/api`, through its served auth stack.
pub(crate) fn api_surface() -> Router {
    oxy_app::server::router::api_auth_layers(
        Router::new().nest("/{workspace_id}", workspace_routes()),
    )
}

/// `/external/api`, through its served auth stack.
pub(crate) fn external_surface() -> Router {
    oxy_app::server::router::external_auth_layers(
        Router::new().nest("/{workspace_id}", workspace_routes()),
    )
}

/// One request: method, uri, extra headers, optional JSON body.
pub(crate) async fn call(
    router: Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = router
        .oneshot(req.body(body).unwrap())
        .await
        .expect("oneshot");
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// GET the probe on `router` with `headers`; the status and the token id it saw.
pub(crate) async fn probe_as(
    fx: &Fixture,
    router: Router,
    query: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let uri = format!("/{}/probe{query}", fx.workspace_id);
    call(router, "GET", &uri, headers, None).await
}
