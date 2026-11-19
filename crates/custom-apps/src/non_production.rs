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
type DecisionCache = crate::cache::TtlLru<(Uuid, Uuid), bool>;

fn non_production_cache() -> &'static DecisionCache {
    static CACHE: std::sync::OnceLock<DecisionCache> = std::sync::OnceLock::new();
    CACHE.get_or_init(crate::cache::standard_cache)
}

/// May this viewer open a **non-production** environment of `app`? Oxy staff
/// holding `develop_apps` over the app's org, decided by oxy-authz
/// (`Action::AppNonProduction`) with today's staff draft-preview gate as the
/// `existing_allow`, so the model can only narrow it. Cached 60 s.
///
/// Asked of the [`Caller`](oxy_server_authz::Caller): an API token opens a
/// non-production environment only with the staff standing it carries, inside
/// the orgs it covers. The cache holds a session's verdict per (user, app), so
/// a token that narrows its bearer neither reads it nor writes it.
pub async fn may_open_non_production(
    db: &sea_orm::DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
) -> bool {
    if caller.reach().is_some_and(oxy_authz::TokenReach::narrows) {
        return decide_non_production(db, caller, app).await;
    }
    let key = (caller.user_id, app.id);
    if let Some(allowed) = non_production_cache().get(&key) {
        return allowed;
    }
    let allowed = decide_non_production(db, caller, app).await;
    non_production_cache().insert(key, allowed);
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
    caller: &oxy_server_authz::Caller,
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

/// [`may_open_environment`], telling a refusal from a failed read: `Err` when
/// the platform grant, the environment's row or the caller's facts could not
/// be read. The same decision otherwise: `Action::AppNonProduction` over the
/// staff gate, with a sandbox agent token's environment facet.
///
/// It shares [`may_open_non_production`]'s 60 s cache on the same terms (a
/// caller whose reach does not narrow, and who is no sandbox agent), but
/// trusts only a cached **allow**: that door caches whatever it decided,
/// including the refusal a failed read becomes there, so a cached refusal may
/// be a blip and is decided again here, where a failed read is an `Err`. A
/// verdict reached here is written back; a failed read is not. For the
/// AppCall door (`ctx.apps.call`, a send's check) and a queued send's
/// delivery, which retries a blip rather than kill the send
/// (`app_function_executor::sender`).
pub async fn may_open_environment_checked(
    db: &sea_orm::DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<bool, DbErr> {
    let cacheable =
        !caller.is_sandbox_agent() && !caller.reach().is_some_and(oxy_authz::TokenReach::narrows);
    let key = (caller.user_id, app.id);
    if cacheable && non_production_cache().get(&key) == Some(true) {
        return Ok(true);
    }
    let allowed = decide_checked(db, caller, app, environment).await?;
    if cacheable {
        non_production_cache().insert(key, allowed);
    }
    Ok(allowed)
}

/// [`may_open_environment_checked`]'s decision, uncached.
async fn decide_checked(
    db: &sea_orm::DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<bool, DbErr> {
    let facet = if caller.is_sandbox_agent() {
        Some(environment_facet(db, app, environment).await?)
    } else {
        None
    };
    let existing_allow = oxy_server_authz::globals::platform_reaches_checked(
        db,
        caller,
        oxy_authz::Cap::DevelopApps,
        app.org_id,
    )
    .await?;
    let facts = oxy_server_authz::loader::load_principal_facts(db, caller)
        .await
        .ok_or_else(|| DbErr::Custom("the caller's facts could not be read".into()))?;
    Ok(oxy_authz::enforce(
        "custom_app_non_production",
        &facts,
        oxy_authz::Action::AppNonProduction,
        &resource_in(app, facet),
        existing_allow,
    ))
}

/// The resource a non-production decision is about: the app, in the
/// environment `facet` names when one does.
fn resource_in(app: &apps::Model, facet: Option<oxy_authz::EnvFacet>) -> oxy_authz::Resource {
    let resource = oxy_authz::Resource::app(app.id, app.org_id);
    match facet {
        Some(facet) => resource.in_environment(facet),
        None => resource,
    }
}

/// [`may_open_environment`] for the two routes that name no one sandbox yet:
/// listing an app's environments, and creating one.
pub async fn may_open_new_sandbox(
    db: &sea_orm::DatabaseConnection,
    caller: &oxy_server_authz::Caller,
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
pub async fn environment_facet<C: ConnectionTrait>(
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
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
) -> bool {
    decide_in(db, caller, app, None).await
}

/// The decision, for the environment `facet` names — or for none, which a
/// sandbox agent token is refused.
async fn decide_in(
    db: &sea_orm::DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
    facet: Option<oxy_authz::EnvFacet>,
) -> bool {
    let resource = resource_in(app, facet);
    let existing_allow = oxy_server_authz::globals::platform_reaches(
        db,
        caller,
        oxy_authz::Cap::DevelopApps,
        app.org_id,
    )
    .await;
    oxy_server_authz::enforce_for(
        db,
        caller,
        "custom_app_non_production",
        oxy_authz::Action::AppNonProduction,
        resource,
        existing_allow,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection that is never reached: every statement fails, as when the
    /// database is unreachable.
    async fn unreachable_db() -> sea_orm::DatabaseConnection {
        let mut options = sea_orm::ConnectOptions::new("postgres://probe:probe@127.0.0.1:1/probe");
        options
            .connect_lazy(true)
            .acquire_timeout(std::time::Duration::from_millis(500));
        sea_orm::Database::connect(options)
            .await
            .expect("a lazy pool")
    }

    fn app(id: Uuid) -> apps::Model {
        let now = chrono::Utc::now().fixed_offset();
        apps::Model {
            visibility: "org".to_string(),
            id,
            slug: "x".to_string(),
            name: "X".to_string(),
            org_id: Uuid::from_u128(0x0A),
            project_id: Uuid::from_u128(0x0B),
            branch: "main".to_string(),
            source_repo: String::new(),
            status: "created".to_string(),
            source_type: "s3".to_string(),
            source_config: serde_json::json!({}),
            bootstrap_pr_url: None,
            last_synced_at: None,
            manifest_override: None,
            published_at: None,
            repo_path: None,
            last_promoted_by: None,
            last_promoted_at: None,
            require_second_person_to_promote: false,
            created_at: now,
            updated_at: now,
        }
    }

    fn caller(id: Uuid) -> oxy_server_authz::Caller {
        oxy_server_authz::Caller::from_user(&oxy_auth::types::AuthenticatedUser {
            id,
            email: Some("ada@example.com".into()),
            name: "Ada".into(),
            picture: None,
            status: entity::users::UserStatus::Active,
            credential: None,
        })
    }

    /// The AppCall door outside production answers from the route door's 60 s
    /// verdict cache before it reads anything, so a staging page whose function
    /// calls on loads no facts on a hit; with no verdict cached, a read that
    /// fails is an `Err`, never a refusal, and leaves nothing in the cache.
    #[tokio::test]
    async fn the_checked_door_answers_from_the_shared_cache_before_reading() {
        let db = unreachable_db().await;
        let (user, app) = (Uuid::new_v4(), app(Uuid::new_v4()));
        let caller = caller(user);
        let staging = AppEnvironment::Staging;

        assert!(
            may_open_environment_checked(&db, &caller, &app, &staging)
                .await
                .is_err(),
            "no verdict cached: the read is tried and its failure is an error"
        );
        assert_eq!(
            non_production_cache().get(&(user, app.id)),
            None,
            "a failed read is never cached"
        );

        non_production_cache().insert((user, app.id), true);
        assert_eq!(
            may_open_environment_checked(&db, &caller, &app, &staging)
                .await
                .expect("answered from the cache, without a read"),
            true
        );
    }

    /// A refusal in the shared cache may be the route door's reading of a
    /// failed read, so the checked door decides again rather than trust it:
    /// a blip stays an `Err` (a send's delivery retries it), never the
    /// refusal that would kill the send for good.
    #[tokio::test]
    async fn the_checked_door_decides_again_over_a_cached_refusal() {
        let db = unreachable_db().await;
        let (user, app) = (Uuid::new_v4(), app(Uuid::new_v4()));
        let caller = caller(user);

        non_production_cache().insert((user, app.id), false);
        assert!(
            may_open_environment_checked(&db, &caller, &app, &AppEnvironment::Staging)
                .await
                .is_err(),
            "a cached refusal is decided again, and the failed read is an error"
        );
    }
}
