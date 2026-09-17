//! Shared TTL caches for the customer-apps hot paths.
//!
//! Both the bundle-serve handler (`custom_apps_serve`) and the
//! debug snapshot handler (`custom_apps_debug`) run the same
//! per-request resolution chain — authenticate, look up user, check
//! org membership, resolve the app and its build — for every asset request and every
//! product fetch. A single Next.js page load is 30-100 asset requests +
//! N parallel product fetches; without caching, that's hundreds of DB
//! hits per click.
//!
//! These caches are small, in-process, TTL'd. Eviction is a sweep of
//! expired entries when a write would push past a soft cap — no LRU
//! machinery. The TTL is intentionally short (60s) so that membership
//! revocations propagate within a minute without requiring explicit
//! invalidation.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

use oxy_auth::types::AuthenticatedUser;
use uuid::Uuid;

/// Lifetime of cache entries before they need re-fetching. Short enough
/// that org membership revocations propagate quickly; long enough to
/// absorb the asset-storm a Next.js page load triggers.
pub(super) const CACHE_TTL: Duration = Duration::from_secs(60);

/// Soft cap on cache size. A sweep of expired entries runs whenever a
/// write would push the cache past this threshold; if the sweep doesn't
/// reclaim anything (everything still fresh), the new entry still goes
/// in. Bounds the memory cost without forcing an LRU implementation.
pub(super) const CACHE_MAX_ENTRIES: usize = 4_096;

// ── Generic TTL map helpers ─────────────────────────────────────────────────

pub(super) fn get_fresh<K: Eq + Hash, V: Clone>(
    cache: &RwLock<HashMap<K, (V, Instant)>>,
    key: &K,
) -> Option<V> {
    let guard = cache.read().ok()?;
    let (value, inserted_at) = guard.get(key)?;
    if inserted_at.elapsed() > CACHE_TTL {
        return None;
    }
    Some(value.clone())
}

pub(super) fn insert_with_sweep<K: Eq + Hash, V>(
    cache: &RwLock<HashMap<K, (V, Instant)>>,
    key: K,
    value: V,
) {
    if let Ok(mut guard) = cache.write() {
        if guard.len() >= CACHE_MAX_ENTRIES {
            guard.retain(|_, (_, inserted_at)| inserted_at.elapsed() <= CACHE_TTL);
        }
        guard.insert(key, (value, Instant::now()));
    }
}

// ── User cache ──────────────────────────────────────────────────────────────
//
// Keyed by `custom_apps_auth::user_cache_key`: the user id a session names,
// and the lowercased address only for a provider identity that names nobody
// yet. It was keyed by the email string, which is "" for every frontline
// worker — one slot for the whole crew. See that function for the story.

fn user_cache() -> &'static RwLock<HashMap<String, (AuthenticatedUser, Instant)>> {
    static CACHE: OnceLock<RwLock<HashMap<String, (AuthenticatedUser, Instant)>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

pub(super) fn cached_user(key: &str) -> Option<AuthenticatedUser> {
    get_fresh(user_cache(), &key.to_string())
}

pub(super) fn set_cached_user(key: String, user: AuthenticatedUser) {
    insert_with_sweep(user_cache(), key, user);
}

// Org-membership caching now lives in `custom_apps_auth` alongside the
// combined (member | grant | global admin) access check, keyed on
// (user_id, app_id) — see that module for the rationale.

// ── App resolution cache ((org_slug, app_slug) → org + app rows) ────────────
//
// The serve handler's remaining uncached cost. Auth, user, access and
// platform standing were all cached; the two lookups that turn a URL into
// rows were not, so a 100-asset page load ran 200 indexed queries to
// re-derive the same two rows 100 times.
//
// Cached together because they're resolved together and neither is useful
// without the other. Invalidated wholesale by
// `invalidate_app_resolution_cache` on any app-row mutation (publish,
// promote, visibility, delete) — the app row carries `published_at`,
// `draft_build_id`, `published_build_id` and `visibility`, all of which
// steer the serve decision, so a stale row must not outlive a publish.

/// An `(organizations, apps)` row pair resolved from a URL's slugs.
#[derive(Clone)]
pub(super) struct ResolvedApp {
    pub org: entity::organizations::Model,
    pub app: entity::apps::Model,
}

type AppResolutionKey = (String, String);

fn app_resolution_cache() -> &'static RwLock<HashMap<AppResolutionKey, (ResolvedApp, Instant)>> {
    static CACHE: OnceLock<RwLock<HashMap<AppResolutionKey, (ResolvedApp, Instant)>>> =
        OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

pub(super) fn cached_app_resolution(org_slug: &str, app_slug: &str) -> Option<ResolvedApp> {
    get_fresh(
        app_resolution_cache(),
        &(org_slug.to_string(), app_slug.to_string()),
    )
}

pub(super) fn set_cached_app_resolution(org_slug: &str, app_slug: &str, resolved: ResolvedApp) {
    insert_with_sweep(
        app_resolution_cache(),
        (org_slug.to_string(), app_slug.to_string()),
        resolved,
    );
}

/// Drop every cached slug→rows resolution.
///
/// Called from the same mutation sites as
/// `custom_apps_auth::invalidate_access_cache`. Like that one, we don't know
/// which slugs are affected (a rename changes the KEY, not just the value),
/// so we drop the whole map. Mutations are rare; reads are the hot path.
///
/// A *miss* is never cached, so a newly-created app is reachable
/// immediately without any invalidation.
pub fn invalidate_app_resolution_cache() {
    if let Ok(mut guard) = app_resolution_cache().write() {
        guard.clear();
    }
}

// ── Build-row cache (app_builds by primary key) ─────────────────────────────
//
// The third per-asset query. A build row is written once by the publish
// pipeline and the fields the serve path reads (`build_id`, `s3_prefix`)
// never change afterwards — a promote/rollback repoints `apps`, it does not
// rewrite `app_builds`. Keyed by PK, so a repointed channel looks up a
// different key and cannot serve a stale build.

fn build_cache() -> &'static RwLock<HashMap<Uuid, (entity::app_builds::Model, Instant)>> {
    static CACHE: OnceLock<RwLock<HashMap<Uuid, (entity::app_builds::Model, Instant)>>> =
        OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

pub(super) fn cached_build(build_pk: Uuid) -> Option<entity::app_builds::Model> {
    get_fresh(build_cache(), &build_pk)
}

pub(super) fn set_cached_build(build_pk: Uuid, build: entity::app_builds::Model) {
    insert_with_sweep(build_cache(), build_pk, build);
}
