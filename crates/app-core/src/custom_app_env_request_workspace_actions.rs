//! The environment guard's one question of an environment's planes: does its
//! `workspace-actions` plane hold the three surfaces of
//! [`is_held_workspace_action`] (start and cancel an agent ask, start an
//! automation run), or refuse them?
//!
//! The guard itself is a pure function of the request ([`guard_with_actions`]).
//! The middleware around it asks this lookup first, and only for those three
//! `POST`s outside production (and, for an automation run's cancel, whether
//! the plane is `preview`), so every other request pays nothing. The
//! lookup is a trait because the planes are read by `oxy-custom-apps`, which
//! sits above this crate; `oxy-app` supplies the real one.
//!
//! [`guard_with_actions`]: super::guard_with_actions

use async_trait::async_trait;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};

use super::{
    ActionsOpen, is_api_path, is_held_workspace_action, is_preview_run_cancel, request_environment,
};
use crate::custom_app_environment::AppEnvironment;

/// Whether the `workspace-actions` plane of an environment is `held`.
#[async_trait]
pub trait WorkspaceActions: Send + Sync {
    /// `Ok(true)`: the plane of `environment`, for the app the request names,
    /// is `held`. `Ok(false)`: it is `refused`, or the request names no app
    /// (the guard then answers as it always did, `403`). `Err(detail)`: the
    /// plane could not be read; the request is refused and the defaults are
    /// never a fallback for a choice nobody could read.
    async fn held(&self, headers: &HeaderMap, environment: &AppEnvironment)
    -> Result<bool, String>;

    /// `Ok(true)`: the plane is `preview`, so the bundle's automation runs are
    /// real and their cancel may pass ([`is_preview_run_cancel`]). The same
    /// `Err` as [`Self::held`]. A lookup that previews nothing keeps the
    /// default.
    async fn previews(
        &self,
        _headers: &HeaderMap,
        _environment: &AppEnvironment,
    ) -> Result<bool, String> {
        Ok(false)
    }
}

/// A lookup that holds nothing: every held surface answers `403` outside
/// production. For a stack with no app database behind it.
pub struct NoWorkspaceActions;

#[async_trait]
impl WorkspaceActions for NoWorkspaceActions {
    async fn held(&self, _: &HeaderMap, _: &AppEnvironment) -> Result<bool, String> {
        Ok(false)
    }
}

/// `409 EnvironmentRefused`: the caller may open the environment, but its
/// planes cannot be read (a database fault, or a `planes` value nobody can
/// parse), so nothing runs on a guess. The status and body are the function
/// gate's for the same fault.
pub fn planes_unreadable(environment: &AppEnvironment, detail: &str) -> Response {
    (
        StatusCode::CONFLICT,
        axum::Json(serde_json::json!({
            "error": "EnvironmentRefused",
            "environment": environment.name(),
            "message": format!(
                "the {environment} environment's planes cannot be read ({detail}); fix them \
                 with `oxyc env update <app> {environment} --plane <key>=<mode>`"
            ),
        })),
    )
        .into_response()
}

/// What the environment a request addresses opens to the guard, asked only
/// when the answer can matter: a held surface, or an automation run's cancel,
/// outside production. An
/// unreadable environment header is left to [`super::guard_with_actions`],
/// which answers it `400`. Takes the request's parts, not the request: a body
/// is not `Sync`, so a borrow of it cannot be held across the lookup.
pub(super) async fn actions_open(
    lookup: &dyn WorkspaceActions,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Result<ActionsOpen, Box<Response>> {
    let Ok(environment) = request_environment(headers) else {
        return Ok(ActionsOpen::Refused);
    };
    if environment == AppEnvironment::Production || !is_api_path(path) {
        return Ok(ActionsOpen::Refused);
    }
    let (answer, open) = if is_held_workspace_action(method, path) {
        (lookup.held(headers, &environment).await, ActionsOpen::Held)
    } else if is_preview_run_cancel(method, path) {
        (
            lookup.previews(headers, &environment).await,
            ActionsOpen::Preview,
        )
    } else {
        return Ok(ActionsOpen::Refused);
    };
    match answer {
        Ok(true) => Ok(open),
        Ok(false) => Ok(ActionsOpen::Refused),
        Err(detail) => Err(Box::new(planes_unreadable(&environment, &detail))),
    }
}

#[cfg(test)]
#[path = "custom_app_env_request_workspace_actions_tests.rs"]
mod tests;
