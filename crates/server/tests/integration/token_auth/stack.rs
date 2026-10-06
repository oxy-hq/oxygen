//! What the Phase 2 tests share: the **real** routers, and seeds for the facts
//! a grant is checked against.
//!
//! - [`flat_api`] is the cloud flat tree behind the served auth stack — the
//!   real mounts of `/orgs`, `/user/tokens`, `/auth/cli/*`, `/admin/*`,
//!   `/assume` and the rest, with `org_middleware` and the platform guards
//!   exactly where the server puts them.
//! - [`workspace_api`] / [`workspace_external`] put the workspace tree's own
//!   authorization step (`workspace_access_middleware`, the one
//!   `workspace_middleware` runs first) under the same auth stacks, around
//!   routes guarded by the real role extractors.
//!
//! Tokens are minted with `personal::create`, the function the HTTP API calls,
//! so a test states the reach it is about in one line.

use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Router, middleware};
use entity::org_members::{self, OrgRole};
use entity::{app_admins, apps};
use oxy_app::server::api::middlewares::workspace_context::workspace_access_middleware;
use oxy_app::server::authz::Caller;
use oxy_app::server::authz::role_guards::{WorkspaceAdmin, WorkspaceEditor};
use oxy_auth::authenticator::Authenticator;
use oxy_auth::built_in::BuiltInAuthenticator;
use oxy_auth::token::credential::source;
use oxy_auth::token::personal::{self, GrantSpec, NewToken};
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{PlatformRole, RoleCeiling};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Fixture, call};

/// The flat tree as `oxy-server` serves it: `oxy-app`'s global routes plus the
/// surfaces its `api` seam merges in and its staff-console sections — the
/// same crates, in the same order, as `api_seam_routes` in `main.rs`.
pub(crate) fn flat_api() -> Router {
    let api_seam = oxy_api_github::routes()
        .merge(oxy_api_tenancy::routes())
        .merge(oxy_api_tenancy::partner_console::routes())
        .merge(oxy_api_tenancy::onboarding::routes())
        .merge(oxy_api_documents::routes())
        .merge(oxy_api_frontline::routes());
    oxy_app::server::router::flat_api_surface(api_seam, oxy_api_tenancy::admin_sections())
}

/// `/read` asks for nothing beyond reaching the workspace; `/write` and
/// `/manage` take the guards every real write and admin route takes.
fn workspace_tree() -> Router {
    Router::new()
        .route("/read", get(|| async { StatusCode::NO_CONTENT }))
        .route(
            "/write",
            post(|_: WorkspaceEditor| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/manage",
            post(|_: WorkspaceAdmin| async { StatusCode::NO_CONTENT }),
        )
        .layer(middleware::from_fn(workspace_access_middleware))
}

pub(crate) fn workspace_api() -> Router {
    oxy_app::server::router::api_auth_layers(
        Router::new().nest("/{workspace_id}", workspace_tree()),
    )
}

pub(crate) fn workspace_external() -> Router {
    oxy_app::server::router::external_auth_layers(
        Router::new().nest("/{workspace_id}", workspace_tree()),
    )
}

pub(crate) async fn join_org(db: &DatabaseConnection, org_id: Uuid, user_id: Uuid, role: OrgRole) {
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(role),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed org membership");
}

pub(crate) async fn leave_org(db: &DatabaseConnection, org_id: Uuid, user_id: Uuid) {
    org_members::Entity::delete_many()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(user_id))
        .exec(db)
        .await
        .expect("remove org membership");
}

/// Oxy staff: a Global Admin grant over every org.
pub(crate) async fn make_staff(db: &DatabaseConnection, email: &str) {
    app_admins::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        email: ActiveValue::Set(email.to_string()),
        granted_by: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        role: ActiveValue::Set(PlatformRole::GlobalAdmin.as_str().to_string()),
        scope_all: ActiveValue::Set(true),
        updated_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed staff grant");
}

/// A published app of `org_id`, published from `workspace_id`.
pub(crate) async fn published_app(
    db: &DatabaseConnection,
    org_id: Uuid,
    workspace_id: Uuid,
) -> apps::Model {
    let id = Uuid::new_v4();
    apps::ActiveModel {
        id: ActiveValue::Set(id),
        slug: ActiveValue::Set(format!("app-{}", &id.simple().to_string()[..8])),
        name: ActiveValue::Set("Token Test App".into()),
        org_id: ActiveValue::Set(org_id),
        project_id: ActiveValue::Set(workspace_id),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("acme/test".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(json!({})),
        published_at: ActiveValue::Set(Some(chrono::Utc::now().into())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app")
}

/// What a minted token may reach.
#[derive(Clone)]
pub(crate) struct Reach {
    pub all_access: bool,
    pub platform: bool,
    pub partner: bool,
    pub grants: Vec<GrantSpec>,
}

impl Reach {
    /// Everything its owner reaches by membership, with no standing.
    pub(crate) fn all_access() -> Self {
        Self {
            all_access: true,
            platform: false,
            partner: false,
            grants: Vec::new(),
        }
    }

    pub(crate) fn with_platform(mut self) -> Self {
        self.platform = true;
        self
    }

    /// Only these grants.
    pub(crate) fn granted(grants: Vec<GrantSpec>) -> Self {
        Self {
            all_access: false,
            platform: false,
            partner: false,
            grants,
        }
    }
}

pub(crate) fn workspace_grant(org_id: Uuid, workspace_id: Uuid, ceiling: RoleCeiling) -> GrantSpec {
    GrantSpec {
        org_id,
        workspace_id: Some(workspace_id),
        ceiling,
    }
}

/// Every workspace in the org — the grant an org route needs.
pub(crate) fn org_grant(org_id: Uuid, ceiling: RoleCeiling) -> GrantSpec {
    GrantSpec {
        org_id,
        workspace_id: None,
        ceiling,
    }
}

/// A new-format personal token of `user_id`: `(id, secret)`.
pub(crate) async fn mint(db: &DatabaseConnection, user_id: Uuid, reach: Reach) -> (Uuid, String) {
    let minted = personal::create(
        db,
        NewToken {
            user_id,
            name: "phase 2".into(),
            all_access: reach.all_access,
            platform: reach.platform,
            partner: reach.partner,
            grants: reach.grants,
            expires_at: None,
            source: source::UI,
        },
    )
    .await
    .expect("mint a personal token");
    (minted.row.id, minted.secret)
}

/// The caller the custom-app path builds for a request carrying `secret`
/// (`custom_apps_auth::authenticate_and_authorize`, step for step): the same
/// dispatch as `/api`, then the credential attached to the user.
pub(crate) async fn custom_app_caller(fx: &Fixture, secret: &str) -> Caller {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {secret}").parse().unwrap());
    let (identity, credential) = BuiltInAuthenticator::new(oxy_auth::token::SandboxAgent::Refuse)
        .authenticate_with_credential(&headers)
        .await
        .expect("the custom-app path authenticates the credential");
    assert_eq!(identity.user_id, Some(fx.user.id));
    let user = AuthenticatedUser::from(fx.user.clone()).with_credential(credential);
    Caller::from_user(&user)
}

/// `GET uri` on the flat tree as `secret`.
pub(crate) async fn get_as(secret: &str, uri: &str) -> (StatusCode, Value) {
    let bearer = format!("Bearer {secret}");
    call(flat_api(), "GET", uri, &[("authorization", &bearer)], None).await
}

/// `GET uri` on the flat tree under the fixture's browser session.
pub(crate) async fn get_in_session(fx: &Fixture, uri: &str) -> (StatusCode, Value) {
    call(flat_api(), "GET", uri, &[("cookie", &fx.cookie)], None).await
}

/// The `id` of every element of a JSON array.
pub(crate) fn ids(list: &Value) -> Vec<Uuid> {
    list.as_array()
        .expect("a JSON array")
        .iter()
        .map(|item| {
            item["id"]
                .as_str()
                .and_then(|s| Uuid::parse_str(s).ok())
                .expect("an id")
        })
        .collect()
}
