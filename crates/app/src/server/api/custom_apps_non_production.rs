//! May this caller open a **non-production** environment of a custom app?
//!
//! One decision, `Action::AppNonProduction`, asked three ways:
//!
//! - [`may_open_non_production`]: of the app as a whole. Staff holding
//!   `develop_apps` over the app's org; cached for a session.
//! - [`may_open_environment`]: of one named environment. The same answer for
//!   every caller but a sandbox agent token, which holds it only on a sandbox
//!   it created.
//! - [`may_open_new_sandbox`]: of "a sandbox that does not exist yet", for the
//!   list and create routes.
//!
//! Split from `custom_apps_env_resolve`, which keeps resolving what an
//! environment serves and re-exports these.

use entity::{app_environments, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

/// Decisions of [`may_open_non_production`], per `(user_id, app_id)`, for the
/// same 60 s as every other step of the serve chain: a staging page load is the
/// same 30-100 asset requests as a production one, and the decision loads the
/// principal's facts. A revoked grant stops opening staging within a minute,
/// like a revoked membership stops opening production.
type DecisionCache =
    std::sync::RwLock<std::collections::HashMap<(Uuid, Uuid), (bool, std::time::Instant)>>;

fn non_production_cache() -> &'static DecisionCache {
    static CACHE: std::sync::OnceLock<DecisionCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// May this viewer open a **non-production** environment of `app`? Oxy staff
/// holding `develop_apps` over the app's org, decided by oxy-authz
/// (`Action::AppNonProduction`) with today's staff draft-preview gate as the
/// `existing_allow`, so the model can only narrow it. Cached 60 s.
///
/// Asked of the [`Caller`](crate::server::authz::Caller): an API token opens a
/// non-production environment only with the staff standing it carries, inside
/// the orgs it covers. The cache holds a session's verdict per (user, app), so
/// a token that narrows its bearer neither reads it nor writes it.
pub async fn may_open_non_production(
    db: &sea_orm::DatabaseConnection,
    caller: &crate::server::authz::Caller,
    app: &apps::Model,
) -> bool {
    use super::custom_apps_cache::{get_fresh, insert_with_sweep};
    if caller.reach().is_some_and(oxy_authz::TokenReach::narrows) {
        return decide_non_production(db, caller, app).await;
    }
    let key = (caller.user_id, app.id);
    if let Some(allowed) = get_fresh(non_production_cache(), &key) {
        return allowed;
    }
    let allowed = decide_non_production(db, caller, app).await;
    insert_with_sweep(non_production_cache(), key, allowed);
    allowed
}

/// [`may_open_non_production`] for a call site that knows **which**
/// environment it decides for: the loop's routes, which a sandbox agent token
/// may reach.
///
/// For every other caller this is `may_open_non_production`, unchanged and
/// with no extra read: an environment facet changes nothing without such a
/// token. For the token it is uncached, reads the sandbox's row, and holds
/// only on a `dev-*` sandbox the token created itself (sandbox agent
/// credential design §3.2). Production, staging and another creator's sandbox
/// are refused.
pub async fn may_open_environment(
    db: &sea_orm::DatabaseConnection,
    caller: &crate::server::authz::Caller,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> bool {
    if !caller.is_sandbox_agent() {
        return may_open_non_production(db, caller, app).await;
    }
    match environment_facet(db, app, environment).await {
        Ok(facet) => decide_in(db, caller, app, Some(facet)).await,
        Err(e) => {
            tracing::error!(app_id = %app.id, error = %e, "sandbox agent: environment lookup failed");
            false
        }
    }
}

/// [`may_open_environment`] for the two routes that name no one sandbox yet:
/// listing an app's environments, and creating one.
pub async fn may_open_new_sandbox(
    db: &sea_orm::DatabaseConnection,
    caller: &crate::server::authz::Caller,
    app: &apps::Model,
) -> bool {
    if !caller.is_sandbox_agent() {
        return may_open_non_production(db, caller, app).await;
    }
    decide_in(db, caller, app, Some(oxy_authz::EnvFacet::NewSandbox)).await
}

/// What `environment` is to the authorization model: fixed, or a sandbox with
/// the token that created it. A sandbox with no row has no creator, so no
/// token owns it.
pub(crate) async fn environment_facet<C: ConnectionTrait>(
    db: &C,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<oxy_authz::EnvFacet, DbErr> {
    Ok(match environment {
        AppEnvironment::Production => oxy_authz::EnvFacet::Production,
        AppEnvironment::Staging => oxy_authz::EnvFacet::Staging,
        AppEnvironment::Dev { .. } => oxy_authz::EnvFacet::Sandbox {
            created_by_token: sandbox_creator(db, app.id, environment).await?,
        },
    })
}

/// The sandbox agent token that created `environment` of `app_id`, read
/// uncached. `None` for a sandbox a person created, and for one with no row.
async fn sandbox_creator<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
) -> Result<Option<Uuid>, DbErr> {
    Ok(app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Name.eq(environment.name()))
        .one(db)
        .await?
        .and_then(|row| row.created_by_token_id))
}

async fn decide_non_production(
    db: &sea_orm::DatabaseConnection,
    caller: &crate::server::authz::Caller,
    app: &apps::Model,
) -> bool {
    decide_in(db, caller, app, None).await
}

/// The decision, for the environment `facet` names — or for none, which a
/// sandbox agent token is refused.
async fn decide_in(
    db: &sea_orm::DatabaseConnection,
    caller: &crate::server::authz::Caller,
    app: &apps::Model,
    facet: Option<oxy_authz::EnvFacet>,
) -> bool {
    let resource = oxy_authz::Resource::app(app.id, app.org_id);
    let resource = match facet {
        Some(facet) => resource.in_environment(facet),
        None => resource,
    };
    let existing_allow = crate::server::authz::globals::platform_reaches(
        db,
        caller,
        oxy_authz::Cap::DevelopApps,
        app.org_id,
    )
    .await;
    crate::server::authz::enforce_for(
        db,
        caller,
        "custom_app_non_production",
        oxy_authz::Action::AppNonProduction,
        resource,
        existing_allow,
    )
    .await
}
