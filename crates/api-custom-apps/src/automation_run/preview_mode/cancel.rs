//! Who may cancel an automation run from a staging or sandbox host.
//!
//! The environment guard lets the cancel through there only when the
//! environment's `workspace-actions` plane is `preview`
//! (`custom_app_env_request::is_preview_run_cancel`), because only then does
//! the bundle start real runs. The handler decides the rest here: a caller who
//! may open the environment, cancelling one of that environment's own preview
//! runs. Anything else is refused, so a staging page can never stop a
//! production run by its id.

use axum::http::HeaderMap;
use axum::response::Response;
use oxy_app_core::custom_app_env_request::request_environment;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{DatabaseConnection, EntityTrait};
use tracing::warn;
use uuid::Uuid;

use crate::agent_ask_staging::{non_production_app, refused};

/// `Ok(())` in production, where a cancel behaves as it always did. Outside
/// it, `Ok(())` only for a caller who may open the request's environment,
/// cancelling a preview run of that app and environment; else the `404
/// EnvironmentRefused` a refused write gets. A failed lookup refuses.
pub(in crate::automation_run) async fn may_cancel(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    user: &AuthenticatedUser,
    project_id: Uuid,
    run_id: Uuid,
) -> Result<(), Response> {
    let environment = request_environment(headers).map_err(|e| e.into_response())?;
    if environment == AppEnvironment::Production {
        return Ok(());
    }
    let refusal = || refused(&environment, "cancelling an automation run");
    let Some(app) = non_production_app(db, headers, user, project_id, &environment).await else {
        return Err(refusal());
    };
    let branch = super::environment_branch(app.id, &environment);
    match entity::workspace_preview_runs::Entity::find_by_id(run_id.to_string())
        .one(db)
        .await
    {
        Ok(Some(row)) if row.kind == "procedure" && row.branch == branch => Ok(()),
        Ok(_) => Err(refusal()),
        Err(e) => {
            warn!(%run_id, error = %e, "preview cancel: run lookup failed; refusing");
            Err(refusal())
        }
    }
}
