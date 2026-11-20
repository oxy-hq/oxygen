//! The builder-generation lever: invalidating rollups whose CONTENT a builder
//! change made wrong, and the bookkeeping that lets the invalidation prove it
//! finished.
//!
//! Separate from [`crate::cycle`] because it answers a different question.
//! The executor asks "is this rollup STALE?" — a freshness question, answered
//! by a refresh key. This asks "was this rollup built by a builder that stored
//! the wrong thing?" — which no refresh key can answer, because the stored
//! value changed while everything the key watches stayed put.

use std::sync::{Arc, RwLock};

use agentic_semantic::refresh_key_cache::RefreshKeyCache;

use crate::retract;
use crate::worker_config::PreaggWorkerConfig;
use oxy_compile::preagg_ledger;

/// Generation of the rollup *builder*. Bump this by hand, in the same commit,
/// whenever an airlayer bump (or an oxy-side change) alters WHAT A BUILT ROLLUP
/// CONTAINS — as opposed to how an already-built one is queried.
///
/// The lever exists because a change in what a build WRITES is invisible to
/// every other invalidation path: the refresh key describes the source data,
/// not the builder. airlayer #99 is the worked example: it folded
/// `Measure.filters` into the stored partials, where a filtered measure had
/// been storing the unfiltered total and serving it under the Pre-aggregated
/// badge. Without a lever like this one, deploying that fix repairs nothing
/// already cached — a rollup keyed `every: 24h` keeps serving the wrong number
/// for a day, and one keyed on a `sql:` probe whose value has not moved keeps
/// serving it indefinitely, reported Cached the whole time.
///
/// **Since airlayer #104 the hash is no longer the blind spot it was.** This
/// doc used to say `compute_rollup_hash` covers only member NAMES, time
/// dimension and granularity, "so the artifact keeps its name". airlayer
/// `0b4cf10` folded a `definition_fingerprint` into it — the view name and
/// `table:`/`sql:`, each dimension's `expr`, and per measure
/// `name:type:expr:filters` — so a change to what a member EXPANDS TO now
/// moves the hash, the artifact gets a new name, and the invalidation is
/// intrinsic. A #99-shaped change would today self-invalidate.
///
/// That narrows this constant's remit; it does not retire it. The fingerprint
/// covers the rollup's DEFINITION, so a builder change that emits different
/// SQL for an unchanged definition — a new `MeasureType` arm, a different
/// partial for the same `avg`, an oxy-side change to how a plan is executed —
/// still moves nothing the hash can see. That is what this is still for.
///
/// A cycle that finds `<cache_dir>/builder_generation` disagreeing with this
/// constant rebuilds every rollup the manifest says was BUILT — not every
/// declared one: invalidate what exists, don't build what nobody asked for —
/// and stamps the file only once it can show that every one of them was
/// actually replaced.
///
/// The stamp is per NODE, because a node that pulls a post-bump manifest from
/// S3 has not thereby rebuilt anything locally, and the local Parquet beside
/// that manifest may still be the old builder's. The manifest is a fleet-wide
/// artifact and cannot answer "did THIS node build this, with this builder?".
///
/// Which hashes a sweep covers is therefore not read off the manifest either:
/// `preagg_ledger` records the generation per committed hash on this node, and
/// a hash already at the current generation is not swept at all. That makes
/// the claim checkable rather than inferred — the sweep no longer force-rebuilds
/// entries another node already replaced correctly, and its failure-retraction
/// cannot delete one of them fleet-wide over a transient warehouse error.
pub const PREAGG_BUILDER_GENERATION: u32 = 1;

/// Stamp file inside the workspace's airlayer cache dir. Deliberately NOT part
/// of `manifest.json`: the manifest's shape is airlayer's, and a node that
/// pulls a newer manifest from S3 has not thereby rebuilt anything locally.
const BUILDER_GENERATION_FILE: &str = "builder_generation";

pub fn read_builder_generation(cache_dir: &std::path::Path) -> Option<u32> {
    std::fs::read_to_string(cache_dir.join(BUILDER_GENERATION_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Stamp the current generation. Best-effort: a failed write costs one
/// redundant rebuild next cycle, which is the right way round — the stamp must
/// never run ahead of the rebuilds it certifies.
pub fn write_builder_generation(cache_dir: &std::path::Path) {
    let write = std::fs::create_dir_all(cache_dir).and_then(|()| {
        std::fs::write(
            cache_dir.join(BUILDER_GENERATION_FILE),
            PREAGG_BUILDER_GENERATION.to_string(),
        )
    });
    if let Err(e) = write {
        tracing::warn!(
            error = %e,
            "preagg: could not stamp builder generation; the next cycle will rebuild again"
        );
    }
}

/// The rollup hashes this workspace's manifest says have actually been built.
/// Empty when nothing has — in which case a generation bump has nothing to
/// invalidate and the stamp can be written immediately.
pub fn built_rollup_hashes(cache_dir: &std::path::Path) -> std::collections::HashSet<String> {
    agentic_semantic::preagg::load_local_manifest(cache_dir)
        .map(|m| m.rollups.into_iter().map(|r| r.rollup_hash).collect())
        .unwrap_or_default()
}

/// What a cycle still owes before it may stamp the builder generation.
///
/// The stamp is a claim — "every artifact the previous builder left on this
/// node has been replaced" — and this is what lets a cycle prove it. `failed ==
/// 0` cannot: it means "nothing that ran, failed", which a targeted Rebuild of
/// a single row, a cancelled cycle, and a rollup that rebuilt to zero rows and
/// committed nothing all satisfy while leaving old artifacts in place. Stamping
/// on that spends the lever and leaves those rollups serving the previous
/// builder's numbers under the Pre-aggregated badge permanently.
///
/// The set starts as what the manifest says is built, intersected with what
/// the layer still declares, MINUS what `preagg_ledger` says this node already
/// published at this generation. That last subtraction is what keeps the claim
/// checkable rather than inferred: the manifest is fleet-wide and re-synced
/// from S3 on every status read, so without it a node could sweep — and, on a
/// failure, retract — an artifact another node had already rebuilt correctly.
///
/// A hash then leaves the set four ways: it was rebuilt AND committed; it was
/// retracted, so nothing of the previous builder's survives; no cycle can ever
/// rebuild it (a view whose datasource is not configured, or an entry the
/// layer no longer declares); or this node holds no Parquet for it at all, so
/// it is serving nothing for the hash and has nothing to prove. The last three
/// are why the sweep converges instead of re-running forever.
///
/// Discharging that second kind is not a claim that nothing reads it. The
/// rollup read path resolves no connector (`executing::execute_rollup` runs
/// before `resolve_solution_connector`, which only the fallback arm reaches),
/// so a rollup whose datasource was removed keeps answering from whatever
/// Parquet is on disk. It is discharged because no cycle can REPLACE it and
/// blocking the whole workspace's sweep on an artifact this builder can never
/// reach would be worse — not because the artifact is unreachable to a reader.
///
/// Nor because something else will clean it up. Retraction is the only path in
/// the tree that removes a manifest entry, and it is reached by hash from a
/// rebuild, so an orphaned entry and its Parquet stay on disk indefinitely.
/// That is a known gap, not a mechanism to lean on.
///
/// It has to terminate, because while it is open `generation_forces` overrides
/// every freshness gate — an undischargeable hash would mean the whole
/// workspace force-rebuilding on every tick forever, refresh keys ignored, with
/// one unbuildable rollup as the only visible symptom. So the two outcomes that
/// replace nothing RETRACT instead: a rollup that rebuilds to zero rows drops
/// its entry and Parquet in `rebuild_rollup`, and one whose rebuild fails while
/// the sweep is forcing it is resolved by [`resolve_failed_rollup`], which
/// retracts only when this node actually holds the file. Retraction is sound
/// there precisely because the sweep is running AND the ledger has no record
/// of this node publishing the hash — the artifact is the previous builder's,
/// known wrong — and the query falls back to the warehouse and answers
/// correctly without the badge.
#[derive(Debug, Default)]
pub struct GenerationSweep {
    pending: std::collections::HashSet<String>,
}

impl GenerationSweep {
    /// `built` is what the manifest says exists; `declared` is what the layer
    /// this cycle loaded still declares. The intersection is what a rebuild
    /// could actually reach — an entry outside it is an orphan no cycle can
    /// rebuild, and it cannot serve a query either.
    ///
    /// `ledger` then subtracts what this node has already published at
    /// `generation`. Without it the sweep is populated purely from a manifest
    /// the whole fleet writes and `sync_manifest_from_s3` pulls down on every
    /// status read, so opening the Pre-aggregation tab could hand a node 30
    /// entries it never built and no way to tell them from the previous
    /// builder's.
    pub fn new(
        built: std::collections::HashSet<String>,
        declared: &std::collections::HashSet<String>,
        ledger: &preagg_ledger::RollupLedger,
        generation: u32,
    ) -> Self {
        Self {
            pending: built
                .into_iter()
                .filter(|h| declared.contains(h))
                .filter(|h| !ledger.is_at_generation(h, generation))
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn contains(&self, hash: &str) -> bool {
        self.pending.contains(hash)
    }

    /// The rollup was rebuilt and its manifest entry committed.
    pub fn rebuilt(&mut self, hash: &str) {
        self.pending.remove(hash);
    }

    /// The artifact was REMOVED rather than replaced — nothing of the previous
    /// builder's is being served for this hash, which is what the stamp claims.
    pub fn retracted(&mut self, hash: &str) {
        self.pending.remove(hash);
    }

    /// How a rebuild's outcome maps onto the sweep, in ONE place so the mapping
    /// itself is what a test exercises. Both `Ok`s discharge — `Ok(true)`
    /// replaced the artifact, `Ok(false)` retracted it as empty — while an
    /// `Err` leaves the hash pending until the cycle retracts it, so a
    /// retraction that itself fails is retried next cycle rather than papered
    /// over.
    pub fn record<E>(&mut self, hash: &str, outcome: &Result<bool, E>) {
        if outcome.is_ok() {
            self.rebuilt(hash);
        }
    }

    /// No cycle can rebuild this one until something outside the cycle changes.
    pub fn unrebuildable(&mut self, hash: &str) {
        self.pending.remove(hash);
    }

    /// This node holds no Parquet for the hash, so it serves nothing for it —
    /// which is exactly what a PER-NODE stamp claims. The manifest entry is
    /// another node's build, and destroying it from here would mirror the
    /// deletion fleet-wide to fix a problem this node does not have.
    pub fn absent(&mut self, hash: &str) {
        self.pending.remove(hash);
    }

    pub fn is_complete(&self) -> bool {
        self.pending.is_empty()
    }
}

/// What a FAILED rebuild does to the previous builder's artifact, in one place.
///
/// Both places a rollup can fail reach this: the pre-loop that resolves each
/// database's connector, and the join of the spawned rebuilds. They see the
/// same root causes — `get_connector` is resolved in both — and treating one
/// of them two ways depending on which loop noticed it left the pre-loop's
/// hashes pending, so a single unreachable warehouse held the whole
/// workspace's sweep open and force-rebuilt every built rollup on every tick.
///
/// Three outcomes, and only the last one destroys anything:
///
/// * Not under a sweep → nothing. The artifact is this builder's own, merely
///   stale, and a failed refresh is no reason to stop serving it.
/// * Under a sweep, no local Parquet → discharge as `absent`. The entry is
///   another node's build; this node serves nothing for the hash, which is
///   what its per-node stamp claims. Deleting it would mirror the deletion
///   fleet-wide to fix a problem this node does not have.
/// * Under a sweep, holding the file → retract. It is the previous builder's,
///   known wrong, and this builder just proved it cannot replace it. Dropping
///   it costs the badge, not the answer: the query falls back to the
///   warehouse. A retraction that itself fails leaves the hash pending, so the
///   next cycle retries rather than papering over it.
///
/// A TRANSIENT failure is deliberately terminal here — `is_database_configured`
/// has already filtered the not-declared case, so what reaches this from the
/// pre-loop is "configured but currently unreachable". A blip therefore deletes
/// the artifact, fleet-wide via `mirror_manifest_to_s3`. That is the trade, not
/// an oversight: while the sweep is open the artifact is the previous builder's
/// and is being served under the Pre-aggregated badge, and the alternative —
/// keeping the hash pending — force-rebuilds the whole workspace every tick
/// with refresh keys bypassed. It converges: no manifest entry reads as stale,
/// so the next reachable cycle rebuilds it.
///
/// One shape worth knowing: the caller `await`s this per rollup inside its
/// loop, so N rollups behind one unreachable database is N serial publish-lock
/// acquisitions and N full-manifest uploads before the first rebuild spawns.
pub async fn resolve_failed_rollup(
    sweep: &mut GenerationSweep,
    rollup_hash: &str,
    view_name: &str,
    rollup_name: &str,
    cache_dir: &std::path::Path,
    config: &PreaggWorkerConfig,
    cache: &Arc<RwLock<RefreshKeyCache>>,
) {
    if !sweep.contains(rollup_hash) {
        return;
    }

    if !retract::local_parquet_present(cache_dir, rollup_hash) {
        sweep.absent(rollup_hash);
        tracing::info!(
            view = %view_name,
            rollup = %rollup_name,
            "preagg: rebuild failed under a builder-generation sweep, but this node holds no \
             parquet for it; left the entry another node built alone"
        );
        return;
    }

    match retract::retract_rollup(
        rollup_hash,
        config.workspace_id,
        PREAGG_BUILDER_GENERATION,
        retract::Retraction::Wrong,
        &config.manifest_write_lock,
        cache,
    )
    .await
    {
        Ok(()) => {
            sweep.retracted(rollup_hash);
            tracing::warn!(
                view = %view_name,
                rollup = %rollup_name,
                "preagg: rebuild failed under a builder-generation sweep; retracted the \
                 previous builder's rollup so queries fall back to the warehouse"
            );
        }
        Err(retract_err) => tracing::warn!(
            view = %view_name,
            rollup = %rollup_name,
            error = %retract_err,
            "preagg: could not retract a previous-generation rollup; the next cycle retries it"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{GenerationSweep, preagg_ledger};

    // ── Builder-generation sweep ──────────────────────────────────────────

    fn hashes(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// A node that has published nothing: every built-and-declared hash is
    /// swept, which is the conservative starting point.
    fn no_ledger() -> preagg_ledger::RollupLedger {
        preagg_ledger::RollupLedger::default()
    }

    /// The narrowing finding #2 asked for: the sweep is populated from a
    /// manifest the whole fleet writes, and `sync_manifest_from_s3` pulls a
    /// newer one down every time someone opens the Pre-aggregation tab. Reading
    /// a status page must not hand this node 30 entries to force-rebuild — and
    /// must not put another node's current-generation artifact within reach of
    /// the failure-retraction. What this node published at this generation is
    /// not swept.
    #[tokio::test]
    async fn a_hash_this_node_already_published_is_not_swept() {
        let dir = tempfile::tempdir().expect("tempdir");
        preagg_ledger::record_built(dir.path(), "mine", 1, "orders", "mine").await;
        let ledger = preagg_ledger::RollupLedger::load(dir.path());

        let declared = hashes(&["mine", "theirs"]);
        let sweep = GenerationSweep::new(hashes(&["mine", "theirs"]), &declared, &ledger, 1);

        assert!(
            !sweep.contains("mine"),
            "this node built it at generation 1"
        );
        assert!(
            sweep.contains("theirs"),
            "an entry synced from another node is still unproven here"
        );
    }

    /// And a bump un-certifies everything, or the lever would not work.
    #[tokio::test]
    async fn a_generation_bump_re_sweeps_what_the_previous_one_certified() {
        let dir = tempfile::tempdir().expect("tempdir");
        preagg_ledger::record_built(dir.path(), "mine", 1, "orders", "mine").await;
        let ledger = preagg_ledger::RollupLedger::load(dir.path());

        let declared = hashes(&["mine"]);
        let sweep = GenerationSweep::new(hashes(&["mine"]), &declared, &ledger, 2);
        assert!(sweep.contains("mine"));
    }

    /// A rollup this node retracted as EMPTY is still this node's published
    /// decision — nothing of the previous builder's survives it — so a sweep
    /// must not reopen it and rebuild to zero rows again on every tick.
    #[tokio::test]
    async fn an_empty_rollup_is_a_published_decision_and_is_not_swept() {
        let dir = tempfile::tempdir().expect("tempdir");
        preagg_ledger::record_empty(dir.path(), "empty", 1, "orders", "daily", Some("7".into()))
            .await;
        let ledger = preagg_ledger::RollupLedger::load(dir.path());
        let declared = hashes(&["empty"]);
        assert!(!GenerationSweep::new(hashes(&["empty"]), &declared, &ledger, 1).contains("empty"));
    }

    /// The failure the sweep exists to prevent: a targeted Rebuild of one row
    /// is the natural thing to press on the first cycle after a generation
    /// bump, and under a `failed == 0` guard one success would have stamped the
    /// whole workspace clean — leaving every other rollup serving the previous
    /// builder's numbers under the Pre-aggregated badge, permanently.
    #[test]
    fn one_rollup_rebuilt_does_not_certify_the_others() {
        let declared = hashes(&["a", "b", "c"]);
        let mut sweep = GenerationSweep::new(hashes(&["a", "b", "c"]), &declared, &no_ledger(), 1);
        sweep.rebuilt("a");
        assert!(
            !sweep.is_complete(),
            "two rollups are still the old builder's"
        );
        assert_eq!(sweep.len(), 2);

        sweep.rebuilt("b");
        sweep.rebuilt("c");
        assert!(sweep.is_complete());
    }

    /// A view whose datasource isn't configured is skipped by every cycle, so
    /// holding the stamp for it would mean sweeping the whole workspace on
    /// every tick forever. It can't be queried either, for the same reason.
    #[test]
    fn a_rollup_no_cycle_can_rebuild_does_not_block_the_stamp() {
        let declared = hashes(&["a", "b"]);
        let mut sweep = GenerationSweep::new(hashes(&["a", "b"]), &declared, &no_ledger(), 1);
        sweep.rebuilt("a");
        sweep.unrebuildable("b");
        assert!(sweep.is_complete());
    }

    /// A manifest entry the layer no longer declares can never be rebuilt —
    /// nothing rebuilds it, and nothing can query it. Counting it would make the
    /// sweep unable to converge.
    #[test]
    fn an_undeclared_manifest_entry_is_not_part_of_the_sweep() {
        let declared = hashes(&["a"]);
        let sweep = GenerationSweep::new(hashes(&["a", "orphan"]), &declared, &no_ledger(), 1);
        assert_eq!(sweep.len(), 1);
        assert!(!sweep.contains("orphan"));
    }

    /// The outcome mapping the cycle actually runs, not a hand-made equivalent.
    /// `Ok(false)` is the zero-row case, where `rebuild_rollup` retracted the
    /// entry and its Parquet — nothing of the previous builder's survives, so
    /// it discharges exactly like a commit. An `Err` does not: the artifact is
    /// still there, and only a successful retraction clears it.
    #[test]
    fn only_an_outcome_that_left_nothing_behind_discharges() {
        let declared = hashes(&["a", "b", "c"]);
        let mut sweep = GenerationSweep::new(hashes(&["a", "b", "c"]), &declared, &no_ledger(), 1);

        sweep.record("a", &Ok::<bool, String>(true));
        assert!(!sweep.contains("a"), "a committed rebuild replaced it");

        sweep.record("b", &Ok::<bool, String>(false));
        assert!(!sweep.contains("b"), "a zero-row rebuild retracted it");

        sweep.record("c", &Err::<bool, String>("boom".to_string()));
        assert!(
            sweep.contains("c"),
            "a failed rebuild replaced nothing and retracted nothing"
        );
        assert!(!sweep.is_complete());
    }

    /// Why the sweep can always terminate: the one outcome that leaves an old
    /// artifact in place is a failed rebuild, and a failure under the sweep is
    /// followed by a retraction. Without that, an unbuildable rollup would hold
    /// the stamp forever — and an open sweep force-rebuilds the WHOLE workspace
    /// every tick, refresh keys bypassed.
    #[test]
    fn a_retraction_is_what_lets_an_unbuildable_rollup_stop_blocking() {
        let declared = hashes(&["a", "broken"]);
        let mut sweep = GenerationSweep::new(hashes(&["a", "broken"]), &declared, &no_ledger(), 1);

        sweep.record("a", &Ok::<bool, String>(true));
        sweep.record("broken", &Err::<bool, String>("view sql is invalid".into()));
        assert!(
            !sweep.is_complete(),
            "the old parquet is still being served"
        );

        sweep.retracted("broken");
        assert!(sweep.is_complete(), "nothing old is served any more");
    }

    /// Nothing built yet means nothing to invalidate, so the very first cycle
    /// on a fresh node stamps immediately rather than forcing a pointless sweep.
    #[test]
    fn a_node_that_has_built_nothing_is_already_complete() {
        let sweep = GenerationSweep::new(HashSet::new(), &hashes(&["a", "b"]), &no_ledger(), 1);
        assert!(sweep.is_complete());
    }
}
