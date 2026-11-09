//! Rate limit: 60/min per (user, app, environment, function) by default.
//!
//! Mirrors the (user, app) limit on SDK events in `custom_apps_activity.rs`
//! (design doc §11.6), extended with the function name and the app
//! environment. The environment is in the key so a non-production environment
//! never spends the same person's production budget (see [`super::call_scope`]).
//!
//! Counted in Postgres (`oxy_app_core::rate_limit`), so the limit is the limit
//! whatever the replica count; until 2026-11 it was a per-process table and the
//! effective cap was `limit × N`. This is a hot path, so a per-process
//! [`Precheck`] refuses a flood before the database once this replica alone
//! has seen more than the limit — the database then sees at most one write per
//! admitted request, and none for a caller already over.

use std::time::Duration;

use oxy_app_core::rate_limit::{self, Precheck};
use sea_orm::DatabaseConnection;

use super::call_scope::CallScope;

pub(super) const DEFAULT_RATE_PER_MIN: u64 = 60;
const RATE_WINDOW: Duration = Duration::from_secs(60);

static PRECHECK: Precheck = Precheck::new();

/// `function:<user>:<app>:<environment>:<function>`.
fn key(scope: &CallScope<'_>) -> String {
    format!(
        "function:{}:{}:{}:{}",
        scope.user_id,
        scope.app_id,
        scope.environment_name(),
        scope.function_name
    )
}

/// Count this call against its window; `true` when it is over the limit.
pub(super) async fn would_exceed_rate(
    db: &DatabaseConnection,
    scope: &CallScope<'_>,
    limit_per_min: u64,
) -> bool {
    let key = key(scope);
    if PRECHECK.over(&key, limit_per_min, RATE_WINDOW) {
        return true;
    }
    rate_limit::take(db, &key, limit_per_min, RATE_WINDOW)
        .await
        .is_limited()
}

#[cfg(test)]
mod tests {
    use oxy_app_core::custom_app_environment::AppEnvironment;
    use uuid::Uuid;

    use super::*;

    fn scope<'a>(app: Uuid, user: Uuid, env: &'a AppEnvironment) -> CallScope<'a> {
        CallScope {
            app_id: app,
            environment: env,
            function_name: "refresh",
            user_id: user,
            credential_token_id: None,
        }
    }

    /// The collision the key prevents: an engineer exercising a function
    /// outside production must not use up the calls the same person is allowed
    /// against production.
    #[test]
    fn a_non_production_environment_has_its_own_key() {
        let (app, user) = (Uuid::new_v4(), Uuid::new_v4());
        let staging = key(&scope(app, user, &AppEnvironment::Staging));
        let production = key(&scope(app, user, &AppEnvironment::Production));
        assert_ne!(staging, production);
        assert!(staging.starts_with(&format!("function:{user}:{app}:")));
    }

    /// The pre-check is the in-process half: it refuses only once this replica
    /// alone is past the limit, and one scope's calls never count for another.
    #[test]
    fn the_precheck_refuses_a_flood_before_the_database() {
        let pre = Precheck::new();
        let (app, user) = (Uuid::new_v4(), Uuid::new_v4());
        let staging = key(&scope(app, user, &AppEnvironment::Staging));
        let production = key(&scope(app, user, &AppEnvironment::Production));
        assert!(!pre.over(&staging, 2, RATE_WINDOW));
        assert!(!pre.over(&staging, 2, RATE_WINDOW));
        assert!(
            pre.over(&staging, 2, RATE_WINDOW),
            "the third staging call in the minute is over a limit of 2"
        );
        assert!(
            !pre.over(&production, 2, RATE_WINDOW),
            "production's budget is its own: staging's calls must not count against it"
        );
    }
}
