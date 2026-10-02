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
use oxy::database::client::establish_connection;
use oxy_app_core::custom_app_environment::AppEnvironment;

use crate::server::api::custom_apps_auth::AuthOutcome;
use crate::server::api::custom_apps_env_resolve::may_open_non_production;

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

/// The environment a log read asks for, as the store takes it: `""` for
/// production (absent, blank or named), otherwise the name — for a caller who
/// may open that app's non-production environments. The app-admin gate has
/// already passed; this is the second one, and only a non-production read
/// pays for it.
pub(super) async fn log_environment(
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
    let email = outcome.user_email.as_deref().unwrap_or("");
    if !may_open_non_production(&db, outcome.user_id, email, &outcome.app).await {
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
