//! Who may call each route, decided by oxy-authz (module docs, **Who**).
//!
//! Authentication is inline, as on the public router's other slug-addressed
//! reads: the built-in authenticator (a sandbox agent token refused), the
//! user through the serve path's cache, the credential attached after it.

use axum::http::HeaderMap;
use entity::{apps, organizations};
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::authenticator::Authenticator;
use oxy_auth::built_in::BuiltInAuthenticator;
use oxy_auth::token::SandboxAgent;
use oxy_auth::types::AuthenticatedUser;
use oxy_authz::{Action, Resource};
use oxy_custom_apps::authenticate::{authenticate_and_authorize, require_app_admin};
use oxy_custom_apps::env_resolve::may_open_non_production;
use oxy_server_authz::Caller;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::reply::ApiError;
use crate::app_events::enabled_for;

/// A caller admitted to an org-wide route.
pub(super) struct OrgAdmitted {
    pub db: DatabaseConnection,
    pub org_id: Uuid,
    pub actor: RequestActor,
}

/// A caller admitted to a per-app route.
pub(super) struct AppAdmitted {
    pub db: DatabaseConnection,
    pub app: apps::Model,
    /// The environment a read was admitted to (production for a change).
    pub environment: AppEnvironment,
    pub actor: Option<RequestActor>,
}

/// An org-wide read: `Action::AppAdmin` over the org.
pub(super) async fn org_reader(
    headers: &HeaderMap,
    org_slug: &str,
) -> Result<OrgAdmitted, ApiError> {
    org_door(headers, org_slug, Action::AppAdmin).await
}

/// An org-wide change: `Action::AppEventsManage` over the org, with the org's
/// flag on.
pub(super) async fn org_manager(
    headers: &HeaderMap,
    org_slug: &str,
) -> Result<OrgAdmitted, ApiError> {
    let admitted = org_door(headers, org_slug, Action::AppEventsManage).await?;
    flag_on(admitted.org_id)?;
    Ok(admitted)
}

/// A per-app read, by the gate `GET …/invocations` takes: the shared auth
/// flow, the app-admin ring, and non-production reach for an environment other
/// than production.
///
/// `environment` is read once the caller is admitted to the app, so a typo
/// says nothing to a caller who may not ask.
pub(super) async fn app_reader(
    headers: &HeaderMap,
    org_slug: &str,
    app_slug: &str,
    environment: Option<&str>,
) -> Result<AppAdmitted, ApiError> {
    let outcome = authenticate_and_authorize(headers, org_slug, app_slug, SandboxAgent::Refuse)
        .await
        .map_err(ApiError::from_status)?;
    require_app_admin(&outcome)
        .await
        .map_err(ApiError::from_status)?;
    let environment = super::reply::environment(environment)?;
    let db = connect().await?;
    if environment != AppEnvironment::Production
        && !may_open_non_production(&db, &outcome.caller, &outcome.app).await
    {
        return Err(ApiError::Forbidden("non_production_refused"));
    }
    Ok(AppAdmitted {
        db,
        app: outcome.app,
        environment,
        actor: None,
    })
}

/// A per-app change: `Action::AppEventsManage` over the app, with the org's
/// flag on.
pub(super) async fn app_manager(
    headers: &HeaderMap,
    org_slug: &str,
    app_slug: &str,
) -> Result<AppAdmitted, ApiError> {
    let user = authenticate(headers).await?;
    let db = connect().await?;
    let org = org_by_slug(&db, org_slug).await?;
    let app = apps::Entity::find()
        .filter(apps::Column::OrgId.eq(org.id))
        .filter(apps::Column::Slug.eq(app_slug))
        .one(&db)
        .await
        .map_err(|e| ApiError::db("read the app", e))?
        .ok_or_else(|| ApiError::NotFound("app_not_found", format!("no app `{app_slug}`")))?;
    let resource = Resource::app(app.id, app.org_id).published_from(app.project_id);
    decide(&db, &user, Action::AppEventsManage, &resource).await?;
    flag_on(org.id)?;
    Ok(AppAdmitted {
        db,
        app,
        environment: AppEnvironment::Production,
        actor: Some(actor(user, headers)),
    })
}

/// Whether the request's caller may manage `app`'s events: the decision
/// `app_manager` takes, for another app than the one the route names.
pub(super) async fn may_manage(
    headers: &HeaderMap,
    db: &DatabaseConnection,
    app: &apps::Model,
) -> bool {
    let Ok(user) = authenticate(headers).await else {
        return false;
    };
    let resource = Resource::app(app.id, app.org_id).published_from(app.project_id);
    decide(db, &user, Action::AppEventsManage, &resource)
        .await
        .is_ok()
}

async fn org_door(
    headers: &HeaderMap,
    org_slug: &str,
    action: Action,
) -> Result<OrgAdmitted, ApiError> {
    let user = authenticate(headers).await?;
    let db = connect().await?;
    let org = org_by_slug(&db, org_slug).await?;
    decide(&db, &user, action, &Resource::org(org.id)).await?;
    Ok(OrgAdmitted {
        db,
        org_id: org.id,
        actor: actor(user, headers),
    })
}

/// The user behind the request's credential, the credential attached.
async fn authenticate(headers: &HeaderMap) -> Result<AuthenticatedUser, ApiError> {
    let (identity, credential) = BuiltInAuthenticator::new(SandboxAgent::Refuse)
        .authenticate_with_credential(headers)
        .await
        .map_err(|e| ApiError::Unauthenticated(e.to_string()))?;
    let user = oxy_custom_apps::gate::gate_user(&identity)
        .await
        .map_err(|_| ApiError::Unauthenticated("the credential names nobody".into()))?;
    Ok(user.with_credential(credential))
}

/// The one decision: `action` over `resource`, from the caller's facts. Facts
/// that cannot be loaded refuse (fail closed): there is no shipped check here
/// to fall back to.
async fn decide(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    action: Action,
    resource: &Resource,
) -> Result<(), ApiError> {
    let caller = Caller::from_user(user);
    let facts = oxy_server_authz::loader::load_principal_facts_scoped(db, &caller, false)
        .await
        .ok_or_else(|| ApiError::Internal("could not load who you are; retry shortly".into()))?;
    oxy_authz::require(&facts, action, resource).map_err(|_| ApiError::Forbidden("forbidden"))
}

fn flag_on(org_id: Uuid) -> Result<(), ApiError> {
    if enabled_for(org_id) {
        Ok(())
    } else {
        Err(ApiError::not_enabled())
    }
}

async fn connect() -> Result<DatabaseConnection, ApiError> {
    establish_connection().await.map_err(|e| {
        tracing::error!(error = %e, "app events api: db connect failed");
        ApiError::Internal("could not reach the database; retry shortly".into())
    })
}

async fn org_by_slug(
    db: &DatabaseConnection,
    org_slug: &str,
) -> Result<organizations::Model, ApiError> {
    organizations::Entity::find()
        .filter(organizations::Column::Slug.eq(org_slug))
        .one(db)
        .await
        .map_err(|e| ApiError::db("read the org", e))?
        .ok_or_else(|| ApiError::NotFound("org_not_found", format!("no org `{org_slug}`")))
}

/// The request's actor for its audit row: the user, the key or token it came
/// with, and the request's own context.
fn actor(user: AuthenticatedUser, headers: &HeaderMap) -> RequestActor {
    let credential = user.credential.clone();
    let mut actor = RequestActor::for_user(user, headers);
    actor.credential = credential;
    actor
}
