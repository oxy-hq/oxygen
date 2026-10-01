//! Which environment's secrets a request to the secrets surface addresses.
//!
//! Production is the default and needs nothing beyond each mount's own guard.
//! **Staging is staff-only**: setting, reading or deleting
//! `apps/<id>/staging/<KEY>` is decided by oxy-authz
//! (`Action::AppNonProduction`, via `may_open_non_production` — the rule that
//! serves a staging host and admits a staging `/fn`), so a tenant admin on the
//! workspace mount, or an operator without `develop_apps`, is refused. Dev
//! slots hold no secrets: they do not run functions.

use axum::http::StatusCode;
use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;
use serde::Deserialize;

use super::Failure;

/// `?environment=staging` on the list, delete and reveal routes.
#[derive(Debug, Default, Deserialize)]
pub struct EnvironmentQuery {
    #[serde(default)]
    pub environment: Option<String>,
}

/// The environment `raw` names: absent or `production` is production,
/// `staging` is staging; anything else is a 400.
pub(super) fn parse(raw: Option<&str>) -> Result<AppEnvironment, Failure> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("production") => Ok(AppEnvironment::Production),
        Some("staging") => Ok(AppEnvironment::Staging),
        Some(other) => Err((
            StatusCode::BAD_REQUEST,
            format!(
                "environment {other:?} holds no secrets: use \"production\" (the default) or \
                 \"staging\""
            ),
        )),
    }
}

/// Refuse a non-production environment to anyone oxy-authz does not let open
/// one for this app. Production passes: the mount's own guard decided it.
pub(super) async fn authorize(
    db: &DatabaseConnection,
    app: &apps::Model,
    user: &AuthenticatedUser,
    environment: &AppEnvironment,
) -> Result<(), Failure> {
    if *environment == AppEnvironment::Production {
        return Ok(());
    }
    let email = user.email.as_deref().unwrap_or("");
    if crate::server::api::custom_apps_env_resolve::may_open_non_production(db, user.id, email, app)
        .await
    {
        return Ok(());
    }
    Err((
        StatusCode::FORBIDDEN,
        format!(
            "{environment} secrets are set by Oxy staff who may open this app's non-production \
             environments"
        ),
    ))
}

/// Outside production, which of `entries` a run reads from production: a key
/// is `shared` only when both the declaring build and the build production
/// serves mark it so (`shared_env::effective_shared_env`), and it is
/// inherited while this environment stores no value of its own.
pub(super) async fn mark_inherited(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    manifest: Option<&serde_json::Value>,
    entries: &mut [super::declared::AppSecretEntry],
) {
    if *environment == AppEnvironment::Production {
        return;
    }
    let shared = super::shared_env::effective_shared_env(db, app, environment, manifest).await;
    for entry in entries.iter_mut() {
        entry.shared = shared.contains(&entry.key);
        entry.inherits_production = entry.shared && !entry.is_set;
    }
}

/// [`parse`] then [`authorize`]: the environment a request may act on.
pub(super) async fn resolve(
    db: &DatabaseConnection,
    app: &apps::Model,
    user: &AuthenticatedUser,
    raw: Option<&str>,
) -> Result<AppEnvironment, Failure> {
    let environment = parse(raw)?;
    authorize(db, app, user, &environment).await?;
    Ok(environment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_is_the_default_and_staging_the_only_other() {
        assert_eq!(parse(None).unwrap(), AppEnvironment::Production);
        assert_eq!(parse(Some(" ")).unwrap(), AppEnvironment::Production);
        assert_eq!(
            parse(Some("production")).unwrap(),
            AppEnvironment::Production
        );
        assert_eq!(parse(Some("staging")).unwrap(), AppEnvironment::Staging);
        for refused in ["dev-luong", "Staging", "prod"] {
            let (code, _) = parse(Some(refused)).unwrap_err();
            assert_eq!(code, StatusCode::BAD_REQUEST, "{refused}");
        }
    }
}
