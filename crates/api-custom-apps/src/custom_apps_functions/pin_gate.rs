//! The semantic-pin half of the environment gate: which model a non-production
//! run reads ([`with_effective_pin`]), and the refusal when that model is not
//! ready ([`EnvironmentRefused::semantic_pin_not_ready`], answered as the same
//! `503 semantic_branch_not_ready` the data plane's gate sends).
//!
//! Split from `environment_gate` (which decides *whether* a run happens) the
//! way the pin reader is split from the rest of `staging_pin`: this is the one
//! part that reads the database for the pin, and the only part that renders a
//! `503`.

use axum::response::Response;
use oxy_app_core::custom_app_environment::AppEnvironment;

use super::environment_gate::{EnvironmentRefused, RefusedReason};
use crate::custom_apps_staging_pin::PinNotReady;

impl EnvironmentRefused {
    /// A run refused because `environment`'s semantic model is not ready.
    pub(crate) fn semantic_pin_not_ready(
        environment: AppEnvironment,
        pin_not_ready: PinNotReady,
    ) -> Self {
        Self {
            environment,
            reason: RefusedReason::SemanticPinNotReady,
            pin_not_ready: Some(pin_not_ready),
            detail: None,
        }
    }

    /// `503 semantic_branch_not_ready` with `Retry-After`: the branch may
    /// finish compiling, so the caller can retry (`409` when the planes do not
    /// parse). The body is the one `oxy_app_core::gate_response` renders for
    /// the data plane too.
    pub(super) fn not_ready_response(&self) -> Response {
        let environment = self.environment.name();
        let fix = self.message();
        oxy_app_core::gate_response::pin_refused(oxy_app_core::gate_response::NotReady {
            environment: &environment,
            git_sha: self.pin_not_ready.as_ref().map_or("", PinNotReady::git_sha),
            fix: &fix,
            planes_unreadable: self
                .pin_not_ready
                .as_ref()
                .is_some_and(PinNotReady::is_planes_unreadable),
        })
    }
}

/// `policy`, reading the semantic model this run's **effective pin** names
/// (#3370, `custom_apps_staging_pin`): the build's own pin, else the
/// environment's (`app_environments.semantic_revision_id`). Refused with
/// [`RefusedReason::SemanticPinNotReady`] when the build pins nothing and the
/// environment follows a branch with no ready compile. Production never looks:
/// a promoted build always reads `workspaces.current_revision_id`.
#[cfg(feature = "custom-app-functions")]
pub(crate) async fn with_effective_pin(
    db: &sea_orm::DatabaseConnection,
    policy: super::env_policy::EnvPolicy,
    app_id: uuid::Uuid,
    build_id: uuid::Uuid,
) -> Result<super::env_policy::EnvPolicy, EnvironmentRefused> {
    use crate::custom_apps_staging_pin::{
        EnvironmentPin, effective_pin, environment_pin, pinned_revision_for,
    };
    if policy.is_production() {
        return Ok(policy);
    }
    let build_pin = pinned_revision_for(db, build_id).await;
    // A build's own pin wins, so the environment's is not read for it.
    let env_pin = match build_pin {
        Some(_) => EnvironmentPin::Unset,
        None => environment_pin(db, app_id, &policy.environment().name()).await,
    };
    match effective_pin(build_pin, &env_pin) {
        Ok(pin) => Ok(policy.with_semantic_pin(pin)),
        Err(not_ready) => Err(EnvironmentRefused::semantic_pin_not_ready(
            policy.environment().clone(),
            not_ready,
        )),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{StatusCode, header::RETRY_AFTER};

    use super::*;

    fn refused(environment: &str, cause: PinNotReady) -> EnvironmentRefused {
        EnvironmentRefused::semantic_pin_not_ready(
            AppEnvironment::parse(environment).expect("an environment"),
            cause,
        )
    }

    async fn body_of(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json")
    }

    /// A function in a branch-mode environment whose branch has not compiled
    /// answers the data plane's refusal: `503`, `Retry-After: 5`, and the one
    /// body `{ error, environment, message, git_sha, fix }`.
    #[tokio::test]
    async fn a_not_ready_branch_is_the_data_planes_503() {
        let cause = PinNotReady::branch("staging", "feat-x".into(), Some("abc123".into()));
        let fix = cause.fix();
        let response = refused("staging", cause).into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(RETRY_AFTER).map(|v| v.as_bytes()),
            Some(&b"5"[..])
        );
        assert_eq!(
            body_of(response).await,
            serde_json::json!({
                "error": "semantic_branch_not_ready",
                "environment": "staging",
                "message": fix,
                "git_sha": "abc123",
                "fix": fix,
            })
        );
    }

    /// The same refusal for a sandbox, with no commit recorded: `git_sha` is
    /// `""` (the shape the body has always carried) and the environment is the
    /// sandbox's.
    #[tokio::test]
    async fn a_sandbox_with_no_pin_names_itself_and_no_commit() {
        let response = refused(
            "dev-ada",
            PinNotReady::branch("dev-ada", "feat-x".into(), None),
        )
        .into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response.headers().contains_key(RETRY_AFTER));
        let body = body_of(response).await;
        assert_eq!(body["environment"], "dev-ada");
        assert_eq!(body["git_sha"], "");
        assert_eq!(body["error"], "semantic_branch_not_ready");
    }
}
