//! A directed event: a `ctx.apps.send` (`custom_apps_functions::host::apps_send`)
//! as the relay reads it. Its outbox name is
//! `call:<app>:<function>:<caller function>` ([`SendTarget`]); there is no
//! schema and no subscription, and the one listener is the named function in
//! the build the event's environment serves.
//!
//! **The row is not trusted.** Rows are app-written (each writer role may call
//! `oxy_events.publish`), so the relay enforces what the host enforces: the
//! publishing app's build in the row's environment must have the function the
//! name claims as the caller, and that build's `calls` must declare the target.
//! A row that does not is stored with a relay error and delivered to nobody.
//! TODO(app-events-hardening): `actor` and the caller function are still the
//! row's own claims until the outbox carries the host-signed provenance MAC;
//! until then a delivery presents no actor and always runs as the system.
//!
//! The target is resolved when the event is relayed, so the delivery goes to
//! the build that is live then; an app, function or build that is missing, or a
//! function not marked `"callable": true`, is stored as the event's relay error
//! and delivered to nobody (the events API shows it).

use entity::{app_builds, app_functions, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::calls::{DeclaredCalls, SendTarget, function_callable};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::subscriptions::Listener;
use crate::custom_apps_env_resolve::resolve_function_environment;

/// The listener `target` names in `org_id`'s `environment`, or why there is
/// none.
pub async fn resolve(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
    publisher: &apps::Model,
    target: &SendTarget,
) -> Result<Listener, String> {
    let lookup = |e: sea_orm::DbErr| format!("could not resolve the send's target: {e}");
    let env = AppEnvironment::parse(environment)
        .ok_or_else(|| format!("`{environment}` is not an environment"))?;
    check_publisher(db, publisher, &env, target).await?;
    let app = apps::Entity::find()
        .filter(apps::Column::OrgId.eq(org_id))
        .filter(apps::Column::Slug.eq(&target.app))
        .one(db)
        .await
        .map_err(lookup)?
        .ok_or_else(|| format!("this org has no app `{}`", target.app))?;
    let build_id = resolve_function_environment(db, &app, &env)
        .await
        .map_err(lookup)?
        .build_id
        .ok_or_else(|| format!("{} has no build in {environment}", target.app))?;
    let function = app_functions::Entity::find()
        .filter(app_functions::Column::BuildId.eq(build_id))
        .filter(app_functions::Column::Name.eq(&target.function))
        .one(db)
        .await
        .map_err(lookup)?
        .ok_or_else(|| {
            format!(
                "{}/{} is not in the live build",
                target.app, target.function
            )
        })?;
    let manifest = function.manifest_json.unwrap_or(serde_json::Value::Null);
    if !function_callable(Some(&manifest)) {
        return Err(format!(
            "{}/{} is not marked `\"callable\": true`",
            target.app, target.function
        ));
    }
    Ok(Listener {
        app_id: app.id,
        workspace_id: app.project_id,
        paused: false,
        app_slug: app.slug,
        function_name: target.function.clone(),
        environment: environment.to_string(),
        function_manifest: manifest,
    })
}

/// The publisher's side: its build serving `env` has the function the row names
/// as the caller, and that build's `calls` allows the target.
async fn check_publisher(
    db: &DatabaseConnection,
    publisher: &apps::Model,
    env: &AppEnvironment,
    target: &SendTarget,
) -> Result<(), String> {
    let lookup = |e: sea_orm::DbErr| format!("could not check the sender: {e}");
    let refused = || {
        format!(
            "{} does not declare a call to {}/{} from `{}` in {env}",
            publisher.slug, target.app, target.function, target.caller_function
        )
    };
    let build_id = resolve_function_environment(db, publisher, env)
        .await
        .map_err(lookup)?
        .build_id
        .ok_or_else(refused)?;
    let caller = app_functions::Entity::find()
        .filter(app_functions::Column::BuildId.eq(build_id))
        .filter(app_functions::Column::Name.eq(&target.caller_function))
        .one(db)
        .await
        .map_err(lookup)?;
    let build = app_builds::Entity::find_by_id(build_id)
        .one(db)
        .await
        .map_err(lookup)?;
    let declared = build
        .as_ref()
        .and_then(|b| DeclaredCalls::from_manifest(b.manifest_json.as_ref()).ok())
        .unwrap_or_default();
    if caller.is_some() && declared.allows(&target.app, &target.function) {
        Ok(())
    } else {
        Err(refused())
    }
}
