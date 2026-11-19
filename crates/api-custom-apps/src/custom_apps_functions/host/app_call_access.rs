//! The AppCall door: may a person open another app, as a run for them asks it.
//!
//! One decision, asked from two places: a `ctx.apps.call` at the target's door
//! ([`may_call_as`], via `apps_op::authorize_target`) and a send's delivery
//! before it runs as its sender ([`may_call_as_checked`],
//! `app_function_executor::sender`). Production goes through
//! `Action::AppCall` (the ring of `AppAccess`, with the shipped
//! `user_can_access_app` as the existing check); a non-production environment
//! through the route door's own `may_open_environment`. There is one copy of
//! the decision: [`may_call_as`] is [`may_call_as_checked`] with a failed read
//! taken for a refusal, so the two cannot diverge.

use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;

/// May `caller` open `app` in `environment`? `site` names the asker in the
/// decision's log. A read that failed is a refusal here, as for a call's
/// caller, who sees the refusal and tries again; see [`may_call_as_checked`]
/// for the form that tells the two apart.
pub(crate) async fn may_call_as(
    db: &sea_orm::DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
    environment: &AppEnvironment,
    site: &'static str,
) -> bool {
    may_call_as_checked(db, caller, app, environment, site)
        .await
        .unwrap_or(false)
}

/// [`may_call_as`], telling a refusal from a failed read: `Err` when the
/// caller's facts, their platform grant or the app's access rows could not be
/// read. The caller's facts are the ones `oxy_server_authz::enforce_for`
/// loads (`load_principal_facts`, workspace facts included). For a send's
/// delivery, which retries a blip and kills only a real refusal.
pub(crate) async fn may_call_as_checked(
    db: &sea_orm::DatabaseConnection,
    caller: &oxy_server_authz::Caller,
    app: &apps::Model,
    environment: &AppEnvironment,
    site: &'static str,
) -> Result<bool, sea_orm::DbErr> {
    // Outside production: the route door's own decision, through its 60 s
    // verdict cache, so a staging page whose function calls on loads no facts
    // on a hit.
    if !matches!(environment, AppEnvironment::Production) {
        return oxy_custom_apps::non_production::may_open_environment_checked(
            db,
            caller,
            app,
            environment,
        )
        .await;
    }
    let facts = oxy_server_authz::loader::load_principal_facts(db, caller)
        .await
        .ok_or_else(|| sea_orm::DbErr::Custom("the caller's facts could not be read".into()))?;
    // `user_can_access_app` reads the platform grant checked: a failed read is
    // its `Err`, never "not staff".
    let existing = oxy_custom_apps::auth::user_can_access_app(db, caller, app).await?;
    Ok(oxy_server_authz::enforce(
        site,
        &facts,
        oxy_authz::Action::AppCall,
        &crate::workspace_custom_apps::app_access_resource(app).published_from(app.project_id),
        existing,
    ))
}
