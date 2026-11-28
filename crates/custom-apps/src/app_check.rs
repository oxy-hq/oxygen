//! The platform checker on a custom app: the promote checks' runner
//! (`oxy-api-custom-apps` `custom_apps_build_checks`, one `oxyc checks run …
//! --report` on a GitHub Actions runner), signed in as the `app_checker`
//! service. Two environments, one action each, and nothing else:
//!
//! | Environment | Action | Asked beside |
//! | --- | --- | --- |
//! | staging | `Action::AppCheck` | `Action::AppNonProduction` |
//! | production | `Action::AppCheckProduction` | what production already asks of a viewer |
//!
//! Asked only at the doors a build's checks need, so the checker reaches those
//! and nothing else:
//!
//! | Door | Where |
//! | --- | --- |
//! | the page of that environment, its assets and the runtime's ready beacon | `resolve::resolve_page_request` |
//! | the app's own data-plane READ calls, `/query` and `/semantic-query` (and, on staging, their pin) | `gate::check_custom_app_read_gates`, `staging_pin::resolve` |
//! | recording what the build's checks found: the runner routes of an attempt of that environment (`POST …/checks/claim`, `POST …/checks/functions/settle`, `GET …/checks/stylesheets`, `POST …/checks/report`, `PUT …/checks/files/{name}`) | `oxy-api-custom-apps` `custom_apps_build_checks::admit` |
//!
//! **Never `/fn`, never a write on the app, on either environment.** The serve
//! tree's fence (`oxy-api-custom-apps` `serve::checker_may`) refuses the
//! checker anything but a `GET`/`HEAD` and the beacon before anything runs, so
//! a page that calls a function as it mounts sees that call refused; the
//! build's own `"check": true` functions are run by the platform when the
//! runner claims a staging attempt, never by the checker. Its only writes are
//! the rows and files of the build's checks.
//!
//! **On production the page is the only shared door it passes.** Production's
//! other doors resolve through `resolve::resolve_app_request` (the function
//! route, logs, health, debug, availability, the events door), which never
//! asks this module: holding `AppCheckProduction` opens none of them.
//!
//! Every other door (an environment's secrets, tail, logs, schedules, held
//! writes, promote, a function call, an ask, an automation run, the console)
//! asks `AppNonProduction`, `AppAccess` or a lifecycle action alone, which the
//! checker does not hold.
//!
//! The decision is `existing_allow && allows(..)`: the shipped term is
//! `globals::platform_reaches(.., Cap::CheckApps, org)`, and the ring holds it
//! to the one environment its action names.

use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::DatabaseConnection;

/// The action and the facet a check of `environment` is decided with; `None`
/// for a sandbox, which the checker never opens.
fn asked_for(environment: &AppEnvironment) -> Option<(oxy_authz::Action, oxy_authz::EnvFacet)> {
    match environment {
        AppEnvironment::Staging => {
            Some((oxy_authz::Action::AppCheck, oxy_authz::EnvFacet::Staging))
        }
        AppEnvironment::Production => Some((
            oxy_authz::Action::AppCheckProduction,
            oxy_authz::EnvFacet::Production,
        )),
        AppEnvironment::Dev { .. } => None,
    }
}

/// May `caller` check `app` in `environment`? Staging and production only,
/// each by its own action, only staff holding `check_apps` over the app's
/// org (the checker, a Global Admin, or root), and never in a workspace whose
/// org has locked Oxy out. Uncached: the checker makes a
/// few dozen requests per build, and every other caller is decided by the one
/// memoised standing read below without loading facts.
pub async fn may_check(
    db: &DatabaseConnection,
    caller: &contracts_authz::Caller,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> bool {
    let Some((action, facet)) = asked_for(environment) else {
        return false;
    };
    let existing_allow = match caller
        .authz()
        .platform_reaches_checked(db, caller, oxy_authz::Cap::CheckApps, app.org_id)
        .await
    {
        Ok(reaches) => reaches,
        Err(e) => {
            tracing::warn!(app_id = %app.id, error = %e, "app check: the standing could not be read");
            false
        }
    };
    // Nearly every caller holds no `check_apps`: decided without loading facts.
    if !existing_allow {
        return false;
    }
    // An org that locked Oxy out of the workspace has locked out this way in
    // too, for the checker and for every staff member who holds `check_apps`
    // (a Global Admin): the same switch every other staff door reads
    // (`auth::is_oxy_locked_down`), unreadable counting as locked. Without it a
    // Global Admin the ordinary rule refused for the lockdown was let in here.
    let locked = crate::auth::is_oxy_locked_down(db, app.project_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(app_id = %app.id, error = %e,
                "app check: the workspace's lockdown could not be read; counted as locked");
            true
        });
    if locked {
        return false;
    }
    let Some(facts) = caller.authz().load_principal_facts(db, caller).await else {
        return false;
    };
    let resource = oxy_authz::Resource::app(app.id, app.org_id).in_environment(facet);
    oxy_authz::enforce(
        "custom_app_check",
        &facts,
        action,
        &resource,
        existing_allow,
    )
}

/// [`may_check`] on staging and nowhere else: `false` for production and a
/// sandbox, whoever asks. For a door only staging's check passes (the staging
/// pin).
pub async fn may_check_staging(
    db: &DatabaseConnection,
    caller: &contracts_authz::Caller,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> bool {
    *environment == AppEnvironment::Staging && may_check(db, caller, app, environment).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_environment_is_asked_by_its_own_action_and_a_sandbox_by_none() {
        assert_eq!(
            asked_for(&AppEnvironment::Staging),
            Some((oxy_authz::Action::AppCheck, oxy_authz::EnvFacet::Staging))
        );
        assert_eq!(
            asked_for(&AppEnvironment::Production),
            Some((
                oxy_authz::Action::AppCheckProduction,
                oxy_authz::EnvFacet::Production
            ))
        );
        let sandbox = AppEnvironment::parse("dev-a1").expect("a sandbox name");
        assert_eq!(asked_for(&sandbox), None);
    }
}
