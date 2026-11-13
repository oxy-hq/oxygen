//! Per-workspace TTL caches shared through `AppState`.
//!
//! `TtlCache<V>` is generic and depends on nothing app-specific, so it lives
//! here in `oxy-app-core` with `AppState`. The two semantic caches `AppState`
//! holds are no longer aliases of it: both moved to `oxy-airlayer-compat` so
//! they can share one `LayerSource`, and both are re-exported below under their
//! old names.
//!
//! The `OxyProjectContext`-typed `WorkspaceContextCache` stays in `oxy-app`
//! (`server/router/workspace_cache.rs`): it needs the pipeline adapter, which is
//! oxy-app-internal, and `AppState` does not carry it.
//!
//! Staleness: TTL bounds how long a cached entry can lag a workspace's on-disk
//! config change. Explicit invalidation on every semantic file write is the
//! primary freshness mechanism; the TTL is a safety net.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

pub struct TtlCache<V: Send + Sync + 'static> {
    ttl: Duration,
    inner: Mutex<HashMap<Uuid, Entry<V>>>,
}

/// The parsed-layer cache is **not** a `TtlCache` alias any more either, and for
/// the reason its sibling moved: it is keyed on `(workspace_id, layer source)`,
/// and `LayerSource` is defined next to the engine cache in
/// `oxy-airlayer-compat`. Keeping the layer key here would mean a second name
/// for that concept in a second crate.
///
/// Keyed on `workspace_id` alone — as it was — the two scan roots of one
/// workspace collided: on an `ide` or `all` node the world-model handlers scan
/// the working copy while `/semantic`, metric-tree and preagg scan the compile
/// boundary, and whichever ran first decided what the other saw for a TTL. That
/// also undercut the engine cache one level up, which keys on the source
/// honestly but can only be as right as the layer it is handed. See
/// `oxy_airlayer_compat::layer_cache`.
pub use oxy_airlayer_compat::SemanticLayerCache;

pub fn new_semantic_layer_cache() -> Arc<SemanticLayerCache> {
    // 60s TTL: cheap safety net; explicit invalidation on every semantic file
    // write keeps the cache fresh within the same editing session.
    SemanticLayerCache::new()
}

/// The compiled-engine cache is **not** a `TtlCache` alias any more: it lives in
/// `oxy-airlayer-compat` so `agentic-semantic` and `agentic-analytics` can reach
/// the same instance. Importing it from here would be a cargo cycle — this crate
/// already depends on `agentic-semantic`.
///
/// Re-exported under the old name so the eight `AppState` construction sites and
/// the `server::router::workspace_cache` shim are unchanged.
///
/// Two differences from the type this replaced, both deliberate:
///   * it is keyed on `(workspace_id, layer source, dialects)`, not
///     `workspace_id` alone — where the source says whether the layer came from
///     this node's working copy or from a materialised compiled revision. See
///     `oxy_airlayer_compat::engine_cache`;
///   * it holds a bare `Arc<SemanticEngine>`. The old alias wrapped the engine in
///     a `Mutex` on the stated grounds that it is "`Send` but not `Sync`". It is
///     both, so the lock only serialised compiles that could have run in
///     parallel; `preagg_rebuild` had already been sharing a bare
///     `Arc<SemanticEngine>` across spawned tasks, which is only possible for a
///     `Sync` type.
pub use oxy_airlayer_compat::SemanticEngineCache;

pub fn new_semantic_engine_cache() -> Arc<SemanticEngineCache> {
    // Explicit invalidation on every semantic file write is the primary freshness
    // mechanism (same as the layer cache). TTL is a safety net only, and it is
    // deliberately the layer cache's 60s rather than longer: an engine must not
    // outlive the layer it has to agree with. See `engine_cache`.
    SemanticEngineCache::new()
}

struct Entry<V> {
    built_at: Instant,
    /// What the value was built FROM, for [`TtlCache::get_or_build_tagged`].
    /// `None` for an entry written by the untagged API.
    tag: Option<Uuid>,
    value: Arc<V>,
}

impl<V: Send + Sync + 'static> TtlCache<V> {
    pub fn with_ttl(ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            ttl,
            inner: Mutex::new(HashMap::new()),
        })
    }

    pub fn lookup(&self, key: Uuid) -> Option<Arc<V>> {
        let mut guard = self.inner.lock().expect("ttl cache mutex poisoned");
        let Some(entry) = guard.get(&key) else {
            return None;
        };
        if entry.built_at.elapsed() >= self.ttl {
            guard.remove(&key);
            return None;
        }
        Some(entry.value.clone())
    }

    pub fn remove(&self, key: Uuid) {
        let mut guard = self.inner.lock().expect("ttl cache mutex poisoned");
        guard.remove(&key);
    }

    pub fn insert(&self, key: Uuid, value: Arc<V>) {
        self.insert_tagged(key, None, value);
    }

    /// [`Self::insert`], recording the `tag` the value was built from.
    pub fn insert_tagged(&self, key: Uuid, tag: Option<Uuid>, value: Arc<V>) {
        let mut guard = self.inner.lock().expect("ttl cache mutex poisoned");
        guard.insert(
            key,
            Entry {
                built_at: Instant::now(),
                tag,
                value,
            },
        );
    }

    /// [`Self::lookup`], where an entry built from a different `tag` is a miss
    /// (and is dropped, exactly like an expired one).
    pub fn lookup_tagged(&self, key: Uuid, tag: Option<Uuid>) -> Option<Arc<V>> {
        let mut guard = self.inner.lock().expect("ttl cache mutex poisoned");
        let entry = guard.get(&key)?;
        if entry.built_at.elapsed() >= self.ttl || entry.tag != tag {
            guard.remove(&key);
            return None;
        }
        Some(entry.value.clone())
    }

    /// Look up; on miss run `build` outside the lock and (if it succeeds)
    /// insert. Concurrent misses are tolerated — last writer wins, which
    /// is idempotent for the cached value.
    pub async fn get_or_build<F, Fut>(&self, key: Uuid, build: F) -> Option<Arc<V>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Option<Arc<V>>>,
    {
        if let Some(v) = self.lookup(key) {
            return Some(v);
        }
        let v = build().await?;
        self.insert(key, v.clone());
        Some(v)
    }

    /// [`Self::get_or_build`] for a value that is a function of some versioned
    /// input — the worker's workspace context is a function of the promoted
    /// revision. `tag` names the version the caller sees NOW; an entry built
    /// from another one counts as expired, so a change of input is picked up on
    /// the next lookup instead of after the TTL.
    ///
    /// The tag must be something the caller reads, not something derived from
    /// the cached value: a derived tag that can legitimately differ from the
    /// input (a last-known-good revision pinned in place of a broken promoted
    /// one) would never match and rebuild on every lookup.
    pub async fn get_or_build_tagged<F, Fut>(
        &self,
        key: Uuid,
        tag: Option<Uuid>,
        build: F,
    ) -> Option<Arc<V>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Option<Arc<V>>>,
    {
        if let Some(v) = self.lookup_tagged(key, tag) {
            return Some(v);
        }
        let v = build().await?;
        self.insert_tagged(key, tag, v.clone());
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn hit_within_ttl_avoids_rebuild() {
        let cache: Arc<TtlCache<u32>> = TtlCache::with_ttl(Duration::from_secs(60));
        let key = Uuid::new_v4();
        let calls = Arc::new(AtomicUsize::new(0));

        let v1 = {
            let c = calls.clone();
            cache
                .get_or_build(key, || async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Some(Arc::new(7u32))
                })
                .await
                .unwrap()
        };
        assert_eq!(*v1, 7);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Second call hits the cache; builder must not run.
        let v2 = cache
            .get_or_build(key, || async {
                panic!("builder must not be invoked on cache hit");
            })
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&v1, &v2), "same Arc returned");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// Long enough that the "still cached" lookup right after an insert cannot
    /// outlive it on a loaded runner: the expiry clock is `std::time::Instant`,
    /// so it cannot be paused. At 40 ms that lookup missed in CI.
    const SHORT_TTL: Duration = Duration::from_millis(500);
    const PAST_SHORT_TTL: Duration = Duration::from_millis(600);

    #[tokio::test]
    async fn ttl_expiration_forces_rebuild() {
        let cache: Arc<TtlCache<u32>> = TtlCache::with_ttl(SHORT_TTL);
        let key = Uuid::new_v4();
        cache.insert(key, Arc::new(1));
        assert!(cache.lookup(key).is_some());
        tokio::time::sleep(PAST_SHORT_TTL).await;
        assert!(cache.lookup(key).is_none(), "expired entry must be gone");
    }

    #[tokio::test]
    async fn distinct_keys_are_isolated() {
        let cache: Arc<TtlCache<u32>> = TtlCache::with_ttl(Duration::from_secs(60));
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        cache.insert(a, Arc::new(1));
        cache.insert(b, Arc::new(2));
        assert_eq!(*cache.lookup(a).unwrap(), 1);
        assert_eq!(*cache.lookup(b).unwrap(), 2);
    }

    #[tokio::test]
    async fn same_tag_hits_and_a_changed_tag_rebuilds() {
        let cache: Arc<TtlCache<u32>> = TtlCache::with_ttl(Duration::from_secs(60));
        let key = Uuid::new_v4();
        let r1 = Some(Uuid::new_v4());
        let r2 = Some(Uuid::new_v4());

        let v1 = cache
            .get_or_build_tagged(key, r1, || async { Some(Arc::new(1u32)) })
            .await
            .unwrap();
        let again = cache
            .get_or_build_tagged(key, r1, || async {
                panic!("same tag within the TTL must hit");
            })
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&v1, &again));

        // The input moved on well inside the TTL: the old entry is a miss.
        let v2 = cache
            .get_or_build_tagged(key, r2, || async { Some(Arc::new(2u32)) })
            .await
            .unwrap();
        assert_eq!(
            *v2, 2,
            "a changed tag must rebuild, not serve the old value"
        );
        assert_eq!(*cache.lookup_tagged(key, r2).unwrap(), 2);
        assert!(
            cache.lookup_tagged(key, r1).is_none(),
            "the old tag no longer matches"
        );
    }

    #[tokio::test]
    async fn tag_from_some_to_none_and_back_rebuilds() {
        let cache: Arc<TtlCache<u32>> = TtlCache::with_ttl(Duration::from_secs(60));
        let key = Uuid::new_v4();
        let r1 = Some(Uuid::new_v4());
        cache.insert_tagged(key, None, Arc::new(0));
        assert!(cache.lookup_tagged(key, r1).is_none(), "None != Some");
        cache.insert_tagged(key, r1, Arc::new(1));
        assert!(cache.lookup_tagged(key, None).is_none(), "Some != None");
    }

    #[tokio::test]
    async fn tagged_entry_still_expires_on_ttl() {
        let cache: Arc<TtlCache<u32>> = TtlCache::with_ttl(SHORT_TTL);
        let key = Uuid::new_v4();
        let r1 = Some(Uuid::new_v4());
        cache.insert_tagged(key, r1, Arc::new(1));
        assert!(cache.lookup_tagged(key, r1).is_some());
        tokio::time::sleep(PAST_SHORT_TTL).await;
        assert!(cache.lookup_tagged(key, r1).is_none());
    }

    #[tokio::test]
    async fn failed_build_is_not_cached() {
        let cache: Arc<TtlCache<u32>> = TtlCache::with_ttl(Duration::from_secs(60));
        let key = Uuid::new_v4();
        let calls = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let c = calls.clone();
            let r = cache
                .get_or_build(key, || async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    None
                })
                .await;
            assert!(r.is_none());
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "None results are not memoized, so each call re-runs the builder"
        );
    }
}
