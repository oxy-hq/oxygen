//! Rate limit: 60/min per (user, app, environment, function) by default.
//!
//! Mirrors the (user, app) `RateBucket` in `custom_apps_activity.rs` (design
//! doc §11.6), extended with the function name and the app environment. The
//! environment is in the key so a non-production environment never spends the
//! same person's production budget (see [`super::call_scope`]).
//!
//! TODO(scaling): this table is per-process, so in the multi-instance worker
//! fleet (see oxy-scaling-design) the effective limit is `limit * N` instances,
//! not `limit`. Acceptable for the MVP; back this with a shared counter (e.g.
//! a Postgres-backed sliding window) before relying on it as a hard cap.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::call_scope::CallScope;

pub(super) const DEFAULT_RATE_PER_MIN: u64 = 60;
const RATE_BUCKET_TTL: Duration = Duration::from_secs(120);

/// `(user, app, environment, function)`.
type Key = (Uuid, Uuid, String, String);

#[derive(Debug)]
struct RateBucket {
    window_start: Instant,
    count: u64,
}

fn rate_table() -> &'static Mutex<HashMap<Key, RateBucket>> {
    static TABLE: std::sync::OnceLock<Mutex<HashMap<Key, RateBucket>>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key(scope: &CallScope<'_>) -> Key {
    (
        scope.user_id,
        scope.app_id,
        scope.environment_name(),
        scope.function_name.to_string(),
    )
}

/// Count this call against its bucket; `true` when it is over the limit.
pub(super) fn would_exceed_rate(scope: &CallScope<'_>, limit_per_min: u64) -> bool {
    let now = Instant::now();
    let mut table = rate_table().lock().unwrap();
    table.retain(|_, b| now.duration_since(b.window_start) < RATE_BUCKET_TTL);

    let bucket = table.entry(key(scope)).or_insert_with(|| RateBucket {
        window_start: now,
        count: 0,
    });
    if now.duration_since(bucket.window_start).as_secs() >= 60 {
        bucket.window_start = now;
        bucket.count = 0;
    }
    bucket.count += 1;
    bucket.count > limit_per_min
}

#[cfg(test)]
mod tests {
    use oxy_app_core::custom_app_environment::AppEnvironment;

    use super::*;

    fn scope<'a>(app: Uuid, user: Uuid, env: &'a AppEnvironment) -> CallScope<'a> {
        CallScope {
            app_id: app,
            environment: env,
            function_name: "refresh",
            user_id: user,
        }
    }

    /// The collision this key prevents: an engineer exercising a function
    /// outside production must not use up the calls the same person is allowed
    /// against production.
    #[test]
    fn a_non_production_environment_does_not_spend_the_production_budget() {
        let (app, user) = (Uuid::new_v4(), Uuid::new_v4());
        let staging = AppEnvironment::Staging;
        let production = AppEnvironment::Production;

        assert!(!would_exceed_rate(&scope(app, user, &staging), 2));
        assert!(!would_exceed_rate(&scope(app, user, &staging), 2));
        assert!(
            would_exceed_rate(&scope(app, user, &staging), 2),
            "the third staging call in the minute is over a limit of 2"
        );

        assert!(
            !would_exceed_rate(&scope(app, user, &production), 2),
            "production's bucket is its own: staging's calls must not count against it"
        );
    }

    #[test]
    fn the_same_scope_shares_one_bucket() {
        let (app, user) = (Uuid::new_v4(), Uuid::new_v4());
        let production = AppEnvironment::Production;
        assert!(!would_exceed_rate(&scope(app, user, &production), 1));
        assert!(would_exceed_rate(&scope(app, user, &production), 1));
    }
}
