//! A check run a **sandbox agent token** queued, admitted again before it
//! starts (sandbox agent credential design §4).
//!
//! The request that queued the run was admitted when it arrived. The run
//! starts later, on a worker, and "the token stops at once" has to hold for
//! it too: the task carries the token's id, and [`recheck`] asks, with no
//! cache anywhere on the path, what a request presenting the token now would
//! be asked —
//!
//! - the token row: not revoked, not expired, still a sandbox agent token;
//! - its grants: the one for this app still live;
//! - the minter: still an active user;
//! - the minter's standing: still `develop_apps` over the app's org, read
//!   uncached through the `Caller` door;
//! - the sandbox: still one this token created.
//!
//! A run that fails any of them is cancelled before a line of the function
//! runs. A run already started is not interrupted: that is the design's
//! stated bound.

use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::user::UserService;
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use crate::server::api::custom_apps_env_resolve::may_open_environment;
use crate::server::authz::Caller;

/// The token a task says asked for it. A value that is not a uuid names no
/// token, so the run is refused rather than started unchecked.
pub(crate) fn token_of(task_field: Option<&str>) -> Result<Option<Uuid>, String> {
    match task_field {
        None => Ok(None),
        Some(raw) => raw
            .parse()
            .map(Some)
            .map_err(|_| "app_function task names a credential token that is not a uuid".into()),
    }
}

/// Whether token `token_id` may, now, run a function in `environment` of app
/// `app_id`. `Err` says why not, for the run's record.
pub(crate) async fn recheck(
    db: &DatabaseConnection,
    token_id: Uuid,
    app_id: Uuid,
    environment: &AppEnvironment,
) -> Result<(), String> {
    const ENDED: &str = "the sandbox agent token that queued this run is no longer valid";
    let resolved = oxy_auth::token::sandbox_recheck::readmit(db, token_id)
        .await
        .map_err(|e| {
            tracing::info!(%token_id, error = %e, "queued check run: token not admitted again");
            ENDED.to_string()
        })?;
    let minter = UserService::find_user_by_identity(&resolved.identity)
        .await
        .map_err(|e| format!("minter lookup failed: {e}"))?
        .ok_or_else(|| ENDED.to_string())?
        .with_credential(Some(resolved.credential));
    let app = apps::Entity::find_by_id(app_id)
        .one(db)
        .await
        .map_err(|e| format!("app lookup failed: {e}"))?
        .ok_or_else(|| format!("app {app_id} not found"))?;
    if may_open_environment(db, &Caller::from_user(&minter), &app, environment).await {
        return Ok(());
    }
    tracing::info!(%token_id, %app_id, %environment, "queued check run: token no longer reaches the sandbox");
    Err(format!(
        "the sandbox agent token that queued this run no longer reaches {environment}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A task with no token is every task but a token's check; a malformed id
    /// is refused, never read as "no token".
    #[test]
    fn a_malformed_token_id_refuses_the_run() {
        assert_eq!(token_of(None), Ok(None));
        let id = Uuid::from_u128(7);
        assert_eq!(token_of(Some(&id.to_string())), Ok(Some(id)));
        assert!(token_of(Some("")).is_err());
        assert!(token_of(Some("not-a-uuid")).is_err());
    }
}
