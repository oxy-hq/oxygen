//! Opt-in result cache for Oxy Functions (manifest `cache: { ttlSeconds }`).
//!
//! A function is arbitrary, frequently side-effectful server-side logic, so
//! results are NEVER cached by default — only functions that explicitly declare
//! a cache TTL land here. The key is:
//!
//!   (environment, build_id, function_name, user_id, hash(request_body))
//!
//! - `environment` — staging and production serve the **same** build, so a key
//!   without it would hand a result computed in staging to a production caller
//!   (see [`super::call_scope`]).
//! - `build_id` — a promote/rollback rotates the channel pointer to a new
//!   build, so the cache invalidates automatically on deploy (no eviction).
//! - `user_id` — USER-SCOPED: a function receives `ctx.user` and may return
//!   per-user data, so a shared cache would leak one user's result to another.
//!   Per-user keying still collapses the common case (a dashboard re-invoking
//!   on reload for the same logged-in user).
//! - `hash(body)` — the invocation input.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use lru::LruCache;
use parking_lot::Mutex;
use uuid::Uuid;

use super::call_scope::CallScope;

/// Entry cap across all functions/users. Count-bounded for simplicity.
const MAX_ENTRIES: usize = 4096;

/// `(environment, build_id, function, user_id, hash(body))`.
type Key = (String, Uuid, String, Uuid, u64);
/// value = (stored_at, ttl, body). Per-entry TTL so functions can differ.
type Cache = Mutex<LruCache<Key, (Instant, Duration, Arc<String>)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(LruCache::new(
            NonZeroUsize::new(MAX_ENTRIES).expect("MAX_ENTRIES > 0"),
        ))
    })
}

fn key(scope: &CallScope<'_>, build_id: Uuid, body: &[u8]) -> Key {
    let mut h = DefaultHasher::new();
    body.hash(&mut h);
    (
        scope.environment_name(),
        build_id,
        scope.function_name.to_string(),
        scope.user_id,
        h.finish(),
    )
}

/// The cached response body if present and within its per-entry TTL; else
/// `None` (and a stale entry is evicted).
pub(super) fn get(scope: &CallScope<'_>, build_id: Uuid, body: &[u8]) -> Option<Arc<String>> {
    let mut c = cache().lock();
    let k = key(scope, build_id, body);
    match c.get(&k) {
        Some((at, ttl, v)) if at.elapsed() < *ttl => Some(v.clone()),
        Some(_) => {
            c.pop(&k);
            None
        }
        None => None,
    }
}

/// Store a successful function result under its TTL.
pub(super) fn put(
    scope: &CallScope<'_>,
    build_id: Uuid,
    body: &[u8],
    value: String,
    ttl: Duration,
) {
    cache().lock().put(
        key(scope, build_id, body),
        (Instant::now(), ttl, Arc::new(value)),
    );
}

#[cfg(test)]
mod tests {
    use oxy_app_core::custom_app_environment::AppEnvironment;

    use super::*;

    fn scope<'a>(env: &'a AppEnvironment, function: &'a str, user: Uuid) -> CallScope<'a> {
        CallScope {
            app_id: Uuid::nil(),
            environment: env,
            function_name: function,
            user_id: user,
        }
    }

    #[test]
    fn put_then_get_hits_within_ttl() {
        let production = AppEnvironment::Production;
        let b = Uuid::new_v4();
        let u = Uuid::new_v4();
        let s = scope(&production, "f", u);
        put(&s, b, b"{}", "RESULT".to_string(), Duration::from_secs(60));
        assert_eq!(
            get(&s, b, b"{}").as_deref().map(String::as_str),
            Some("RESULT")
        );
    }

    #[test]
    fn misses_on_different_build_user_fn_or_body() {
        let production = AppEnvironment::Production;
        let b = Uuid::new_v4();
        let u = Uuid::new_v4();
        let s = scope(&production, "f", u);
        put(&s, b, b"{}", "R".to_string(), Duration::from_secs(60));
        assert!(get(&s, Uuid::new_v4(), b"{}").is_none(), "build isolation");
        assert!(
            get(&scope(&production, "f", Uuid::new_v4()), b, b"{}").is_none(),
            "user isolation"
        );
        assert!(
            get(&scope(&production, "g", u), b, b"{}").is_none(),
            "function isolation"
        );
        assert!(get(&s, b, b"{\"x\":1}").is_none(), "body isolation");
    }

    /// The collision this key prevents. Staging and production point at the
    /// same build after a promote, and the same person calls the same function
    /// with the same body in both — so everything else in the key matches.
    #[test]
    fn a_result_computed_in_staging_is_never_served_to_production() {
        let build = Uuid::new_v4();
        let user = Uuid::new_v4();
        let staging = AppEnvironment::Staging;
        let production = AppEnvironment::Production;
        put(
            &scope(&staging, "totals", user),
            build,
            b"{}",
            "STAGING RESULT".to_string(),
            Duration::from_secs(60),
        );

        assert!(
            get(&scope(&production, "totals", user), build, b"{}").is_none(),
            "a production call must miss a result staging cached"
        );
        assert_eq!(
            get(&scope(&staging, "totals", user), build, b"{}")
                .as_deref()
                .map(String::as_str),
            Some("STAGING RESULT"),
            "staging still reads its own entry"
        );
    }

    #[test]
    fn expires_after_ttl() {
        let production = AppEnvironment::Production;
        let b = Uuid::new_v4();
        let u = Uuid::new_v4();
        let s = scope(&production, "f", u);
        put(&s, b, b"{}", "R".to_string(), Duration::from_millis(0));
        std::thread::sleep(Duration::from_millis(5));
        assert!(get(&s, b, b"{}").is_none(), "expired entry must miss");
    }
}
