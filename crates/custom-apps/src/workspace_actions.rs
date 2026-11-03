//! What a non-production environment's `workspace-actions` plane makes of an
//! agent ask or an automation run, and which app such a request names.
//!
//! The decision follows the **plane**, never the kind of environment: staging
//! and a sandbox both hold by default, and either can be set to `refused`.
//! The doors that read it (the agent ask, the automation run, and the
//! bundle's chat history, which must serve exactly the threads an ask may
//! write) share these two functions so they cannot disagree. They live here,
//! below `oxy-app` and the `api-*` surfaces, so every door can reach them.
//!
//! A web framework is not a dependency of this crate, so a plane that cannot
//! be read is a `String` here; each door answers it as `409` through
//! `oxy_app_core::custom_app_env_request::planes_unreadable`.

use entity::apps;
use http::HeaderMap;
use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::non_production::{may_open_environment, may_open_non_production};
use crate::planes::{WorkspaceActionsMode, environment_planes};
use crate::staging_pin::request::{find_app, staging_app_ref};

/// The mode of `environment`'s `workspace-actions` plane for app `app_id`.
/// `Err` carries why the plane cannot be read: the caller answers `409`, and
/// never falls back to a default for a choice nobody could read. Production
/// has no planes and is never asked; if it were, nothing would be held.
pub async fn workspace_actions_mode(
    db: &DatabaseConnection,
    app_id: Uuid,
    environment: &AppEnvironment,
) -> Result<WorkspaceActionsMode, String> {
    Ok(environment_planes(db, app_id, environment)
        .await?
        .map_or(WorkspaceActionsMode::Refused, |planes| {
            planes.workspace_actions
        }))
}

/// The app a non-production request names ([`staging_app_ref`]: the host
/// wins), when it was published from `project_id` and the caller may open its
/// `environment`. `None` on any miss, a lookup error included.
///
/// Staging is decided as it always was (`may_open_non_production`). A
/// sandbox is decided by `may_open_environment`, which differs only for a
/// sandbox agent token: it holds a sandbox it created itself, nothing else.
pub async fn non_production_app(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    user: &AuthenticatedUser,
    project_id: Uuid,
    environment: &AppEnvironment,
) -> Option<apps::Model> {
    let caller = oxy_server_authz::Caller::from_user(user);
    let app = find_app(db, &staging_app_ref(headers)?).await?;
    if app.project_id != project_id {
        return None;
    }
    let allowed = match environment.kind() {
        AppEnvironmentKind::Dev => may_open_environment(db, &caller, &app, environment).await,
        _ => may_open_non_production(db, &caller, &app).await,
    };
    allowed.then_some(app)
}
