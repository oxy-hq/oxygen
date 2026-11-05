//! The one cached prelude for every app-keyed custom-app surface.
//!
//! Bundle bytes (`serve_pretty`), function invocation (`/fn`), health, debug,
//! logs and availability all start the same way: authenticate, load the
//! user, turn `(org_slug, app_slug)` into rows, decide whether this caller may
//! open this app. The bundle path had every step cached and the others ran
//! two uncached row reads per call; this module is the single resolver both
//! take, so a step cached here is cached for all of them, and a step added
//! here is added once.
//!
//! The chain, in order — the ORDER is a contract, not a convenience:
//!
//! 1. **Authenticate**, before any database read. An anonymous probe must not
//!    learn whether an `(org, app)` pair exists.
//! 2. **User**, from the 60 s cache keyed by who the credential names
//!    (`custom_apps_auth::user_cache_key`). A sandbox agent token bypasses it
//!    in both directions: its minter's row is read now, so a change to the
//!    minter is seen by the next request.
//! 3. **Rows**, from the `(org_slug, app_slug)` cache; a miss costs the org,
//!    the app and the environment builds, together, and is cached together.
//! 4. **Existence to this caller.** A token with no grant on the app's
//!    workspace, or a sandbox agent token whose grants do not name this app,
//!    is told the app does not exist — the same answer an unknown slug gets.
//! 5. **Access**, from the `(user_id, app_id)` verdict cache: the combined
//!    (member | grant | staff) rule for production, the staff draft rule for
//!    any other environment. A token that narrows its bearer is never cached.
//! 6. **Staff reach**, `platform_reaches(DevelopApps, org)`, which is itself
//!    memoised per address in `oxy_server_authz::globals` for the same 60 s,
//!    so it costs no query on a warm replica and needs no second cache here.
//!
//! Warm steady state is **zero** database round trips for every caller but a
//! sandbox agent token (one, by design).
//!
//! What a refusal carries is what the two callers need to answer it in their
//! own voice: `serve_pretty` redirects an unauthenticated browser to login,
//! answers `no_store_404` and records a 403 against the app for the SLI;
//! `authenticate_and_authorize` maps the same refusals to bare statuses.

use entity::prelude::{Apps, Organizations};
use entity::{apps, organizations};
use http::HeaderMap;
use oxy::database::client::establish_connection;
use oxy_auth::authenticator::Authenticator;
use oxy_auth::built_in::BuiltInAuthenticator;
use oxy_auth::token::{CredentialContext, SandboxAgent};
use oxy_auth::types::{AuthenticatedUser, Identity};
use oxy_auth::user::UserService;
use oxy_server_authz::Caller;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use tracing::Instrument;
use uuid::Uuid;

use oxy_app_core::custom_app_environment::AppEnvironment;

use crate::agent;
use crate::auth::{user_cache_key, user_can_access_app};
use crate::cache::{
    ResolvedApp, cached_app_resolution, cached_user, set_cached_app_resolution, set_cached_user,
};
use crate::env_resolve::{EnvironmentBuilds, load_environment_builds, may_open_environment};

/// A request resolved to the app it addresses and the caller who may open it.
pub struct AppRequest {
    /// The user, with the credential the request arrived with attached.
    pub user: AuthenticatedUser,
    pub caller: Caller,
    pub org: organizations::Model,
    pub app: apps::Model,
    /// The builds the app's environments serve, read with the app row.
    pub environments: EnvironmentBuilds,
    /// The connection the resolver opened — reused by the caller so a request
    /// pays for one pool checkout, not two.
    pub db: DatabaseConnection,
    /// `platform_reaches(DevelopApps, org)`: the same call the access check
    /// makes, so a staff viewer is never "allowed but flagged a customer".
    pub is_staff: bool,
}

/// Why the chain stopped. Each variant is one HTTP answer; what it carries is
/// what the surface needs to record or explain it.
pub enum Refusal {
    /// No usable credential. The authenticator's error, for the operator's
    /// log line ("no cookie" vs "stale JWT" vs "wrong domain").
    Unauthenticated(String),
    /// A credential that names nobody (a deleted user, a dangling token).
    UnknownUser,
    /// No such org, no such app — or none *to this caller*.
    NotFound,
    /// Known app, known user, no access. The app is carried so the serve
    /// path can record the 403 against it.
    Forbidden(Box<Denied>),
    /// A platform failure mid-chain, logged where it happened.
    Internal(Box<Failure>),
}

/// The app a known user was refused.
pub struct Denied {
    pub app: apps::Model,
    pub user_id: Uuid,
}

/// A failed step. `app` and `user_id` are set when the chain had already
/// resolved them, so the serve path's SLI can attribute the 500.
pub struct Failure {
    pub app: Option<apps::Model>,
    pub user_id: Option<Uuid>,
}

impl Refusal {
    /// The bare status: what `authenticate_and_authorize` answers.
    pub fn status(&self) -> http::StatusCode {
        use http::StatusCode;
        match self {
            Refusal::Unauthenticated(_) | Refusal::UnknownUser => StatusCode::UNAUTHORIZED,
            Refusal::NotFound => StatusCode::NOT_FOUND,
            Refusal::Forbidden(_) => StatusCode::FORBIDDEN,
            Refusal::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn internal(app: Option<&apps::Model>, user_id: Option<Uuid>) -> Self {
        Refusal::Internal(Box::new(Failure {
            app: app.cloned(),
            user_id,
        }))
    }
}

/// Resolve `(org_slug, app_slug)` for the caller behind `headers`, for the
/// `environment` the request addresses (production decides access by the
/// customer rule, anything else by the staff rule).
///
/// `sandbox_agent` is the surface's answer to a sandbox agent token: `/fn`
/// admits one, the bundle path and the operator surfaces refuse it.
pub async fn resolve_app_request(
    headers: &HeaderMap,
    org_slug: &str,
    app_slug: &str,
    sandbox_agent: SandboxAgent,
    environment: &AppEnvironment,
) -> Result<AppRequest, Refusal> {
    let (identity, credential) = authenticate(headers, sandbox_agent).await?;
    let user = principal(&identity, credential).await?;
    let caller = Caller::from_user(&user);

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("DB connection failed serving custom app {org_slug}/{app_slug}: {e}");
        Refusal::internal(None, Some(user.id))
    })?;
    let ResolvedApp {
        org,
        app,
        environments,
    } = resolve_rows(&db, org_slug, app_slug).await?;

    // An API token with no grant on the app's workspace answers exactly as an
    // unknown app does, so it cannot probe which apps exist; a sandbox agent
    // token's workspace grant stands for the apps it names, not for every app
    // of the workspace.
    if caller
        .workspace_ceiling(app.org_id, app.project_id)
        .is_none()
        || !agent::admits_app(&caller, &app)
    {
        return Err(Refusal::NotFound);
    }

    if !authorize(&db, &caller, &app, environment).await? {
        return Err(Refusal::Forbidden(Box::new(Denied {
            user_id: user.id,
            app,
        })));
    }

    // Literally the same call, capability and scope, as the access check's
    // staff term — the two drifting apart is a staff viewer silently losing
    // draft previews.
    let is_staff = oxy_server_authz::globals::platform_reaches(
        &db,
        &caller,
        oxy_authz::Cap::DevelopApps,
        app.org_id,
    )
    .await;

    Ok(AppRequest {
        user,
        caller,
        org,
        app,
        environments,
        db,
        is_staff,
    })
}

/// Step 1: the credential, before any database read.
async fn authenticate(
    headers: &HeaderMap,
    sandbox_agent: SandboxAgent,
) -> Result<(Identity, Option<CredentialContext>), Refusal> {
    BuiltInAuthenticator::new(sandbox_agent)
        .authenticate_with_credential(headers)
        .instrument(tracing::info_span!(
            target: "custom_apps_serve",
            "custom_app_authenticate"
        ))
        .await
        .map_err(|e| Refusal::Unauthenticated(e.to_string()))
}

/// Step 2: the user the credential names, through the 60 s cache — except
/// for a sandbox agent token, whose minter is read now (`custom_apps_agent`).
/// The credential is attached AFTER the cache: the cached row is the user,
/// never the credential one request happened to arrive with.
async fn principal(
    identity: &Identity,
    credential: Option<CredentialContext>,
) -> Result<AuthenticatedUser, Refusal> {
    oxy_app_core::usage_probe::seen(credential.as_ref());
    let user = if agent::is_agent(credential.as_ref()) {
        // Boxed, so the token's branch adds a pointer to this future, which
        // the function route awaits inline.
        Box::pin(agent::fresh_user(identity))
            .await
            .map_err(|e| {
                tracing::error!("user lookup failed: {e}");
                Refusal::internal(None, None)
            })?
            .ok_or(Refusal::UnknownUser)?
    } else {
        cached_or_loaded_user(identity).await?
    };
    Ok(user.with_credential(credential))
}

async fn cached_or_loaded_user(identity: &Identity) -> Result<AuthenticatedUser, Refusal> {
    let cache_key = user_cache_key(identity);
    if let Some(user) = cached_user(&cache_key) {
        return Ok(user);
    }
    let user = UserService::find_user_by_identity(identity)
        .instrument(tracing::info_span!(
            target: "custom_apps_serve",
            "custom_app_user_lookup"
        ))
        .await
        .map_err(|e| {
            tracing::error!("user lookup failed: {e}");
            Refusal::internal(None, None)
        })?
        .ok_or(Refusal::UnknownUser)?;
    set_cached_user(cache_key, user.clone());
    Ok(user)
}

/// Step 3: the rows, through the `(org_slug, app_slug)` cache. A miss is not
/// cached — a newly created app is reachable at once.
async fn resolve_rows(
    db: &DatabaseConnection,
    org_slug: &str,
    app_slug: &str,
) -> Result<ResolvedApp, Refusal> {
    if let Some(resolved) = cached_app_resolution(org_slug, app_slug) {
        return Ok(resolved);
    }
    let org = Organizations::find()
        .filter(organizations::Column::Slug.eq(org_slug))
        .one(db)
        .instrument(tracing::info_span!(
            target: "custom_apps_serve",
            "custom_app_resolve_org"
        ))
        .await
        .map_err(|e| {
            tracing::error!("Failed to look up org {org_slug}: {e}");
            Refusal::internal(None, None)
        })?
        .ok_or(Refusal::NotFound)?;
    let app = Apps::find()
        .filter(apps::Column::OrgId.eq(org.id))
        .filter(apps::Column::Slug.eq(app_slug))
        .one(db)
        .instrument(tracing::info_span!(
            target: "custom_apps_serve",
            "custom_app_resolve_app"
        ))
        .await
        .map_err(|e| {
            tracing::error!("Failed to look up custom app {org_slug}/{app_slug}: {e}");
            Refusal::internal(None, None)
        })?
        .ok_or(Refusal::NotFound)?;
    let environments = load_environment_builds(db, &app)
        .instrument(tracing::info_span!(
            target: "custom_apps_serve",
            "custom_app_resolve_environments"
        ))
        .await
        .map_err(|e| {
            tracing::error!(
                "Failed to read the environments of custom app {org_slug}/{app_slug}: {e}"
            );
            Refusal::internal(Some(&app), None)
        })?;
    let resolved = ResolvedApp {
        org,
        app,
        environments,
    };
    set_cached_app_resolution(org_slug, app_slug, resolved.clone());
    Ok(resolved)
}

/// Step 5: may this caller open this environment of the app? Production: the
/// combined access check (org member | grant | staff), cached per
/// `(user_id, app_id)` — see `custom_apps_auth::user_can_access_app`. Any
/// other environment: Oxy staff reaching the org, decided by oxy-authz — an
/// unreleased build is never a customer's to open.
///
/// A sandbox agent token differs on the second arm only: any other environment
/// is decided for **that** environment, uncached (`may_open_environment`) — a
/// `dev-*` sandbox it created, or the staging it was granted, and nothing else.
/// Its production arm is the ordinary one, because `/fn` resolves at
/// production for every caller and decides the environment it runs in at
/// `environment_gate`; the page path, which resolves at the environment it
/// serves, refuses the token at production itself (`ServeContext::prepare`).
async fn authorize(
    db: &DatabaseConnection,
    caller: &Caller,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<bool, Refusal> {
    let access = async {
        if *environment == AppEnvironment::Production {
            user_can_access_app(db, caller, app).await
        } else {
            // For every caller but a sandbox agent token this is
            // `may_open_non_production`, unchanged.
            Ok(may_open_environment(db, caller, app, environment).await)
        }
    };
    access
        .instrument(tracing::info_span!(
            target: "custom_apps_serve",
            "custom_app_authorize"
        ))
        .await
        .map_err(|e| {
            tracing::error!("Access check failed for custom app {}: {e}", app.id);
            Refusal::internal(Some(app), Some(caller.user_id))
        })
}
