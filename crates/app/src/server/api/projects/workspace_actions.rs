//! What a non-production environment's `workspace-actions` plane makes of an
//! agent ask or an automation run, read once per request from the
//! environment's row (`oxy_custom_apps::planes::environment_planes`).
//!
//! The decision follows the **plane**, never the kind of environment: staging
//! and a sandbox both hold by default, and either can be set to `refused`.
//! Four doors read it, and each must agree:
//!
//! - the environment guard (`custom_app_env_request`), through
//!   [`PlanesWorkspaceActions`], so a `refused` environment answers `403`
//!   before a handler runs;
//! - `oxy-api-custom-apps`' `agent_ask_staging::ask_scope`, which holds the
//!   ask or refuses it;
//! - its `automation_run::staging_hold`, which answers `409 held_in_staging`
//!   or refuses it (both read the plane through
//!   `oxy_custom_apps::workspace_actions`);
//! - the bundle's chat history (`api-custom-apps`' `custom_apps_threads`), which
//!   serves a sandbox's threads only while an ask there may write them.
//!
//! A row that cannot be read refuses the request (`409`,
//! `custom_app_env_request::planes_unreadable`); the defaults are never a fallback for a choice
//! nobody could read.

use async_trait::async_trait;
use axum::http::HeaderMap;
use oxy_app_core::custom_app_env_request::WorkspaceActions;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::planes::{WorkspaceActionsMode, environment_planes};
use oxy_custom_apps::staging_pin::request::{find_app, staging_app_ref};

/// The environment guard's lookup: whether the `workspace-actions` plane of
/// the app a request names is `held` (or `preview`) in its environment. Runs
/// before authentication, and decides nothing but the guard's `403`: a request that
/// names no app, or one that does not exist, is not held (the guard's old
/// answer), and every handler behind it still gates the caller.
pub struct PlanesWorkspaceActions;

#[async_trait]
impl WorkspaceActions for PlanesWorkspaceActions {
    async fn held(
        &self,
        headers: &HeaderMap,
        environment: &AppEnvironment,
    ) -> Result<bool, String> {
        // `preview` lets the write through too: the ask is held and the
        // automation run becomes a preview dry run, both behind the guard.
        Ok(matches!(
            guard_mode(headers, environment).await?,
            Some(WorkspaceActionsMode::Held | WorkspaceActionsMode::Preview)
        ))
    }

    async fn previews(
        &self,
        headers: &HeaderMap,
        environment: &AppEnvironment,
    ) -> Result<bool, String> {
        Ok(guard_mode(headers, environment).await? == Some(WorkspaceActionsMode::Preview))
    }
}

/// The `workspace-actions` mode of the app a request names, for the guard:
/// `None` when it names no app or one that does not exist.
async fn guard_mode(
    headers: &HeaderMap,
    environment: &AppEnvironment,
) -> Result<Option<WorkspaceActionsMode>, String> {
    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| format!("database: {e}"))?;
    let Some(app_ref) = staging_app_ref(headers) else {
        return Ok(None);
    };
    let Some(app) = find_app(&db, &app_ref).await else {
        return Ok(None);
    };
    let planes = environment_planes(&db, app.id, environment).await?;
    Ok(planes.map(|p| p.workspace_actions))
}
