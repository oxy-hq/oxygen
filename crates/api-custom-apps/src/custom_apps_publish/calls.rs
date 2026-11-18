//! What a publish says about `calls` and `callable` (app-to-app calls).
//!
//! The manifest is held to its shape ([`check`]): a malformed `calls`, or a
//! `callable` that is not a boolean, refuses the publish. A declared target that
//! is missing, has no build in the environment published to, or is not marked
//! `"callable": true` only warns ([`warnings`]): apps publish independently, so
//! the callee may land after its caller. At runtime the same facts are a typed
//! refusal (`AppCallNotFound`, `AppCallNotDeployed`, `AppCallNotCallable`).

use entity::{app_functions, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::calls::{DeclaredCalls, function_callable};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::PublishError;
use crate::custom_apps_env_resolve::resolve_function_environment;

/// The manifest's `calls` block and every function's `callable`, checked.
pub(super) fn check(
    manifest: Option<&serde_json::Value>,
    functions: &[(String, serde_json::Value)],
) -> Result<(), PublishError> {
    let invalid = |message: String, fix: &str| {
        PublishError::Invalid(crate::custom_apps_validate::BundleValidation::new(
            "calls", message, fix,
        ))
    };
    DeclaredCalls::from_manifest(manifest).map_err(|m| {
        invalid(
            m,
            "declare \"calls\": { \"<app-slug>\": [\"<function>\"] } in oxy-app.json",
        )
    })?;
    for (name, entry) in functions {
        if entry.get("callable").is_some_and(|v| !v.is_boolean()) {
            return Err(invalid(
                format!("function `{name}`: `callable` must be true or false"),
                "write \"callable\": true to let other apps call the function",
            ));
        }
    }
    Ok(())
}

/// One warning for each declared pair that would be refused where this build
/// is published (`environment`).
pub(super) async fn warnings(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &AppEnvironment,
    manifest: Option<&serde_json::Value>,
) -> Vec<String> {
    let Ok(declared) = DeclaredCalls::from_manifest(manifest) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (app, function) in declared.pairs() {
        match problem(db, org_id, environment, app, function).await {
            Ok(None) => {}
            Ok(Some(why)) => out.push(format!(
                "calls {app}/{function}: {why}; the call is refused until that is fixed"
            )),
            Err(e) => out.push(format!(
                "calls {app}/{function}: could not be checked ({e}); publish again to retry"
            )),
        }
    }
    out
}

/// Why `app`/`function` would be refused in `environment`, if it would.
async fn problem(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &AppEnvironment,
    app: &str,
    function: &str,
) -> Result<Option<String>, sea_orm::DbErr> {
    let Some(target) = apps::Entity::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(apps::Column::Slug.eq(app))
        .one(db)
        .await?
    else {
        return Ok(Some(format!("this org has no app `{app}`")));
    };
    let Some(build_id) = resolve_function_environment(db, &target, environment)
        .await?
        .build_id
    else {
        return Ok(Some(format!("`{app}` has no build in {environment}")));
    };
    let row = app_functions::Entity::find()
        .filter(app_functions::Column::BuildId.eq(build_id))
        .filter(app_functions::Column::Name.eq(function))
        .one(db)
        .await?;
    Ok(match row {
        None => Some(format!(
            "`{app}` has no function `{function}` in {environment}"
        )),
        Some(f) if !function_callable(f.manifest_json.as_ref()) => {
            Some(format!("`{function}` is not marked `\"callable\": true`"))
        }
        Some(_) => None,
    })
}
