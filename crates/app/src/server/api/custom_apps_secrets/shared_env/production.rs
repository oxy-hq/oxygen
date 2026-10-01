//! The half of the `shared` rule that reads production: the build production
//! serves, for the runtime overlay ([`effective_shared_env`]) and the publish
//! gate ([`check_publish`]).
//!
//! "The build production serves" is `EnvironmentBuilds::resolve(Production)`,
//! never `resolve_for_functions`: the latter falls back to staging's build for
//! an app never promoted, which would let a staging build agree with itself.

use std::collections::BTreeSet;

use entity::{app_builds, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};
use serde_json::Value;

use super::{BuildFunctions, check_shared_env, effective_shared, manifest_functions};
use crate::server::api::custom_apps_build_store as store;
use crate::server::api::custom_apps_publish::PublishError;

/// The build production serves; `None` for an app never promoted.
async fn production_build(
    db: &DatabaseConnection,
    app: &apps::Model,
) -> Result<Option<app_builds::Model>, DbErr> {
    let builds =
        crate::server::api::custom_apps_env_resolve::load_environment_builds(db, app).await?;
    let Some(build_id) = builds.resolve(&AppEnvironment::Production).build_id else {
        return Ok(None);
    };
    app_builds::Entity::find_by_id(build_id).one(db).await
}

/// The keys a run of `app` in `environment` may read from production through
/// `ctx.env`'s shared fallback: [`effective_shared`] of the running build's
/// manifest and production's. Empty in production, and — failing closed — when
/// production's build cannot be read.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
pub(crate) async fn effective_shared_env(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    running_manifest: Option<&Value>,
) -> BTreeSet<String> {
    if *environment == AppEnvironment::Production
        || super::shared_env_keys(running_manifest).is_empty()
    {
        return BTreeSet::new();
    }
    match production_build(db, app).await {
        Ok(build) => effective_shared(
            running_manifest,
            build.as_ref().and_then(|b| b.manifest_json.as_ref()),
        ),
        Err(e) => {
            tracing::warn!(app_id = %app.id, "production build lookup failed; nothing is shared: {e}");
            BTreeSet::new()
        }
    }
}

/// The publish gate: the new manifest's `shared` keys against the bundle's
/// functions and those of the build production serves (`app` is `None` on a
/// first publish, which has none). Reads production's function artifacts only
/// when the manifest shares something.
pub(crate) async fn check_publish(
    db: &DatabaseConnection,
    app: Option<&apps::Model>,
    manifest_json: Option<&Value>,
    fn_specs: &[(String, Value)],
    files: &[(String, Vec<u8>)],
) -> Result<(), PublishError> {
    if super::shared_env_keys(manifest_json).is_empty() {
        return Ok(());
    }
    let production = match app {
        Some(app) => production_functions(db, app).await?,
        None => None,
    };
    check_shared_env(manifest_json, fn_specs, files, production.as_ref())
        .map_err(|conflicts| PublishError::SharedEnvConflict { conflicts })
}

/// Production's serving build as [`BuildFunctions`]: each declared function
/// with its bundled JS from the build store (empty when the artifact is gone).
async fn production_functions(
    db: &DatabaseConnection,
    app: &apps::Model,
) -> Result<Option<BuildFunctions>, PublishError> {
    let Some(build) = production_build(db, app)
        .await
        .map_err(|e| PublishError::Db(e.to_string()))?
    else {
        return Ok(None);
    };
    let mut functions = Vec::new();
    for (name, spec) in manifest_functions(build.manifest_json.as_ref()) {
        let artifact = format!("functions/{name}.js");
        let source = store::get_object(app.id, &build.build_id, &artifact)
            .await?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        functions.push((name, spec, source));
    }
    Ok(Some(BuildFunctions::production(&build.build_id, functions)))
}
