//! The one door to a compiled [`airlayer::SemanticEngine`].
//!
//! Building an engine revalidates the whole semantic model and rebuilds the
//! join graph. That cost is worth paying once per workspace, not once per
//! request — but before this module every surface transcribed its own
//! `spawn_blocking` → build → compile → drop dance, and only one of them went
//! through a cache. This is that cache, in the one crate every semantic
//! consumer can already reach.
//!
//! ## Why it lives here
//!
//! `oxy-app-core` already depends on `agentic-semantic`, so an agentic crate
//! importing the cache from `oxy-app-core` is a cargo cycle, not merely a
//! layering violation (`internal-docs/backend-architecture.md`). The door has
//! to sit BELOW every consumer, and `oxy-airlayer-compat` — airlayer + serde +
//! thiserror + uuid — is the deepest crate `oxy-app`, `agentic-semantic`,
//! `agentic-analytics` and `oxy-app-core` all already share.
//!
//! ## What the key has to say
//!
//! [`EngineKey`] is a triple, and each part earned its place by being a way two
//! callers can want *different* engines for the same workspace:
//!
//! - `workspace_id` — the obvious one.
//! - `source` — [`LayerSource`], which names WHERE the layer was read from, not
//!   which revision the request is pinned to. The distinction is the whole
//!   point: a node holding a working copy is `Origin::Compiled` too, so
//!   `ConfigManager::revision_id()` returns `Some(R)` for a handler reading
//!   `semantics_scan_path()` (the working copy) AND for one reading a
//!   compiled scan of revision R. Keying on the pinned revision would
//!   collide them — an IDE world-model panel whose working copy is behind the
//!   promoted revision would serve its stale layer to the custom-app data
//!   plane, and vice versa. So callers state their source; they never infer it.
//! - `dialects` — a fingerprint, not the map. `resolve_and_compile` derives
//!   dialects from `config.yml` strings and analytics derives them from live
//!   connectors, so the same layer compiles to different SQL depending on who
//!   asked. An engine built under one map must never be served to the other.
//!
//! ## No lock
//!
//! `SemanticEngine` is `Send + Sync` (asserted below — the earlier cache
//! claimed otherwise and paid for a `Mutex` it did not need). So entries are a
//! bare `Arc<SemanticEngine>`: compiles run concurrently, and there is no guard
//! that a cancelled handler could drop mid-compile.
//!
//! ## Freshness
//!
//! TTL is a safety net. [`SemanticEngineCache::invalidate_workspace`] on every
//! semantic-file write, branch switch, and pull is the primary mechanism — and
//! it clears EVERY key for that workspace, because the writer knows the
//! workspace changed but not which revision/dialect entries that invalidates.
//!
//! ## Bounded
//!
//! Every promote mints a new `revision_id`, so the key space GROWS over a
//! process's life and the explicit invalidations above only ever fire on
//! working-copy routes (they are all IdeOnly). An unbounded map would therefore
//! pin one whole engine per (workspace, revision) a long-lived `oxy serve` or
//! `oxy worker` ever touched. It is an LRU for that reason — the same reason
//! and the same shape as `oxy::config::scan`'s materialised-context cache,
//! which bounds exactly this key space.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;

use uuid::Uuid;

use crate::SemanticError;

/// `SemanticEngine` is plain owned data — layer, evaluator, join graph, dialect
/// map, promotions — with no interior mutability. Sharing it by `Arc` with no
/// lock depends on that, so state it to the compiler rather than to a comment.
const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    fn probe() {
        assert_send_sync::<airlayer::SemanticEngine>();
    }
    let _ = probe;
};

/// Where a layer was read from.
///
/// Not "which revision is pinned" — see the module docs. A node can hold a
/// working copy AND be pinned to a compiled revision at the same time, and two
/// handlers on such a node genuinely read different bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayerSource {
    /// The workspace files on this node's disk (`semantics_scan_path()`).
    WorkingCopy,
    /// A compiled revision, read from the compile boundary.
    Revision(Uuid),
}

/// Identity of a compiled engine. See the module docs for why each field is
/// load-bearing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EngineKey {
    pub workspace_id: Uuid,
    pub source: LayerSource,
    /// Fingerprint of the dialect map — see [`dialect_fingerprint`].
    pub dialects: u64,
}

impl EngineKey {
    /// An engine built from this node's working copy.
    pub fn working_copy(workspace_id: Uuid, databases: &[DatabaseConfig]) -> Self {
        Self {
            workspace_id,
            source: LayerSource::WorkingCopy,
            dialects: dialect_fingerprint(databases),
        }
    }

    /// An engine built from a compiled revision.
    pub fn revision(workspace_id: Uuid, revision_id: Uuid, databases: &[DatabaseConfig]) -> Self {
        Self {
            workspace_id,
            source: LayerSource::Revision(revision_id),
            dialects: dialect_fingerprint(databases),
        }
    }

    /// `Some(revision)` reads that revision; `None` reads the working copy.
    ///
    /// For the callers that resolve their scan through `oxy::config::scan`,
    /// which answers exactly this question per request.
    pub fn for_source(
        workspace_id: Uuid,
        revision_id: Option<Uuid>,
        databases: &[DatabaseConfig],
    ) -> Self {
        match revision_id {
            Some(r) => Self::revision(workspace_id, r, databases),
            None => Self::working_copy(workspace_id, databases),
        }
    }
}

use airlayer::DatabaseConfig;

/// Fingerprint of the `(name, dialect)` pairs an engine was built with,
/// **in order**.
///
/// Order is load-bearing, which is not obvious: `DatasourceDialectMap::
/// from_config_databases` ends by taking `databases.first()` as the DEFAULT
/// dialect for every view that declares no `datasource:`. So reordering
/// `config.yml` changes the SQL those views compile to. An earlier version of
/// this function sorted the pairs "because listing order is not an engine
/// difference"; it is.
///
/// Process-local (`DefaultHasher` is not stable across builds), which is all a
/// process-local cache needs.
pub fn dialect_fingerprint(databases: &[DatabaseConfig]) -> u64 {
    let pairs: Vec<(&str, &str)> = databases
        .iter()
        .map(|d| (d.name.as_str(), d.db_type.as_str()))
        .collect();
    let mut hasher = DefaultHasher::new();
    pairs.hash(&mut hasher);
    hasher.finish()
}

struct Entry {
    built_at: Instant,
    engine: Arc<airlayer::SemanticEngine>,
}

/// TTL + LRU cache of compiled engines, keyed by [`EngineKey`].
pub struct SemanticEngineCache {
    ttl: Duration,
    inner: Mutex<LruCache<EngineKey, Entry>>,
}

/// A safety net behind explicit invalidation, not the freshness mechanism —
/// see the module docs.
///
/// Deliberately the same 60s as the semantic LAYER cache, and not longer. The
/// world-model handlers plan from the layer and compile against the engine, so
/// an engine outliving its layer compiles a plan naming members it never saw;
/// those handlers map a compile failure to `None`, so the panel renders empty
/// instead of erroring. A layer reload now drops the workspace's engines
/// (`SemanticLayerCacheCtx::get_or_load`), and this equal TTL is the backstop
/// for any path that reloads a layer without going through it.
pub const DEFAULT_ENGINE_TTL: Duration = Duration::from_secs(60);

/// Max engines resident at once. Each pins a parsed layer, evaluator, join
/// graph and promotions, so this is the memory ceiling; 128 covers a large hot
/// set of (workspace, revision, dialects) without letting a long-lived process
/// accumulate one per revision ever promoted.
pub const DEFAULT_ENGINE_CAPACITY: usize = 128;

impl SemanticEngineCache {
    pub fn new() -> Arc<Self> {
        Self::with_ttl(DEFAULT_ENGINE_TTL)
    }

    pub fn with_ttl(ttl: Duration) -> Arc<Self> {
        Self::with_ttl_and_capacity(ttl, DEFAULT_ENGINE_CAPACITY)
    }

    pub fn with_ttl_and_capacity(ttl: Duration, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            ttl,
            inner: Mutex::new(LruCache::new(
                NonZeroUsize::new(capacity).expect("engine cache capacity is non-zero"),
            )),
        })
    }

    /// A live entry, or `None` on miss or expiry. Marks the entry most-recently
    /// used, so the LRU evicts by actual traffic rather than insertion order.
    pub fn lookup(&self, key: &EngineKey) -> Option<Arc<airlayer::SemanticEngine>> {
        let mut guard = self.inner.lock().expect("engine cache mutex poisoned");
        let entry = guard.get(key)?;
        if entry.built_at.elapsed() >= self.ttl {
            guard.pop(key);
            return None;
        }
        Some(entry.engine.clone())
    }

    /// The door. Returns the cached engine, or runs `build` and caches it.
    ///
    /// Synchronous by design: every caller is already inside the
    /// `spawn_blocking` it needs for the compile that follows, and an async
    /// signature here would only tempt someone to hold the result across an
    /// await for no reason. A failed build is NOT cached — a workspace whose
    /// layer does not validate must re-report that on the next request, not
    /// serve a memoised failure until the TTL lapses.
    pub fn get_or_build<F>(
        &self,
        key: EngineKey,
        build: F,
    ) -> Result<Arc<airlayer::SemanticEngine>, SemanticError>
    where
        F: FnOnce() -> Result<airlayer::SemanticEngine, SemanticError>,
    {
        if let Some(engine) = self.lookup(&key) {
            return Ok(engine);
        }
        // Built outside the lock: a slow build must not block lookups for
        // other workspaces. Two concurrent misses both build and the last
        // writer wins, which is idempotent.
        let engine = Arc::new(build()?);
        let mut guard = self.inner.lock().expect("engine cache mutex poisoned");
        // No eager sweep of the workspace's other revisions. Across a promote
        // window, requests pinned to the old revision and the new one are both
        // in flight, so evicting one on the other's insert makes the two
        // ping-pong expensive rebuilds. The TTL retires a dead revision within
        // a minute and the LRU bounds the total either way, so keeping both is
        // cheaper than choosing between them.
        guard.put(
            key,
            Entry {
                built_at: Instant::now(),
                engine: engine.clone(),
            },
        );
        Ok(engine)
    }

    /// Drop EVERY entry for a workspace, across all revisions and dialect maps.
    ///
    /// The callers — a semantic file write, a branch switch, a pull — know the
    /// workspace changed underneath them but not which of its keys that
    /// invalidates. Removing only the working-copy key would leave a stale
    /// revision entry to be served by the next fleet read.
    pub fn invalidate_workspace(&self, workspace_id: Uuid) {
        let mut guard = self.inner.lock().expect("engine cache mutex poisoned");
        let doomed: Vec<EngineKey> = guard
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| k.workspace_id == workspace_id)
            .collect();
        for k in doomed {
            guard.pop(&k);
        }
    }

    /// Drop the entries for ONE source of a workspace, across every dialect map.
    ///
    /// The narrow counterpart to [`invalidate_workspace`](Self::invalidate_workspace),
    /// for the caller that DOES know which source it invalidated: a layer
    /// reload. Once the layer cache is keyed per source, reloading the working
    /// copy does not replace the layer a `Revision(R)` engine was built from,
    /// so dropping that engine too is eviction without a reason — and not free:
    ///
    /// * on an `ide`/`all` node the two handler families miss the layer cache
    ///   independently, so a workspace-wide flush from either one throws away
    ///   the other's engine every TTL cycle, and a build revalidates the whole
    ///   layer and rebuilds the join graph;
    /// * across a promote window a straggler revision-N reload and a fresh
    ///   revision-N+1 reload would flush each other — the ping-pong
    ///   [`get_or_build`](Self::get_or_build) declines to do on insert, moved
    ///   one edge over;
    /// * a layer evicted for LRU *capacity* rather than staleness would flush
    ///   engines too, which would make this cache's own capacity much less
    ///   effective than it looks.
    ///
    /// Dialects are deliberately not part of the filter: they describe how a
    /// layer compiles, not which layer it is, so every dialect map's engine for
    /// this source was built from the layer being replaced.
    pub fn invalidate_source(&self, workspace_id: Uuid, source: LayerSource) {
        let mut guard = self.inner.lock().expect("engine cache mutex poisoned");
        let doomed: Vec<EngineKey> = guard
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| k.workspace_id == workspace_id && k.source == source)
            .collect();
        for k in doomed {
            guard.pop(&k);
        }
    }

    /// Entry count. Test/diagnostic use.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("engine cache mutex poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn db(name: &str, dialect: &str) -> DatabaseConfig {
        DatabaseConfig {
            name: name.to_string(),
            db_type: dialect.to_string(),
        }
    }

    /// An engine over an empty layer — always valid, cheap, and enough to
    /// assert identity by `Arc::ptr_eq`.
    fn build_empty() -> Result<airlayer::SemanticEngine, SemanticError> {
        airlayer::SemanticEngine::from_semantic_layer(
            airlayer::SemanticLayer::new(vec![], None),
            airlayer::DatasourceDialectMap::new(),
        )
        .map_err(|e| SemanticError::Engine(e.to_string()))
    }

    #[test]
    fn hit_within_ttl_does_not_rebuild() {
        let cache = SemanticEngineCache::new();
        let key = EngineKey::working_copy(Uuid::new_v4(), &[]);
        let calls = AtomicUsize::new(0);

        let a = cache
            .get_or_build(key, || {
                calls.fetch_add(1, Ordering::SeqCst);
                build_empty()
            })
            .unwrap();
        let b = cache
            .get_or_build(key, || panic!("must not rebuild on a hit"))
            .unwrap();

        assert!(Arc::ptr_eq(&a, &b), "same engine returned");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn expired_entry_rebuilds() {
        let cache = SemanticEngineCache::with_ttl(Duration::from_millis(30));
        let key = EngineKey::working_copy(Uuid::new_v4(), &[]);
        let a = cache.get_or_build(key, build_empty).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        let b = cache.get_or_build(key, build_empty).unwrap();
        assert!(!Arc::ptr_eq(&a, &b), "expired entry must be rebuilt");
    }

    #[test]
    fn failed_build_is_not_cached() {
        let cache = SemanticEngineCache::new();
        let key = EngineKey::working_copy(Uuid::new_v4(), &[]);
        let calls = AtomicUsize::new(0);
        for _ in 0..3 {
            let r = cache.get_or_build(key, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(SemanticError::Engine("nope".into()))
            });
            assert!(r.is_err());
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "a layer that will not validate must re-report on every request"
        );
        assert!(cache.is_empty());
    }

    // ── The ways a workspace-only key was wrong ─────────────────────────────

    #[test]
    fn working_copy_and_revision_are_separate_entries() {
        // The IDE world-model reads the working copy; the fleet metric-tree
        // reads a materialised compiled revision. A node can be BOTH — it holds
        // a working copy and is pinned to a revision — so keying on the pinned
        // revision would let whichever ran first serve the other's layer.
        let cache = SemanticEngineCache::new();
        let ws = Uuid::new_v4();
        let rev = Uuid::new_v4();

        let a = cache
            .get_or_build(EngineKey::working_copy(ws, &[]), build_empty)
            .unwrap();
        let b = cache
            .get_or_build(EngineKey::revision(ws, rev, &[]), build_empty)
            .unwrap();

        assert!(
            !Arc::ptr_eq(&a, &b),
            "a revision read must not serve the working copy's engine"
        );
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn a_pinned_revision_does_not_key_a_working_copy_read() {
        // The regression directly: on a node pinned to revision R,
        // `ConfigManager::revision_id()` is `Some(R)` for BOTH a working-copy
        // reader and a revision reader. The key must come from the source, so
        // `for_source(None)` and `for_source(Some(R))` must not collide.
        let ws = Uuid::new_v4();
        let rev = Uuid::new_v4();
        assert_ne!(
            EngineKey::for_source(ws, None, &[]),
            EngineKey::for_source(ws, Some(rev), &[]),
        );
        assert_eq!(
            EngineKey::for_source(ws, None, &[]),
            EngineKey::working_copy(ws, &[]),
        );
    }

    #[test]
    fn different_revisions_are_separate_entries() {
        let cache = SemanticEngineCache::new();
        let ws = Uuid::new_v4();
        let a = cache
            .get_or_build(EngineKey::revision(ws, Uuid::new_v4(), &[]), build_empty)
            .unwrap();
        let b = cache
            .get_or_build(EngineKey::revision(ws, Uuid::new_v4(), &[]), build_empty)
            .unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn different_dialects_are_separate_entries() {
        // `resolve_and_compile` derives dialects from config.yml strings,
        // analytics from live connectors. The same layer compiles to different
        // SQL under each, so they cannot share an entry.
        let cache = SemanticEngineCache::new();
        let ws = Uuid::new_v4();
        let pg = EngineKey::working_copy(ws, &[db("warehouse", "postgres")]);
        let duck = EngineKey::working_copy(ws, &[db("warehouse", "duckdb")]);
        assert_ne!(pg.dialects, duck.dialects);

        let a = cache.get_or_build(pg, build_empty).unwrap();
        let b = cache.get_or_build(duck, build_empty).unwrap();
        assert!(!Arc::ptr_eq(&a, &b), "a dialect change must rebuild");
    }

    // ── Fingerprint ─────────────────────────────────────────────────────────

    #[test]
    fn fingerprint_is_order_sensitive() {
        // NOT a hash-quality nicety: `from_config_databases` takes
        // `databases.first()` as the default dialect for every view with no
        // `datasource:`, so reordering config.yml changes the SQL those views
        // compile to. Sorting here would serve the old dialect from cache.
        let a = dialect_fingerprint(&[db("ch", "clickhouse"), db("duck", "duckdb")]);
        let b = dialect_fingerprint(&[db("duck", "duckdb"), db("ch", "clickhouse")]);
        assert_ne!(
            a, b,
            "the first database sets the default dialect, so order is an engine difference"
        );
    }

    #[test]
    fn fingerprint_distinguishes_name_and_dialect() {
        assert_ne!(
            dialect_fingerprint(&[db("x", "postgres")]),
            dialect_fingerprint(&[db("x", "duckdb")]),
        );
        assert_ne!(
            dialect_fingerprint(&[db("x", "postgres")]),
            dialect_fingerprint(&[db("y", "postgres")]),
        );
        assert_ne!(
            dialect_fingerprint(&[db("x", "postgres")]),
            dialect_fingerprint(&[]),
        );
    }

    // ── Invalidation ────────────────────────────────────────────────────────

    #[test]
    fn invalidate_clears_every_key_for_the_workspace() {
        // A semantic write knows the workspace changed, not which of its
        // source/dialect entries that invalidates. All of them must go.
        let cache = SemanticEngineCache::new();
        let ws = Uuid::new_v4();
        let other = Uuid::new_v4();
        cache
            .get_or_build(EngineKey::working_copy(ws, &[]), build_empty)
            .unwrap();
        cache
            .get_or_build(EngineKey::revision(ws, Uuid::new_v4(), &[]), build_empty)
            .unwrap();
        cache
            .get_or_build(
                EngineKey::working_copy(ws, &[db("w", "duckdb")]),
                build_empty,
            )
            .unwrap();
        cache
            .get_or_build(EngineKey::working_copy(other, &[]), build_empty)
            .unwrap();
        assert_eq!(cache.len(), 4);

        cache.invalidate_workspace(ws);

        assert_eq!(cache.len(), 1, "only the other workspace survives");
        assert!(cache.lookup(&EngineKey::working_copy(other, &[])).is_some());
    }

    #[test]
    fn workspaces_are_isolated() {
        let cache = SemanticEngineCache::new();
        let a = cache
            .get_or_build(EngineKey::working_copy(Uuid::new_v4(), &[]), build_empty)
            .unwrap();
        let b = cache
            .get_or_build(EngineKey::working_copy(Uuid::new_v4(), &[]), build_empty)
            .unwrap();
        assert!(!Arc::ptr_eq(&a, &b));
    }

    // ── Bounded ─────────────────────────────────────────────────────────────

    #[test]
    fn the_engine_ttl_does_not_outlive_a_layer_generation() {
        // The engine cache and the semantic LAYER cache are not independent:
        // the world-model handlers plan from the layer and compile against the
        // engine, and map a compile failure to `None` — an empty panel, not an
        // error. An engine that outlives its layer therefore fails silently.
        //
        // `SemanticLayerCacheCtx::get_or_load` drops a workspace's engines when
        // it reloads, so this equal TTL is the backstop, not the mechanism. It
        // is asserted because the two constants live in different crates and
        // nothing else would notice them drifting apart.
        assert!(
            DEFAULT_ENGINE_TTL <= Duration::from_secs(60),
            "the engine may not be cached longer than the layer it must agree with",
        );
    }

    #[test]
    fn a_new_revision_keeps_the_workspaces_other_entries() {
        // Deliberately NOT an eager sweep. Across a promote window, requests
        // pinned to the old revision and the new one are both in flight;
        // evicting one on the other's insert makes them ping-pong expensive
        // rebuilds. The TTL retires the dead one within a minute.
        let cache = SemanticEngineCache::new();
        let ws = Uuid::new_v4();
        let working_copy = EngineKey::working_copy(ws, &[]);
        let old_revision = EngineKey::revision(ws, Uuid::new_v4(), &[]);
        cache.get_or_build(working_copy, build_empty).unwrap();
        cache.get_or_build(old_revision, build_empty).unwrap();
        cache
            .get_or_build(EngineKey::revision(ws, Uuid::new_v4(), &[]), build_empty)
            .unwrap();

        assert_eq!(cache.len(), 3);
        assert!(
            cache.lookup(&old_revision).is_some(),
            "a request still pinned to the previous revision must not have its \
             engine evicted by a newer one landing",
        );
        assert!(cache.lookup(&working_copy).is_some());
    }

    #[test]
    fn capacity_bounds_growth_across_workspaces() {
        // The per-workspace sweep above cannot help a process that serves many
        // workspaces; the LRU is what bounds that.
        let cache = SemanticEngineCache::with_ttl_and_capacity(DEFAULT_ENGINE_TTL, 4);
        for _ in 0..20 {
            cache
                .get_or_build(EngineKey::working_copy(Uuid::new_v4(), &[]), build_empty)
                .unwrap();
        }
        assert_eq!(cache.len(), 4, "resident engines are capped");
    }

    #[test]
    fn lookup_refreshes_recency() {
        let cache = SemanticEngineCache::with_ttl_and_capacity(DEFAULT_ENGINE_TTL, 2);
        let a = EngineKey::working_copy(Uuid::new_v4(), &[]);
        let b = EngineKey::working_copy(Uuid::new_v4(), &[]);
        cache.get_or_build(a, build_empty).unwrap();
        cache.get_or_build(b, build_empty).unwrap();
        // Touch `a` so `b` becomes the eviction candidate.
        assert!(cache.lookup(&a).is_some());
        cache
            .get_or_build(EngineKey::working_copy(Uuid::new_v4(), &[]), build_empty)
            .unwrap();
        assert!(cache.lookup(&a).is_some(), "recently used entry survives");
        assert!(
            cache.lookup(&b).is_none(),
            "least recently used was evicted"
        );
    }

    /// A layer reload retires the engines built from THAT source and no others.
    ///
    /// Once the layer cache is keyed per source, a working-copy reload does not
    /// replace the layer a `Revision(R)` engine was built from — so flushing the
    /// whole workspace would evict a live engine for no reason, and would put
    /// the promote-window ping-pong this module avoids on insert back across
    /// the layer/engine edge.
    #[test]
    fn invalidate_source_spares_the_other_source() {
        let cache = SemanticEngineCache::new();
        let ws = Uuid::new_v4();
        let revision = Uuid::new_v4();
        let other_ws = Uuid::new_v4();

        let working_copy = EngineKey::working_copy(ws, &[]);
        let compiled = EngineKey::revision(ws, revision, &[]);
        let neighbour = EngineKey::working_copy(other_ws, &[]);

        for key in [working_copy, compiled, neighbour] {
            cache.get_or_build(key, build_empty).expect("engine builds");
        }
        assert_eq!(cache.len(), 3);

        cache.invalidate_source(ws, LayerSource::WorkingCopy);

        assert!(
            cache.lookup(&working_copy).is_none(),
            "the reloaded source goes"
        );
        assert!(
            cache.lookup(&compiled).is_some(),
            "the other source's engine was built from a layer this reload did not replace"
        );
        assert!(
            cache.lookup(&neighbour).is_some(),
            "another workspace is untouched"
        );
    }

    /// The wide door still exists for the callers that genuinely cannot say
    /// which source they invalidated — a file write, a branch switch, a pull.
    #[test]
    fn invalidate_workspace_still_drops_every_source() {
        let cache = SemanticEngineCache::new();
        let ws = Uuid::new_v4();
        for key in [
            EngineKey::working_copy(ws, &[]),
            EngineKey::revision(ws, Uuid::new_v4(), &[]),
        ] {
            cache.get_or_build(key, build_empty).expect("engine builds");
        }
        cache.invalidate_workspace(ws);
        assert_eq!(cache.len(), 0);
    }
}
