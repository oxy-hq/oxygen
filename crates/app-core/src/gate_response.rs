//! The `{ message, code? }` JSON envelope the custom-app data-plane gate chain
//! answers a refusal with.
//!
//! Kept in sync with `query.rs`'s `ApiErr` — both deliberately use the same
//! `{ message, code? }` shape so callers can render errors uniformly across
//! endpoints. Every 500 the chain raises off a `DbErr` carries a `code` from
//! `DbFailure`; see `oxy_custom_apps::gate::GateError` for why.
//!
//! Here, below both `oxy-app` (the data plane's handlers) and the
//! `oxy-api-custom-apps` surface, because both turn a refusal into a response.
//!
//! [`GatedCaller`], the gate's admitted caller as plain fields, is here too:
//! `oxy-custom-apps` builds it and `oxy-workspace-runtime` reads it, and this
//! is the crate both reach.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(serde::Serialize)]
struct GateErr {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
}

/// `status` with a `{ "message": … }` body.
pub fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (
        status,
        Json(GateErr {
            message: msg.into(),
            code: None,
        }),
    )
        .into_response()
}

/// `status` with a `{ "message": …, "code": … }` body.
pub fn err_with_code(status: StatusCode, msg: impl Into<String>, code: &'static str) -> Response {
    (
        status,
        Json(GateErr {
            message: msg.into(),
            code: Some(code),
        }),
    )
        .into_response()
}

/// The response for a refusal reported as plain values (a status, a message
/// and an optional code): what `oxy_custom_apps::gate::GateError` carries.
pub fn refusal(status: u16, message: &str, code: Option<&'static str>) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    match code {
        Some(code) => err_with_code(status, message, code),
        None => err(status, message),
    }
}

/// What a `503 semantic_branch_not_ready` says: the environment that is
/// refused, the commit it waits on (`""` when none was recorded) and the way
/// out.
#[derive(Debug, Clone, Copy)]
pub struct NotReady<'a> {
    pub environment: &'a str,
    pub git_sha: &'a str,
    pub fix: &'a str,
    /// The pin is refused because the environment's `planes` do not parse.
    /// That is not retryable, so [`pin_refused`] answers it the `409` every
    /// reader of an unreadable row answers, not this `503`.
    pub planes_unreadable: bool,
}

/// `503 { "error": "semantic_branch_not_ready", "environment", "message",
/// "git_sha", "fix" }` with a `Retry-After`: the environment's semantic model
/// cannot be served right now (its branch's head has no ready compile, or its
/// pin could not be read). Retryable — the compile may finish — so the header
/// is the one every retryable 503 here carries; `message` repeats `fix` so a
/// client that renders `{ message }` shows the way out.
///
/// **The one rendering** of this refusal: the data plane's gate, the
/// automation run's pin read and the Oxy Functions admission all call it, so
/// the same `error` code never has two shapes.
pub fn semantic_branch_not_ready(not_ready: NotReady<'_>) -> Response {
    let NotReady {
        environment,
        git_sha,
        fix,
        planes_unreadable: _,
    } = not_ready;
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "semantic_branch_not_ready",
            "environment": environment,
            "message": fix,
            "git_sha": git_sha,
            "fix": fix,
        })),
    )
        .into_response();
    response.headers_mut().insert(
        axum::http::header::RETRY_AFTER,
        axum::http::HeaderValue::from_static("5"),
    );
    response
}

/// What a refused environment pin answers: the `409` of
/// [`planes_unreadable`](crate::custom_app_env_request::planes_unreadable)
/// when the row's planes do not parse (a retry cannot fix that, and it is the
/// answer every other reader of such a row gives), else the retryable
/// [`semantic_branch_not_ready`].
pub fn pin_refused(not_ready: NotReady<'_>) -> Response {
    if not_ready.planes_unreadable
        && let Some(environment) =
            crate::custom_app_environment::AppEnvironment::parse(not_ready.environment)
    {
        return crate::custom_app_env_request::planes_unreadable(
            &environment,
            "a value does not parse",
        );
    }
    semantic_branch_not_ready(not_ready)
}

/// `refusal`, or [`pin_refused`] when the gate refused on a pin:
/// the single rendering of `oxy_custom_apps::gate::GateError`'s parts.
pub fn gate_refusal(
    status: u16,
    message: &str,
    code: Option<&'static str>,
    not_ready: Option<NotReady<'_>>,
) -> Response {
    match not_ready {
        Some(not_ready) => pin_refused(not_ready),
        None => refusal(status, message, code),
    }
}

/// What the custom-app gate admitted a request as: the workspace row it
/// resolved, the authenticated caller, the project and the staging pin (`None`
/// on every live request). The workspace runtime takes this instead of the
/// gate's `CustomAppContext`, which lives above it.
///
/// **Built in one place.** [`GatedCaller::from_gate`] has one production
/// caller, `oxy_custom_apps::gate::CustomAppContext::gated`, and every surface
/// that ran the gate asks the context for it. `staging_pin` picks the
/// result-cache partition (`staging_pin_scope::scope_for`), so a second
/// constructor that read the pin from somewhere else would serve a branch's
/// answer to live users; `oxy-guards`' `gated_caller_has_one_constructor` fails
/// the build on one. The fields are private for the same reason.
#[derive(Clone, Copy)]
pub struct GatedCaller<'a> {
    workspace: &'a entity::workspaces::Model,
    user_id: uuid::Uuid,
    project_id: uuid::Uuid,
    staging_pin: Option<uuid::Uuid>,
}

impl<'a> GatedCaller<'a> {
    /// The gate's answer as plain fields. Call it only from
    /// `CustomAppContext::gated` (see the type's doc).
    pub fn from_gate(
        workspace: &'a entity::workspaces::Model,
        user_id: uuid::Uuid,
        project_id: uuid::Uuid,
        staging_pin: Option<uuid::Uuid>,
    ) -> Self {
        Self {
            workspace,
            user_id,
            project_id,
            staging_pin,
        }
    }

    pub fn workspace(&self) -> &'a entity::workspaces::Model {
        self.workspace
    }

    pub fn user_id(&self) -> uuid::Uuid {
        self.user_id
    }

    pub fn project_id(&self) -> uuid::Uuid {
        self.project_id
    }

    pub fn staging_pin(&self) -> Option<uuid::Uuid> {
        self.staging_pin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_not_ready_branch_is_a_retryable_503() {
        let not_ready = NotReady {
            environment: "staging",
            git_sha: "abc123",
            fix: "wait or push a fix",
            planes_unreadable: false,
        };
        let response = gate_refusal(500, "ignored", None, Some(not_ready));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(axum::http::header::RETRY_AFTER),
            Some(&axum::http::HeaderValue::from_static("5"))
        );
    }

    /// The body is `{ error, environment, message, git_sha, fix }`, the same
    /// for every caller of the builder.
    #[tokio::test]
    async fn the_not_ready_body_carries_the_environment() {
        let response = semantic_branch_not_ready(NotReady {
            environment: "dev-ada",
            git_sha: "",
            fix: "retry shortly",
            planes_unreadable: false,
        });
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(
            body,
            serde_json::json!({
                "error": "semantic_branch_not_ready",
                "environment": "dev-ada",
                "message": "retry shortly",
                "git_sha": "",
                "fix": "retry shortly",
            })
        );
    }

    /// Planes that do not parse are the `409` an unreadable row always gets:
    /// a retry cannot fix them, so they never carry `Retry-After`.
    #[tokio::test]
    async fn unreadable_planes_are_the_409_not_a_retryable_503() {
        let response = gate_refusal(
            500,
            "ignored",
            None,
            Some(NotReady {
                environment: "dev-a1",
                git_sha: "",
                fix: "set them again",
                planes_unreadable: true,
            }),
        );
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .is_none()
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(body["error"], "EnvironmentRefused");
        assert_eq!(body["environment"], "dev-a1");
    }

    #[test]
    fn every_other_refusal_keeps_its_status() {
        let response = gate_refusal(403, "no", None, None);
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .is_none()
        );
    }
}
