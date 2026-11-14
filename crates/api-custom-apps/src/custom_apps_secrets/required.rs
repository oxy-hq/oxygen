//! Which `required` keys a build declares that production holds no value for
//! — a promote's blocker and a rollback's warning (environments design §5.3,
//! §5.5a rulings 8 and 9).
//!
//! The same declarations the secrets view reconciles (the build's `env` block
//! and its functions' `webhook.secretVar`s, [`declared::merge_declared`])
//! against the keys stored directly under `apps/<id>/` and the workspace
//! secrets shared with the app (`workspace_shared`). Production inherits from
//! no other environment, so a key is unset exactly when neither provides it.

use std::collections::{BTreeMap, BTreeSet};

use entity::{app_builds, app_functions, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use super::declared::{self, DeclaredKey};

/// The `required` keys `build` declares with no production value, sorted. An
/// `Err` says the stored secrets or the build's functions could not be read.
pub(crate) async fn unset_in_production(
    db: &DatabaseConnection,
    app: &apps::Model,
    build: &app_builds::Model,
) -> Result<Vec<String>, String> {
    let (env, _) = declared::declared_env(build.manifest_json.as_ref(), app.id);
    let functions = app_functions::Entity::find()
        .filter(app_functions::Column::BuildId.eq(build.id))
        .all(db)
        .await
        .map_err(|e| format!("the build's functions could not be read ({e})"))?;
    let manifests: Vec<serde_json::Value> = functions
        .into_iter()
        .filter_map(|f| f.manifest_json)
        .collect();
    let declared = declared::merge_declared(env, declared::webhook_secret_vars(manifests.iter()));
    let rows = super::workspace_rows(db, app.project_id)
        .await
        .map_err(|(_, message)| message)?;
    let stored = super::stored_secrets_in(db, app, &AppEnvironment::Production, &rows).await;
    let mut stored: BTreeSet<String> = stored.into_iter().map(|s| s.key).collect();
    // A workspace secret shared with the app is read in production as surely
    // as one of its own.
    stored.extend(super::workspace_shared::shared_keys(db, app, &rows).await);
    Ok(unset(&declared, &stored))
}

/// The pure half: declared `required`, and not stored.
fn unset(declared: &BTreeMap<String, DeclaredKey>, stored: &BTreeSet<String>) -> Vec<String> {
    declared
        .values()
        .filter(|key| key.required && !stored.contains(&key.key))
        .map(|key| key.key.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_apps_secrets::declared::{EnvSource, OxyAppEnvDecl};

    #[test]
    fn a_required_key_with_no_production_value_is_unset() {
        let env = BTreeMap::from([
            (
                "STRIPE_KEY".to_string(),
                OxyAppEnvDecl {
                    required: true,
                    ..Default::default()
                },
            ),
            (
                "OPTIONAL".to_string(),
                OxyAppEnvDecl {
                    required: false,
                    ..Default::default()
                },
            ),
            (
                "SET_ONE".to_string(),
                OxyAppEnvDecl {
                    required: true,
                    ..Default::default()
                },
            ),
        ]);
        // A webhook's signing key is required by construction.
        let declared = declared::merge_declared(env, BTreeSet::from(["WEBHOOK_SIG".to_string()]));
        assert_eq!(declared["WEBHOOK_SIG"].source, EnvSource::Webhook);
        let stored = BTreeSet::from(["SET_ONE".to_string(), "UNDECLARED".to_string()]);
        assert_eq!(unset(&declared, &stored), vec!["STRIPE_KEY", "WEBHOOK_SIG"]);
    }
}
