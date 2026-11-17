//! Revisions whose semantic files are byte-for-byte the same share one cached
//! layer and one cached engine.
//!
//! Every promote mints a new revision id, and the layer and engine caches key
//! on it, so a push that changed one automation used to cold-start the
//! semantic engine on every pod: re-read the model, re-validate it, rebuild the
//! join graph, all for the files the previous revision already had. Argo CD
//! meets the same cost the same way — a commit that does not touch an
//! application's path reuses that application's rendered manifests — and so do
//! dbt (unchanged files are not re-parsed) and Bazel (an action whose input
//! digest is known is not re-run).
//!
//! ## Why this cannot serve a stale model
//!
//! An alias is only ever recorded from a **digest of the exact text** the layer
//! is parsed from, in the order it is parsed in, by the reader that parses it
//! (`oxy::config::scan`). The layer is a pure function of that text
//! (`build_layer_from_yaml`), and an engine is a pure function of the layer and
//! the dialect map (`build_engine`), which stays in [`crate::EngineKey`]. So
//! two revisions mapped to one canonical revision were going to build identical
//! layers and identical engines; handing the second one the first one's is the
//! same answer, minus the rebuild. Nothing here is a compile-time claim about
//! what a revision contains: a digest the reader did not compute itself is never
//! trusted.
//!
//! Three rules keep that true:
//!
//! - **First record wins, forever.** Once `(workspace, revision)` maps to a
//!   canonical revision it is never remapped in this process, so a request can
//!   never see a layer from one mapping and an engine from another.
//! - **Only deterministic text is recorded.** The reader records a revision
//!   only when every body came from the source text the compile stored, which
//!   is immutable. A body re-serialised from a fallback after a failed S3 read
//!   is equivalent but not byte-identical, so such a scan is not recorded and
//!   keeps its own revision key, exactly as before.
//! - **Scoped to one workspace.** A digest maps to a revision of the same
//!   workspace only; two tenants with the same files never share an entry.
//!
//! Process-local, like the caches it serves, and bounded.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Mutex, OnceLock};

use lru::LruCache;
use uuid::Uuid;

use crate::engine_cache::LayerSource;

/// SHA-256 of a revision's semantic files as the reader parses them. Opaque
/// here: computed by the reader, compared by equality, never interpreted.
pub type ContentDigest = [u8; 32];

/// Revisions remembered at once, across all workspaces. A miss only costs a
/// rebuild, never a wrong answer, so this bounds memory and nothing else.
const ALIAS_CAPACITY: usize = 4096;

#[derive(Default)]
struct Registry {
    /// `(workspace, digest)` → the first revision seen with that content.
    canonical_by_digest: HashMap<(Uuid, ContentDigest), Uuid>,
    /// `(workspace, revision)` → its canonical revision. Never remapped.
    canonical_by_revision: Option<LruCache<(Uuid, Uuid), Uuid>>,
}

impl Registry {
    fn revisions(&mut self) -> &mut LruCache<(Uuid, Uuid), Uuid> {
        self.canonical_by_revision.get_or_insert_with(|| {
            LruCache::new(NonZeroUsize::new(ALIAS_CAPACITY).expect("ALIAS_CAPACITY is non-zero"))
        })
    }
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

/// Record that `revision_id` of `workspace_id` parses from content `digest`,
/// and return the revision its cache entries live under: the first revision of
/// this workspace recorded with the same digest, or `revision_id` itself.
///
/// Idempotent, and first-record-wins: a revision already recorded keeps the
/// canonical revision it was given, whatever digest a later call passes.
pub fn record_revision_content(
    workspace_id: Uuid,
    revision_id: Uuid,
    digest: ContentDigest,
) -> Uuid {
    let Ok(mut registry) = registry().lock() else {
        // A poisoned registry degrades to no aliasing, which is the old
        // behaviour: every revision under its own key.
        return revision_id;
    };
    if let Some(canonical) = registry.revisions().get(&(workspace_id, revision_id)) {
        return *canonical;
    }
    let canonical = *registry
        .canonical_by_digest
        .entry((workspace_id, digest))
        .or_insert(revision_id);
    // Bound the digest map with the revision map: drop the digest entries whose
    // canonical revision the LRU is about to forget.
    if let Some(((ws, evicted), _)) = registry
        .revisions()
        .push((workspace_id, revision_id), canonical)
        .filter(|(key, _)| *key != (workspace_id, revision_id))
    {
        registry
            .canonical_by_digest
            .retain(|(w, _), r| !(*w == ws && *r == evicted));
    }
    canonical
}

/// The source a cache entry for `source` lives under: a revision recorded with
/// the same content as an earlier one maps to that one. The working copy, and
/// a revision nothing recorded, map to themselves.
pub fn canonical_source(workspace_id: Uuid, source: LayerSource) -> LayerSource {
    let LayerSource::Revision(revision_id) = source else {
        return source;
    };
    let Ok(mut registry) = registry().lock() else {
        return source;
    };
    match registry.revisions().get(&(workspace_id, revision_id)) {
        Some(canonical) => LayerSource::Revision(*canonical),
        None => source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> ContentDigest {
        [byte; 32]
    }

    #[test]
    fn a_revision_with_the_same_content_maps_to_the_first() {
        let ws = Uuid::new_v4();
        let (r1, r2) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(record_revision_content(ws, r1, digest(1)), r1);
        assert_eq!(record_revision_content(ws, r2, digest(1)), r1);
        assert_eq!(
            canonical_source(ws, LayerSource::Revision(r2)),
            LayerSource::Revision(r1)
        );
        assert_eq!(
            canonical_source(ws, LayerSource::Revision(r1)),
            LayerSource::Revision(r1)
        );
    }

    #[test]
    fn different_content_keeps_its_own_key() {
        let ws = Uuid::new_v4();
        let (r1, r2) = (Uuid::new_v4(), Uuid::new_v4());
        record_revision_content(ws, r1, digest(1));
        assert_eq!(record_revision_content(ws, r2, digest(2)), r2);
        assert_eq!(
            canonical_source(ws, LayerSource::Revision(r2)),
            LayerSource::Revision(r2)
        );
    }

    #[test]
    fn the_same_content_in_another_workspace_is_never_shared() {
        let (ws_a, ws_b) = (Uuid::new_v4(), Uuid::new_v4());
        let (ra, rb) = (Uuid::new_v4(), Uuid::new_v4());
        record_revision_content(ws_a, ra, digest(7));
        assert_eq!(record_revision_content(ws_b, rb, digest(7)), rb);
        assert_eq!(
            canonical_source(ws_b, LayerSource::Revision(rb)),
            LayerSource::Revision(rb)
        );
    }

    #[test]
    fn a_recorded_revision_is_never_remapped() {
        let ws = Uuid::new_v4();
        let (r1, r2) = (Uuid::new_v4(), Uuid::new_v4());
        record_revision_content(ws, r1, digest(1));
        record_revision_content(ws, r2, digest(2));
        // r2 shows up again claiming r1's content: it keeps its first mapping.
        assert_eq!(record_revision_content(ws, r2, digest(1)), r2);
        assert_eq!(
            canonical_source(ws, LayerSource::Revision(r2)),
            LayerSource::Revision(r2)
        );
    }

    #[test]
    fn unrecorded_revisions_and_the_working_copy_map_to_themselves() {
        let ws = Uuid::new_v4();
        let r = Uuid::new_v4();
        assert_eq!(
            canonical_source(ws, LayerSource::Revision(r)),
            LayerSource::Revision(r)
        );
        assert_eq!(
            canonical_source(ws, LayerSource::WorkingCopy),
            LayerSource::WorkingCopy
        );
    }
}
