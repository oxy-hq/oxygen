//! The parsed [`airlayer::SemanticLayer`], cached by the source it was read
//! from.
//!
//! Sibling to [`crate::engine_cache`], and it exists for the same reason one
//! level down. The engine cache keys on [`LayerSource`] so that a working-copy
//! reader and a revision reader can never be handed each other's engine — but
//! an engine is built FROM a layer, and the layer cache underneath it was keyed
//! on `workspace_id` alone. A caller could therefore state
//! `LayerSource::WorkingCopy` honestly, be handed a *compiled* layer by the
//! cache below, and file the resulting engine under a source it was not built
//! from. Keying both caches the same way is what makes the engine cache's key
//! mean what it says.
//!
//! ## The collision this closes
//!
//! One workspace has TWO semantic scan roots, and on an `ide` or `all` node
//! both are read in the same process against one map:
//!
//! * the **working copy** — what is checked out, uncommitted edits included.
//!   The world-model handlers scan it directly
//!   (`config_manager.semantics_scan_path()`).
//! * the **compile boundary** — the promoted, immutable revision read
//!   out of Postgres, which `/semantic`, metric-tree and preagg reach through
//!   `semantic::resolve_query_scan_source`.
//!
//! They differ whenever the promoted revision isn't what's on disk: a feature
//! branch, uncommitted edits, a protected-`main` redirect. Keyed on
//! `workspace_id` alone, whichever family populated the entry first decided
//! what the other saw until the TTL lapsed — a compile-boundary reader
//! executing queries against a working copy, or an IDE user shown `main`
//! instead of the branch they are editing.
//!
//! ## Why the key cannot be the pinned revision
//!
//! Same trap as the engine cache: a node holding a working copy is
//! `Origin::Compiled` too, so `ConfigManager::revision_id()` answers `Some(R)`
//! for a handler reading `semantics_scan_path()` AND for one reading a
//! compiled scan of revision R. Callers state their source; they never
//! infer it. `ScanDir::is_compiled()` is what makes that answerable, and
//! `QueryScanSource::source_revision()` is where the query handlers get it.
//!
//! ## Freshness and size
//!
//! TTL is a safety net; [`SemanticLayerCache::invalidate_workspace`] on every
//! semantic-file write, branch switch and pull is the primary mechanism, and it
//! clears every key for the workspace because the writer knows the workspace
//! changed but not which of its sources that invalidates.
//!
//! Bounded for the reason naming the revision creates: every promote mints a
//! new `revision_id`, the explicit invalidations above are all IdeOnly
//! working-copy routes, and expiry is lazy — so a retired revision's entry is
//! never looked up again and an unbounded map would pin one parsed layer per
//! (workspace, revision) a long-lived `oxy serve` ever touched. An LRU bounds
//! it, exactly as in `engine_cache` and `oxy::config::scan`.
//!
//! Note there is deliberately no eager sweep of a workspace's superseded
//! revisions on insert: across a promote window requests pinned to the old and
//! the new revision are both in flight, so evicting one on the other's insert
//! ping-pongs expensive reloads. See `engine_cache::get_or_build`.

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lru::LruCache;
use uuid::Uuid;

use crate::engine_cache::LayerSource;

/// Identity of a parsed layer: the workspace, and which of its two roots the
/// bytes came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayerKey {
    pub workspace_id: Uuid,
    pub source: LayerSource,
}

impl LayerKey {
    /// A layer read from this node's working copy.
    pub fn working_copy(workspace_id: Uuid) -> Self {
        Self {
            workspace_id,
            source: LayerSource::WorkingCopy,
        }
    }

    /// A layer read from a compiled revision.
    pub fn revision(workspace_id: Uuid, revision_id: Uuid) -> Self {
        Self {
            workspace_id,
            source: LayerSource::Revision(revision_id),
        }
    }

    /// `Some(revision)` read that revision; `None` read the working copy.
    ///
    /// Mirrors [`crate::EngineKey::for_source`] so a caller feeds the same
    /// `Option<Uuid>` to both caches and cannot describe one differently to the
    /// other.
    pub fn for_source(workspace_id: Uuid, revision_id: Option<Uuid>) -> Self {
        match revision_id {
            Some(r) => Self::revision(workspace_id, r),
            None => Self::working_copy(workspace_id),
        }
    }
}

struct Entry {
    built_at: Instant,
    layer: Arc<airlayer::SemanticLayer>,
}

/// TTL + LRU cache of parsed semantic layers, keyed by [`LayerKey`].
pub struct SemanticLayerCache {
    ttl: Duration,
    inner: Mutex<LruCache<LayerKey, Entry>>,
}

/// A safety net behind explicit invalidation, not the freshness mechanism.
///
/// Kept equal to [`crate::engine_cache::DEFAULT_ENGINE_TTL`]: the engine cache
/// documents at length why an engine must not outlive its layer, and a layer
/// outliving its engine is the same hazard read from the other end.
pub const DEFAULT_LAYER_TTL: Duration = Duration::from_secs(60);

/// Max parsed layers resident at once. Each pins the views, topics and
/// dimensions of one workspace at one source; 128 matches the engine cache so
/// the two bound the same key space to the same depth.
pub const DEFAULT_LAYER_CAPACITY: usize = 128;

impl SemanticLayerCache {
    pub fn new() -> Arc<Self> {
        Self::with_ttl(DEFAULT_LAYER_TTL)
    }

    pub fn with_ttl(ttl: Duration) -> Arc<Self> {
        Self::with_ttl_and_capacity(ttl, DEFAULT_LAYER_CAPACITY)
    }

    pub fn with_ttl_and_capacity(ttl: Duration, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            ttl,
            inner: Mutex::new(LruCache::new(
                NonZeroUsize::new(capacity).expect("layer cache capacity is non-zero"),
            )),
        })
    }

    /// A live entry, or `None` on miss or expiry. Marks the entry
    /// most-recently used, so the LRU evicts by traffic rather than insertion
    /// order.
    pub fn lookup(&self, key: &LayerKey) -> Option<Arc<airlayer::SemanticLayer>> {
        let mut guard = self.inner.lock().expect("layer cache mutex poisoned");
        let entry = guard.get(key)?;
        if entry.built_at.elapsed() >= self.ttl {
            guard.pop(key);
            return None;
        }
        Some(entry.layer.clone())
    }

    pub fn insert(&self, key: LayerKey, layer: Arc<airlayer::SemanticLayer>) {
        let mut guard = self.inner.lock().expect("layer cache mutex poisoned");
        guard.put(
            key,
            Entry {
                built_at: Instant::now(),
                layer,
            },
        );
    }

    /// Drop EVERY entry for a workspace, across both sources and all revisions.
    ///
    /// The callers — a semantic file write, a branch switch, a pull — know the
    /// workspace changed underneath them but not which of its keys that
    /// invalidates. Dropping only the working-copy key would leave a stale
    /// revision entry for the next fleet read to serve.
    pub fn invalidate_workspace(&self, workspace_id: Uuid) {
        let mut guard = self.inner.lock().expect("layer cache mutex poisoned");
        let doomed: Vec<LayerKey> = guard
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| k.workspace_id == workspace_id)
            .collect();
        for k in doomed {
            guard.pop(&k);
        }
    }

    /// Entry count. Test/diagnostic use — an unbounded map is invisible from
    /// `lookup` alone, which is how one would come back.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("layer cache mutex poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_cache::DEFAULT_ENGINE_TTL;

    fn layer() -> Arc<airlayer::SemanticLayer> {
        Arc::new(airlayer::SemanticLayer::new(Vec::new(), None))
    }

    /// The whole point: one workspace, two sources, two entries.
    ///
    /// Keyed on `workspace_id` alone — as it was before this module — the
    /// second lookup returned the first source's layer, and the world-model
    /// handlers and the compile-boundary readers served each other's bytes for
    /// a full TTL.
    #[test]
    fn the_two_sources_of_one_workspace_do_not_collide() {
        let cache = SemanticLayerCache::new();
        let ws = Uuid::new_v4();
        let revision = Uuid::new_v4();

        let working_copy = LayerKey::working_copy(ws);
        let compiled = LayerKey::revision(ws, revision);

        cache.insert(working_copy, layer());
        assert!(
            cache.lookup(&compiled).is_none(),
            "a working-copy layer must never be served to a compile-boundary reader"
        );

        cache.insert(compiled, layer());
        assert!(cache.lookup(&working_copy).is_some());
        assert!(cache.lookup(&compiled).is_some());
        assert_eq!(cache.len(), 2);
    }

    /// `for_source` must agree with `EngineKey::for_source` on what `None`
    /// means, or a caller feeding the same `Option<Uuid>` to both caches would
    /// describe its layer one way and its engine another.
    #[test]
    fn for_source_maps_none_to_the_working_copy() {
        let ws = Uuid::new_v4();
        let revision = Uuid::new_v4();
        assert_eq!(LayerKey::for_source(ws, None), LayerKey::working_copy(ws));
        assert_eq!(
            LayerKey::for_source(ws, Some(revision)),
            LayerKey::revision(ws, revision)
        );
    }

    /// A promote mints a new revision, so it is a miss by construction — no
    /// invalidation hook is needed on the promote path.
    #[test]
    fn a_new_revision_is_a_miss() {
        let cache = SemanticLayerCache::new();
        let ws = Uuid::new_v4();
        cache.insert(LayerKey::revision(ws, Uuid::new_v4()), layer());
        assert!(
            cache
                .lookup(&LayerKey::revision(ws, Uuid::new_v4()))
                .is_none()
        );
    }

    #[test]
    fn invalidate_workspace_drops_every_source_and_spares_the_neighbour() {
        let cache = SemanticLayerCache::new();
        let mine = Uuid::new_v4();
        let other = Uuid::new_v4();
        let neighbour = LayerKey::working_copy(other);

        cache.insert(LayerKey::working_copy(mine), layer());
        cache.insert(LayerKey::revision(mine, Uuid::new_v4()), layer());
        cache.insert(neighbour, layer());

        cache.invalidate_workspace(mine);

        assert!(cache.lookup(&LayerKey::working_copy(mine)).is_none());
        assert_eq!(cache.len(), 1, "only the other workspace's entry survives");
        assert!(cache.lookup(&neighbour).is_some());
    }

    #[test]
    fn expired_entries_are_not_served() {
        let cache = SemanticLayerCache::with_ttl(Duration::from_millis(30));
        let key = LayerKey::working_copy(Uuid::new_v4());
        cache.insert(key, layer());
        assert!(cache.lookup(&key).is_some());
        std::thread::sleep(Duration::from_millis(60));
        assert!(cache.lookup(&key).is_none());
    }

    /// Naming the revision makes the key space grow without bound over a
    /// process's life; the LRU is what stops it. Expiry cannot, because a
    /// retired revision's key is never looked up again.
    #[test]
    fn the_lru_bounds_a_key_space_that_retires_keys() {
        let cache = SemanticLayerCache::with_ttl_and_capacity(DEFAULT_LAYER_TTL, 2);
        let ws = Uuid::new_v4();
        let first = LayerKey::revision(ws, Uuid::new_v4());
        for _ in 0..8 {
            cache.insert(LayerKey::revision(ws, Uuid::new_v4()), layer());
        }
        assert_eq!(cache.len(), 2, "capacity holds across eight promotes");
        assert!(cache.lookup(&first).is_none());
    }

    /// The two caches back one invariant — plan from the layer, compile against
    /// the engine — so a drift in either constant breaks it silently. They live
    /// in sibling modules and nothing else would notice.
    #[test]
    fn the_layer_ttl_matches_the_engine_ttl() {
        assert_eq!(
            DEFAULT_LAYER_TTL, DEFAULT_ENGINE_TTL,
            "an engine must not outlive its layer, nor a layer its engine"
        );
    }
}
