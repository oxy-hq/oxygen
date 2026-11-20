//! What THIS node's builder actually did to each rollup hash.
//!
//! The manifest cannot answer that. It is a fleet-wide artifact — every
//! rebuild rewrites the whole file, `mirror_manifest_to_s3` uploads it, and
//! `sync_manifest_from_s3` pulls a newer one down on every status read — so an
//! entry in it means "somebody built this", never "this node built this, with
//! this builder". Two decisions need the stronger fact:
//!
//! * A builder-generation sweep asks "is the artifact behind this hash the
//!   PREVIOUS builder's?". Answered from the manifest alone it is an inference
//!   from fleet state, and a node whose stamp is behind would sweep — and on a
//!   transient failure retract — entries another node had already rebuilt
//!   correctly, mirroring the deletion fleet-wide. The `generation` field makes
//!   the claim checkable: a hash this node committed at the current generation
//!   is not swept at all.
//! * A zero-row rebuild RETRACTS its entry (see `preagg_retract`), which erases
//!   the manifest's `build_date` and `refresh_key_value` — the two fields both
//!   staleness evaluators read. Without a record of the attempt, a legitimately
//!   empty rollup reads as never-built and rebuilds on every cadence tick
//!   forever. `empty` is that record: the rebuild ran, its honest answer was
//!   "no rows", and the refresh key still gets to suppress the next tick.
//!
//! Lives beside `builder_generation` in the workspace's airlayer cache dir and
//! is deliberately NOT part of `manifest.json`: the manifest's shape is
//! airlayer's, and anything stored in it would be mirrored and re-synced —
//! which is exactly the property that disqualifies it from answering either
//! question.
//!
//! Every mutator is load-modify-save on one small file, so callers must hold
//! the per-workspace publish lock — the same lock `commit_manifest_and_cache`
//! and `retract_manifest_and_cache` require, and for the same reason: the
//! ledger entry and the manifest entry are one publish.
//!
//! Lives in this crate beside [`crate::preagg_blob`], the same cache's
//! cross-node half (the manifest and Parquet mirror), since 2026-11-16 (Wave
//! D2). It was a private module of `oxy-app`'s preagg runtime, which still
//! writes it (the rebuild, the retraction, the generation sweep, the freshness
//! evaluators — `rebuild`, `retract`, `generation` and `freshness`, then in
//! `oxy-app`, `oxy-preagg` since 2026-11-20); it came down so the IDE's
//! pre-aggregation status (`oxy-api-semantic`'s `preagg`) can read
//! [`empty_rollups`] without reaching into `oxy-app`. The body is unchanged;
//! only the visibility widened from `oxy-app`'s `server` module to `pub`.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

const LEDGER_FILE: &str = "rollup_ledger.json";

/// The zero-row record: a rebuild ran and produced nothing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmptyRecord {
    /// RFC3339 instant of the zero-row rebuild — what an `every:` interval is
    /// measured against once the manifest's `build_date` is gone.
    pub at: String,
    /// The refresh-key value probed for that rebuild, so a `sql:` key compares
    /// against what the empty answer was actually for. `None` for `every:`
    /// keys, which carry no value.
    #[serde(default)]
    pub refresh_key_value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct LedgerEntry {
    /// `PREAGG_BUILDER_GENERATION` as of this node's last published decision
    /// about the hash — a commit or a zero-row retraction.
    pub generation: u32,
    /// The rollup this hash was, by the names the UI addresses it with.
    ///
    /// Not for the status join — that keys on hash, deliberately, so a
    /// superseded spec's record cannot describe its replacement. These are what
    /// let a WRITE enforce one live entry per logical rollup
    /// ([`RollupLedger::insert_replacing_same_rollup`]): the hash is the only
    /// thing a rebuild has in hand, and "is this the same rollup, re-hashed?"
    /// is a question the hash by definition cannot answer. They also give the
    /// retraction log line something a person can read.
    #[serde(default)]
    pub view: String,
    #[serde(default)]
    pub rollup: String,
    /// Present only while the last rebuild's answer was "no rows". Cleared by
    /// the next commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty: Option<EmptyRecord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RollupLedger {
    #[serde(default)]
    entries: HashMap<String, LedgerEntry>,
}

impl RollupLedger {
    /// A missing or unreadable ledger is empty, never an error: it means this
    /// node has published nothing it can vouch for, which is the conservative
    /// answer for both callers — the sweep covers everything, and no freshness
    /// record suppresses a rebuild.
    pub fn load(cache_dir: &Path) -> Self {
        std::fs::read_to_string(cache_dir.join(LEDGER_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Did THIS node publish this hash under the current builder generation?
    pub fn is_at_generation(&self, hash: &str, generation: u32) -> bool {
        self.entries
            .get(hash)
            .is_some_and(|e| e.generation == generation)
    }

    pub fn empty_record(&self, hash: &str) -> Option<&EmptyRecord> {
        self.entries.get(hash).and_then(|e| e.empty.as_ref())
    }

    /// Record a decision about `hash`, dropping any OTHER entry describing the
    /// same logical rollup.
    ///
    /// A rollup's hash covers its dimensions, measures, time dimension and
    /// granularity, so editing any of them re-hashes it — while the name the
    /// status endpoint joins on stays put. Without this, editing a rollup that
    /// currently has no rows strands the old hash's record under names that
    /// still resolve: a brand-new spec renders "Empty — the last rebuild found
    /// no rows" for something nothing has ever attempted, and if the new hash
    /// is also empty there are two records for one `(view, rollup)`. The status
    /// join would then pick between them by `HashMap` iteration order — which
    /// is unspecified and can differ between two requests in the same process,
    /// so the timestamp the tab polls on flips, and a rebuilding row either
    /// spins to its deadline or clears before the rebuild finished.
    ///
    /// One live entry per logical rollup is what the join already assumes.
    fn insert_replacing_same_rollup(&mut self, hash: String, entry: LedgerEntry) {
        self.entries
            .retain(|h, e| *h == hash || !(e.view == entry.view && e.rollup == entry.rollup));
        self.entries.insert(hash, entry);
    }

    /// Forget every hash outside `live` — what the manifest lists plus what the
    /// layer declares.
    ///
    /// Nothing else bounds this file: a hash that stops existing (a rollup
    /// edited, renamed, or deleted) is never written again and never removed,
    /// so without a sweep the ledger grows with every hash the node has ever
    /// built and is re-read on every 3s status poll.
    fn prune(&mut self, live: &std::collections::HashSet<String>) -> usize {
        let before = self.entries.len();
        self.entries.retain(|hash, _| live.contains(hash));
        before - self.entries.len()
    }

    /// Write via a temp file and rename, not in place.
    ///
    /// Every reader is lock-free — `read_cache_facts` on each status poll, and
    /// both staleness evaluators, which run before the publish lock is taken —
    /// so a plain truncate-then-write is observable half-written. `load`
    /// swallows that as "empty", and the conservative answer is not free: the
    /// `every:` evaluator loses its zero-row layer and rebuilds this tick, and
    /// the status row flips Empty → Not built for one poll. Rename is atomic,
    /// which is the same argument the Parquet hot-swap already makes.
    fn save(&self, cache_dir: &Path) -> Result<(), String> {
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
        // Pid-scoped, so two processes writing the same workspace's ledger
        // never share a staging file and rename each other's bytes into place.
        let tmp = cache_dir.join(format!("{LEDGER_FILE}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, cache_dir.join(LEDGER_FILE)).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })
    }
}

/// Apply `mutate` to the ledger on disk. Best-effort by design: the ledger is
/// an optimisation over re-deriving both facts the expensive way (sweep
/// everything; rebuild an empty rollup every tick), so a failed write costs
/// redundant work, never correctness — and must not fail a rebuild that has
/// already committed its manifest entry.
async fn update(cache_dir: &Path, mutate: impl FnOnce(&mut RollupLedger) -> bool + Send + 'static) {
    let cache_dir = cache_dir.to_path_buf();
    let write = tokio::task::spawn_blocking(move || {
        let mut ledger = RollupLedger::load(&cache_dir);
        // `mutate` reports whether anything actually changed. A no-op prune —
        // the common case, once per cycle per workspace — would otherwise be a
        // write plus a rename inside the publish lock's critical section, and
        // would churn the file's mtime for nothing.
        if !mutate(&mut ledger) {
            return Ok(());
        }
        ledger.save(&cache_dir)
    })
    .await;

    match write {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(
            error = %e,
            "preagg: could not write the rollup ledger; the next cycle re-sweeps this workspace"
        ),
        Err(e) => tracing::warn!(error = %e, "preagg: rollup ledger write task panicked"),
    }
}

/// This node committed a real (non-empty) build for `hash`.
pub async fn record_built(cache_dir: &Path, hash: &str, generation: u32, view: &str, rollup: &str) {
    let hash = hash.to_string();
    let (view, rollup) = (view.to_string(), rollup.to_string());
    update(cache_dir, move |ledger| {
        ledger.insert_replacing_same_rollup(
            hash,
            LedgerEntry {
                generation,
                view,
                rollup,
                // Rows again: the zero-row record would otherwise keep
                // suppressing rebuilds against a manifest entry that now
                // carries its own, newer freshness fields.
                empty: None,
            },
        );
        true
    })
    .await;
}

/// This node's rebuild of `hash` produced zero rows, so its entry was
/// retracted. The attempt is still on record — that is the whole point.
pub async fn record_empty(
    cache_dir: &Path,
    hash: &str,
    generation: u32,
    view: &str,
    rollup: &str,
    refresh_key_value: Option<String>,
) {
    let hash = hash.to_string();
    let at = chrono::Utc::now().to_rfc3339();
    let view = view.to_string();
    let rollup = rollup.to_string();
    update(cache_dir, move |ledger| {
        ledger.insert_replacing_same_rollup(
            hash,
            LedgerEntry {
                generation,
                view,
                rollup,
                empty: Some(EmptyRecord {
                    at,
                    refresh_key_value,
                }),
            },
        );
        true
    })
    .await;
}

/// Drop every hash the workspace no longer has: not in the manifest, not
/// declared by the layer this cycle loaded. Called once per cycle, under the
/// publish lock.
pub async fn prune(cache_dir: &Path, live: std::collections::HashSet<String>) {
    let dir = cache_dir.to_path_buf();
    update(&dir, move |ledger| {
        let dropped = ledger.prune(&live);
        if dropped > 0 {
            tracing::debug!(
                dropped,
                "preagg: pruned ledger entries for hashes that no longer exist"
            );
        }
        dropped > 0
    })
    .await;
}

/// Every rollup this node last rebuilt to zero rows, as `hash -> instant`.
///
/// Reading it is how a retracted row reports "Empty" rather than the "Not
/// built" it is otherwise indistinguishable from.
///
/// Keyed by HASH, which is what the record is actually about. A rollup whose
/// `dimensions:` were edited is a different rollup, and its predecessor's empty
/// answer says nothing about it — so the caller resolves the currently-declared
/// hash and looks that up, rather than joining on names that outlive the spec
/// they described. Without that, an edited rollup reads "Empty — the last
/// rebuild found no rows" until the next cycle prunes the old hash.
pub fn empty_rollups(cache_dir: &Path) -> HashMap<String, String> {
    RollupLedger::load(cache_dir)
        .entries
        .into_iter()
        .filter_map(|(hash, e)| e.empty.map(|empty| (hash, empty.at)))
        .collect()
}

/// Drop everything this node knew about `hash`.
///
/// Used where the artifact was removed because it was WRONG rather than empty
/// — the failure-retraction under a sweep. Nothing about that attempt should
/// suppress the next rebuild, and nothing about it certifies a generation.
pub async fn forget(cache_dir: &Path, hash: &str) {
    let hash = hash.to_string();
    update(cache_dir, move |ledger| {
        ledger.entries.remove(&hash).is_some()
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_absent_ledger_vouches_for_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ledger = RollupLedger::load(dir.path());
        assert!(!ledger.is_at_generation("a", 1));
        assert!(ledger.empty_record("a").is_none());
    }

    #[tokio::test]
    async fn a_corrupt_ledger_reads_as_empty_rather_than_failing() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(LEDGER_FILE), b"{not json").expect("seed");
        assert!(!RollupLedger::load(dir.path()).is_at_generation("a", 1));
        // And it is repairable in place, not stuck.
        record_built(dir.path(), "a", 1, "orders", "daily").await;
        assert!(RollupLedger::load(dir.path()).is_at_generation("a", 1));
    }

    #[tokio::test]
    async fn a_build_is_recorded_at_its_generation_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        record_built(dir.path(), "a", 2, "orders", "daily").await;
        let ledger = RollupLedger::load(dir.path());
        assert!(ledger.is_at_generation("a", 2));
        assert!(
            !ledger.is_at_generation("a", 3),
            "a bump must not inherit the previous generation's certificate"
        );
    }

    #[tokio::test]
    async fn a_zero_row_rebuild_keeps_its_probed_value_on_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        record_empty(dir.path(), "a", 1, "orders", "daily", Some("7".into())).await;
        let ledger = RollupLedger::load(dir.path());
        let empty = ledger.empty_record("a").expect("the attempt is on record");
        assert_eq!(empty.refresh_key_value.as_deref(), Some("7"));
        assert!(
            ledger.is_at_generation("a", 1),
            "an empty rollup is still a published decision of this builder's"
        );
    }

    #[tokio::test]
    async fn rows_again_clears_the_zero_row_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        record_empty(dir.path(), "a", 1, "orders", "daily", Some("7".into())).await;
        record_built(dir.path(), "a", 1, "orders", "daily").await;
        assert!(RollupLedger::load(dir.path()).empty_record("a").is_none());
    }

    /// Finding #1's first half: a rollup's hash covers its dimensions and
    /// granularity, so editing one re-hashes it while the names the status
    /// endpoint joins on stay put. The old hash's zero-row record must not
    /// survive to describe a spec nothing has attempted.
    #[tokio::test]
    async fn re_hashing_a_rollup_drops_the_old_hash_s_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        record_empty(dir.path(), "h1", 1, "orders", "daily", None).await;
        record_empty(dir.path(), "h2", 1, "orders", "daily", Some("7".into())).await;

        let listed = empty_rollups(dir.path());
        assert_eq!(
            listed.len(),
            1,
            "one live entry per (view, rollup), not one per hash ever seen"
        );
        assert!(listed.contains_key("h2"), "and it is the current hash's");
        let ledger = RollupLedger::load(dir.path());
        assert!(ledger.empty_record("h1").is_none(), "the old hash is gone");
        assert!(ledger.empty_record("h2").is_some());
    }

    /// The second half, and the one that bit: two records for one
    /// `(view, rollup)` made the status join pick by `HashMap` iteration order,
    /// so the timestamp the tab polls on flipped between requests and a
    /// rebuilding row either spun to its deadline or cleared early.
    #[tokio::test]
    async fn a_rebuild_with_rows_also_clears_a_stale_sibling_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        record_empty(dir.path(), "h1", 1, "orders", "daily", None).await;
        record_built(dir.path(), "h2", 1, "orders", "daily").await;

        assert!(
            empty_rollups(dir.path()).is_empty(),
            "the rollup has rows now; nothing may still report it empty"
        );
        // A different rollup on the same view is untouched — the match is on
        // both names, not just the view.
        record_empty(dir.path(), "h3", 1, "orders", "weekly", None).await;
        record_built(dir.path(), "h4", 1, "orders", "daily").await;
        assert_eq!(empty_rollups(dir.path()).len(), 1);
    }

    #[tokio::test]
    async fn pruning_drops_hashes_the_workspace_no_longer_has() {
        let dir = tempfile::tempdir().expect("tempdir");
        record_built(dir.path(), "live", 1, "orders", "daily").await;
        record_empty(dir.path(), "stale", 1, "orders", "weekly", None).await;

        prune(dir.path(), ["live".to_string()].into_iter().collect()).await;

        let ledger = RollupLedger::load(dir.path());
        assert!(ledger.is_at_generation("live", 1));
        assert!(
            !ledger.is_at_generation("stale", 1),
            "an edited-away hash would otherwise accumulate forever"
        );
    }

    /// A half-written ledger must never be observable: every reader is
    /// lock-free, and one that parses nothing rebuilds an empty rollup or flips
    /// its row to "Not built" for a poll.
    #[tokio::test]
    async fn a_write_leaves_no_partial_file_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        record_built(dir.path(), "a", 1, "orders", "daily").await;
        let strays: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readdir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "staging file left behind: {strays:?}");
        assert!(RollupLedger::load(dir.path()).is_at_generation("a", 1));
    }

    #[tokio::test]
    async fn forgetting_removes_the_generation_certificate_too() {
        let dir = tempfile::tempdir().expect("tempdir");
        record_built(dir.path(), "a", 1, "orders", "daily").await;
        record_built(dir.path(), "b", 1, "orders", "weekly").await;
        forget(dir.path(), "a").await;
        let ledger = RollupLedger::load(dir.path());
        assert!(
            !ledger.is_at_generation("a", 1),
            "a failure-retraction certifies nothing, so the next cycle rebuilds it"
        );
        assert!(ledger.is_at_generation("b", 1), "and leaves the rest alone");
    }
}
