//! The `?environment=` parameter of the staff verify and read-back routes,
//! and who may name an environment that is not production.
//!
//! Absent, the parameter means production and nothing here runs: every route
//! that takes it answers as it did before app environments existed. Present
//! and not production, two rules apply before anything is read or queued:
//!
//! 1. **A publish token is refused.** A token is a machine credential scoped
//!    to shipping a build and reading the app's own console; a non-production
//!    environment is a staff tool, and no token operates one — whoever minted
//!    it. Refused before the reach lookup, so the token's user holding reach
//!    does not matter.
//! 2. **The caller must be able to open the app's non-production
//!    environments** — oxy-authz `Action::AppNonProduction`, through
//!    `may_open_non_production`: the rule that serves a staging host and
//!    admits a staging `/fn`. The mount's own guards (`manage_apps`, the
//!    grant's org scope) have already run; this is the second, narrower door.
//!
//! Errors on the routes added for environments are
//! `{"error":"<code>","message":"<text>"}` ([`ScopeError`]); the codes are the
//! contract in `internal-docs/custom-app-sandboxes.md` §5.4.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use entity::apps;
use oxy::database::client::establish_connection;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::{AppPublishTokenAuth, AuthenticatedUser};
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::dto::RunFunctionJobResponse;
use crate::server::api::custom_apps_env_resolve::may_open_non_production;
use crate::server::api::custom_apps_functions::FunctionJobTrigger;
use crate::server::api::custom_apps_functions::check_run::{self, Asked, TriggerError};

/// `?environment=<name>` on a verify or read-back route.
#[derive(Debug, Default, serde::Deserialize)]
pub struct EnvironmentQuery {
    #[serde(default)]
    pub environment: Option<String>,
}

/// A refusal with a machine-readable code beside its message.
#[derive(Debug)]
pub struct ScopeError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ScopeError {
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// A failure that is the server's: logged with its cause, answered without it.
    pub(crate) fn internal(context: &str, cause: impl std::fmt::Display) -> Self {
        tracing::error!("{context}: {cause}");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the request could not be completed; retry shortly",
        )
    }

    pub(crate) fn app_not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "app_not_found", "no such app")
    }

    /// A publish token asking for the non-production environment `environment`.
    pub(crate) fn publish_token_refused(environment: impl std::fmt::Display) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "publish_token_refused",
            format!(
                "a publish token cannot act in the {environment} environment; use a staff \
                 login token or API key"
            ),
        )
    }
}

/// Who asks: the caller, and the publish token the request rode in on when it
/// rode one.
pub(crate) struct Caller<'a> {
    pub user: &'a AuthenticatedUser,
    pub marker: Option<&'a AppPublishTokenAuth>,
}

impl<'a> Caller<'a> {
    pub(crate) fn new(
        user: &'a AuthenticatedUser,
        marker: Option<&'a axum::Extension<AppPublishTokenAuth>>,
    ) -> Self {
        Self {
            user,
            marker: marker.map(|axum::Extension(marker)| marker),
        }
    }

    /// Whether this caller may read `app`'s non-production rows: never a
    /// publish token, otherwise whoever oxy-authz lets open the app's
    /// non-production environments (`Action::AppNonProduction`).
    ///
    /// Never a sandbox agent token either: it reads one named sandbox of its
    /// own (`agent_scope`), not "every environment", and the decision asked
    /// here names none.
    pub(crate) async fn has_reach(&self, db: &DatabaseConnection, app: &apps::Model) -> bool {
        self.marker.is_none()
            && may_open_non_production(db, &crate::server::authz::Caller::from_user(self.user), app)
                .await
    }
}

/// What a route that predates the error codes answers: the bare status it
/// always answered with, or a coded refusal it gained with `?environment=`.
/// Every error such a route already returned keeps its shape.
#[derive(Debug)]
pub enum RouteError {
    Bare(StatusCode),
    Coded(ScopeError),
}

impl From<StatusCode> for RouteError {
    fn from(status: StatusCode) -> Self {
        Self::Bare(status)
    }
}

impl From<ScopeError> for RouteError {
    /// A server failure stays the bare `500` these routes have always
    /// answered; a refusal carries its code.
    fn from(e: ScopeError) -> Self {
        if e.status.is_server_error() {
            Self::Bare(e.status)
        } else {
            Self::Coded(e)
        }
    }
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        match self {
            Self::Bare(status) => status.into_response(),
            Self::Coded(e) => e.into_response(),
        }
    }
}

impl IntoResponse for ScopeError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.code, "message": self.message })),
        )
            .into_response()
    }
}

pub(crate) async fn connect() -> Result<DatabaseConnection, ScopeError> {
    establish_connection()
        .await
        .map_err(|e| ScopeError::internal("database connection failed", e))
}

/// The app `id` names, or `404 app_not_found`.
pub(crate) async fn load_app(db: &DatabaseConnection, id: Uuid) -> Result<apps::Model, ScopeError> {
    apps::Entity::find_by_id(id)
        .one(db)
        .await
        .map_err(|e| ScopeError::internal("app lookup failed", e))?
        .ok_or_else(ScopeError::app_not_found)
}

/// The environment `raw` names: production when absent or blank, otherwise a
/// name `AppEnvironment::parse` accepts. Names are exact — `Staging` is not
/// `staging` — so a typo is a `400`, never a silent read of production.
pub(crate) fn parse(raw: Option<&str>) -> Result<AppEnvironment, ScopeError> {
    let Some(name) = raw.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(AppEnvironment::Production);
    };
    AppEnvironment::parse(name).ok_or_else(|| {
        ScopeError::new(
            StatusCode::BAD_REQUEST,
            "invalid_environment",
            format!(
                "{name:?} is not an app environment: use \"production\", \"staging\" or \
                 \"dev-<handle>\""
            ),
        )
    })
}

/// Whether a stored or parsed environment name is production's. Anything
/// else — `staging`, a `dev-<handle>`, or a name this build does not know — is
/// treated as non-production, which is the guarded side.
pub(crate) fn is_production(environment: &str) -> bool {
    environment == "production"
}

/// Refuse the non-production environment named `environment` to a caller
/// oxy-authz does not let open one for this app. Production passes: the
/// mount's own guard decided it.
pub(crate) async fn require_reach(
    db: &DatabaseConnection,
    app: &apps::Model,
    user: &AuthenticatedUser,
    environment: &str,
) -> Result<(), ScopeError> {
    // A sandbox agent token reads only a sandbox it created: production does
    // not pass for it, and the refusal is a not-found.
    if super::agent_scope::is_agent(user) {
        return super::agent_scope::require_own_named(db, app, user, environment).await;
    }
    // The request's user with the credential it arrived with: a token that
    // carries no staff standing opens no non-production environment.
    let caller = crate::server::authz::Caller::from_user(user);
    if is_production(environment) || may_open_non_production(db, &caller, app).await {
        return Ok(());
    }
    Err(ScopeError::new(
        StatusCode::FORBIDDEN,
        "non_production_refused",
        format!(
            "the {environment} environment is open to Oxy staff who may open this app's \
             non-production environments"
        ),
    ))
}

/// The environment `raw` names (production when absent), for a caller who may act in it.
pub(crate) async fn resolve(
    db: &DatabaseConnection,
    app: &apps::Model,
    user: &AuthenticatedUser,
    marker: Option<&AppPublishTokenAuth>,
    raw: Option<&str>,
) -> Result<AppEnvironment, ScopeError> {
    let environment = parse(raw)?;
    admit_to(db, app, &Caller { user, marker }, &environment).await?;
    Ok(environment)
}

/// Refuse `environment`, when it is not production, to a publish token —
/// before the reach lookup, so the token's user holding reach does not
/// matter — and to a caller without reach. Production passes.
pub(crate) async fn admit_to(
    db: &DatabaseConnection,
    app: &apps::Model,
    caller: &Caller<'_>,
    environment: &AppEnvironment,
) -> Result<(), ScopeError> {
    // Asked first: production passes for everyone below, and it is not a
    // sandbox agent token's. The token acts in a sandbox it created, or is
    // answered not-found.
    if super::agent_scope::is_agent(caller.user) {
        return super::agent_scope::require_own(db, app, caller.user, environment).await;
    }
    if *environment == AppEnvironment::Production {
        return Ok(());
    }
    if caller.marker.is_some() {
        return Err(ScopeError::publish_token_refused(environment));
    }
    require_reach(db, app, caller.user, &environment.name()).await
}

impl From<TriggerError> for ScopeError {
    fn from(e: TriggerError) -> Self {
        let message = e.to_string();
        match e {
            TriggerError::AppNotFound => Self::app_not_found(),
            TriggerError::NoBuild(_) => {
                Self::new(StatusCode::NOT_FOUND, "environment_has_no_build", message)
            }
            TriggerError::FunctionNotFound(_) => {
                Self::new(StatusCode::NOT_FOUND, "function_not_found", message)
            }
            TriggerError::NotACheck { .. } => {
                Self::new(StatusCode::FORBIDDEN, "not_a_check", message)
            }
            TriggerError::Enqueue(_) | TriggerError::Db(_) => {
                Self::internal("function job trigger failed", message)
            }
        }
    }
}

/// App `id` and the environment `raw` names, for a caller who may act in it:
/// [`load_app`] then [`resolve`].
pub(crate) async fn admit(
    db: &DatabaseConnection,
    id: Uuid,
    user: &AuthenticatedUser,
    marker: Option<&AppPublishTokenAuth>,
    raw: &str,
) -> Result<(apps::Model, AppEnvironment), ScopeError> {
    let app = load_app(db, id).await?;
    let environment = resolve(db, &app, user, marker, Some(raw)).await?;
    Ok((app, environment))
}

/// Queue a run of function `name` of `app` in `environment`, which the caller
/// has already been admitted to ([`resolve`]). The body of
/// `handlers::run_function_job` when `?environment=` is present.
///
/// `asked_by_token` is the sandbox agent token the request used, when that is
/// its credential: it rides the task, and the worker admits it again before
/// the run starts.
pub(crate) async fn run_in_environment(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    name: &str,
    input: Option<serde_json::Value>,
    asked_by_token: Option<Uuid>,
) -> Result<RunFunctionJobResponse, ScopeError> {
    let asked = Asked {
        trigger: FunctionJobTrigger::Manual,
        environment,
        credential_token_id: asked_by_token,
    };
    let run_id = check_run::trigger(db, app.id, name, input, asked).await?;
    Ok(RunFunctionJobResponse {
        run_id,
        environment: environment.name(),
    })
}

#[cfg(test)]
#[path = "environment_scope_tests.rs"]
mod tests;
