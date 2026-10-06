//! Which app environment a log read is for, and who may read it.
//!
//! `/logs` returns production's lines unless `?environment=<name>` asks for
//! another app environment's, and then it returns that environment's alone. A
//! non-production environment is a staff tool, so naming one needs the
//! app-admin gate the route already passed **and** `may_open_non_production`
//! (oxy-authz `Action::AppNonProduction`).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use oxy::database::client::establish_connection;
use oxy_app_core::custom_app_environment::AppEnvironment;

use crate::server::api::custom_apps_auth::{AuthOutcome, require_app_admin};
use crate::server::api::custom_apps_env_resolve::{may_open_environment, may_open_non_production};
use crate::server::api::custom_apps_sandbox_instance::{line_is_of_instance, own_since};

/// A refusal on the environment a log read named: a machine-readable code
/// beside its message, the shape the staff read-back routes answer
/// (`admin::apps::environment_scope`, which this surface may not import — see
/// `tests/custom_apps/custom_apps_boundary.rs`).
pub(super) struct EnvironmentRefusal {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl IntoResponse for EnvironmentRefusal {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.code, "message": self.message })),
        )
            .into_response()
    }
}

/// The environment `raw` names: production when absent or blank, otherwise
/// exactly an `AppEnvironment` name. A typo is a `400`, never a silent read
/// of production.
fn requested_environment(raw: Option<&str>) -> Result<AppEnvironment, EnvironmentRefusal> {
    let Some(name) = raw.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(AppEnvironment::Production);
    };
    AppEnvironment::parse(name).ok_or_else(|| EnvironmentRefusal {
        status: StatusCode::BAD_REQUEST,
        code: "invalid_environment",
        message: format!(
            "{name:?} is not an app environment: use \"production\", \"staging\" or \
             \"dev-<handle>\""
        ),
    })
}

/// The environment whose lines `outcome`'s caller reads, as the store takes
/// it, once both gates have passed: the app-admin gate, then
/// [`log_environment`]. A sandbox agent token passes neither as asked of the
/// app; it is asked about the one sandbox it names ([`agent_environment`]).
pub(super) async fn admitted(
    outcome: &AuthOutcome,
    raw: Option<&str>,
) -> Result<Admitted, Response> {
    if outcome.caller.is_sandbox_agent() {
        return agent_environment(outcome, raw).await;
    }
    if let Err(status) = require_app_admin(outcome).await {
        return Err(super::error_response(status, "app-admin required"));
    }
    let environment = log_environment(outcome, raw)
        .await
        .map_err(IntoResponse::into_response)?;
    Ok(Admitted {
        environment,
        since: None,
    })
}

/// What a log read was admitted to.
pub(super) struct Admitted {
    /// The environment, as the store takes it.
    pub environment: String,
    /// For a sandbox agent token, where the sandbox it has now starts: it is
    /// shown the lines written from then on, and none of an earlier sandbox
    /// that had the name (`custom_apps_sandbox_instance`). `None` for every
    /// other caller, who reads the environment's lines as before.
    pub since: Option<DateTime<Utc>>,
}

impl Admitted {
    /// Whether a line with the served `timestamp` is one this read returns.
    pub(super) fn shows(&self, timestamp: &str) -> bool {
        self.since
            .is_none_or(|since| line_is_of_instance(timestamp, since))
    }
}

/// A sandbox agent token's log read (sandbox agent credential design §1, row
/// L1): the environment is required, and must be a sandbox the token created
/// of an app it is granted — where it is also the app's admin, as the gate
/// above asks of everyone else. Production (the absent parameter), staging
/// and another creator's sandbox are answered as an unknown app is.
async fn agent_environment(outcome: &AuthOutcome, raw: Option<&str>) -> Result<Admitted, Response> {
    use crate::server::api::custom_apps_agent::resolve_app_role_in;
    let not_found = || super::error_response(StatusCode::NOT_FOUND, "not permitted");
    let environment = requested_environment(raw).map_err(IntoResponse::into_response)?;
    if !matches!(environment, AppEnvironment::Dev { .. }) {
        return Err(not_found());
    }
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("db connect failed for the log environment check: {e}");
        super::error_response(StatusCode::INTERNAL_SERVER_ERROR, "retry shortly")
    })?;
    let (caller, app) = (&outcome.caller, &outcome.app);
    if !may_open_environment(&db, caller, app, &environment).await {
        return Err(not_found());
    }
    match resolve_app_role_in(&db, caller, app, &environment).await {
        Ok(Some(entity::app_members::ROLE_ADMIN)) => {}
        _ => return Err(not_found()),
    }
    // The start of the sandbox the token has now, read off the row it owns.
    let token = caller.sandbox_agent().map(|reach| reach.token_id);
    let since = match token {
        Some(token) => own_since(&db, app.id, &environment, token).await,
        None => Ok(None),
    };
    match since {
        Ok(Some(since)) => Ok(Admitted {
            environment: environment.name(),
            since: Some(since),
        }),
        _ => Err(not_found()),
    }
}

/// The environment a log read asks for, as the store takes it: `""` for
/// production (absent, blank or named), otherwise the name — for a caller who
/// may open that app's non-production environments. The app-admin gate has
/// already passed; this is the second one, and only a non-production read
/// pays for it.
async fn log_environment(
    outcome: &AuthOutcome,
    raw: Option<&str>,
) -> Result<String, EnvironmentRefusal> {
    let environment = requested_environment(raw)?;
    if environment == AppEnvironment::Production {
        return Ok(String::new());
    }
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("db connect failed for the log environment check: {e}");
        EnvironmentRefusal {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: "the request could not be completed; retry shortly".to_string(),
        }
    })?;
    if !may_open_non_production(&db, &outcome.caller, &outcome.app).await {
        return Err(EnvironmentRefusal {
            status: StatusCode::FORBIDDEN,
            code: "non_production_refused",
            message: format!(
                "the {environment} environment's logs are open to Oxy staff who may open this \
                 app's non-production environments"
            ),
        });
    }
    Ok(environment.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Absent, blank and `production` are production; any other exact name is
    /// that environment; anything else is refused rather than read as
    /// production.
    #[test]
    fn a_log_read_names_an_environment_exactly_or_is_refused() {
        for raw in [None, Some(""), Some(" "), Some("production")] {
            assert_eq!(
                requested_environment(raw).ok(),
                Some(AppEnvironment::Production),
                "{raw:?}"
            );
        }
        assert_eq!(
            requested_environment(Some("staging")).ok(),
            Some(AppEnvironment::Staging)
        );
        assert_eq!(
            requested_environment(Some("dev-a1")).ok().map(|e| e.name()),
            Some("dev-a1".to_string())
        );
        for raw in ["Staging", "prod", "dev-", "staging' OR '1'='1"] {
            let refused = requested_environment(Some(raw)).expect_err("not an environment");
            assert_eq!(
                (refused.status, refused.code),
                (StatusCode::BAD_REQUEST, "invalid_environment"),
                "{raw}"
            );
        }
    }
}
