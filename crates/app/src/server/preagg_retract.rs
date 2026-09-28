//! Removing a rollup this builder cannot replace.
//!
//! The third responsibility of the pre-aggregation cycle, beside building
//! (`preagg_rebuild`) and publishing: a stored artifact is known WRONG and
//! this builder cannot produce a right one, so the entry and its Parquet go
//! rather than keep answering. Removal is not data loss — the read path falls
//! back to the warehouse and answers correctly, just without the badge —
//! whereas leaving them means serving numbers this builder would not produce.
//!
//! Two callers, and the difference between them is what happens to the
//! FRESHNESS record, not to the artifact:
//!
//! * `Retraction::Empty` — the rebuild ran and the rollup is genuinely empty
//!   now. The artifact must go (an entry pointing at the previous build serves
//!   last period's numbers under the Pre-aggregated badge), but the attempt is
//!   real and stays on record in the ledger. Erasing it too would make a
//!   legitimately empty rollup read as never-built and rebuild on every
//!   cadence tick forever.
//! * `Retraction::Wrong` — a rebuild failed while a builder-generation sweep
//!   was forcing it, so what is on disk is the previous builder's. Nothing
//!   about that attempt should suppress or certify anything, so the ledger
//!   entry is dropped with the artifact.
//!
//! They also differ in REACH, and for the same reason. `Empty` is a statement
//! about a rollup — "`(view, rollup)` has no rows now" — so it removes every
//! entry that identity holds, whatever hash each was built under. `Wrong` is a
//! statement about one artifact the sweep is holding a hash for, so it removes
//! exactly that hash.

use std::sync::{Arc, RwLock};

use tokio::sync::Mutex as TokioMutex;
use uuid::Uuid;

use agentic_semantic::refresh_key_cache::RefreshKeyCache;

use super::preagg_ledger;
use super::preagg_rebuild::{mirror_manifest_to_s3, same_rollup_identity};

/// Why the artifact is going, which decides what is remembered about it.
#[derive(Debug, Clone)]
pub(super) enum Retraction {
    /// Zero rows. The probed refresh-key value rides along so the next tick's
    /// staleness check has something to compare against — the manifest fields
    /// it would normally read are about to be deleted — and the names because
    /// they are what is actually retracted: `(view, rollup)` is the identity
    /// the status endpoint joins on and the one an empty answer is ABOUT, and
    /// the hash the rebuild was for may be a hash the manifest has never seen
    /// (any airlayer change to `compute_rollup_hash` moves it) while the entry
    /// still being served sits under the old one.
    Empty {
        view: String,
        rollup: String,
        refresh_key_value: Option<String>,
    },
    /// The previous builder's, unreplaceable by this cycle.
    Wrong,
}

/// Drop one rollup from the local manifest and delete its Parquet, then tell
/// the other nodes.
///
/// Takes the per-workspace publish lock itself; call
/// [`retract_under_publish_lock`] from a caller already holding it.
///
/// It travels: the write bumps `pulled_at` and mirrors, so `sync_manifest_from_s3`
/// carries the retraction to other nodes by the same recency rule it carries a
/// build with. The mirrored Parquet is left in the bucket — there is no delete
/// path, and nothing references it once the entry is gone.
pub(super) async fn retract_rollup(
    rollup_hash: &str,
    workspace_id: Uuid,
    generation: u32,
    reason: Retraction,
    manifest_write_lock: &Arc<TokioMutex<()>>,
    cache: &Arc<RwLock<RefreshKeyCache>>,
) -> Result<(), String> {
    let cache_dir = oxy_shared::state_dir::get_airlayer_cache_dir(workspace_id);
    let publish = manifest_write_lock.lock().await;
    let retracted =
        retract_under_publish_lock(rollup_hash, &cache_dir, generation, reason, cache).await;
    drop(publish);

    // Mirroring is outside the lock: best-effort network I/O against a manifest
    // already written locally, and holding a per-workspace lock across an S3
    // round-trip would serialize every rollup in the workspace behind the
    // slowest upload.
    if retracted? {
        mirror_manifest_to_s3(&cache_dir).await;
    }
    Ok(())
}

/// The retraction itself, with the publish lock already held.
///
/// Returns `Ok(false)` when there was nothing to retract: a hash the manifest
/// never had is the state this asks for, not an error — a cycle retracting a
/// rollup a concurrent one already dropped must not fail and leave the sweep
/// pending. The caller uses the flag to skip re-uploading an unchanged
/// manifest.
///
/// What gets removed is `rollup_hash`'s entry PLUS, for `Retraction::Empty`,
/// every other entry the same `(view, rollup)` identity holds — the same
/// question `commit_manifest_and_cache`'s reap asks, through the same
/// predicate ([`same_rollup_identity`]), so the success and zero-row paths
/// cannot disagree about what this rollup is still serving. `Retraction::Wrong`
/// stays hash-scoped: the sweep is holding one specific artifact's hash and has
/// checked that this node holds ITS file (`local_parquet_present`), so widening
/// it would destroy entries the sweep never examined.
pub(super) async fn retract_under_publish_lock(
    rollup_hash: &str,
    cache_dir: &std::path::Path,
    generation: u32,
    reason: Retraction,
    cache: &Arc<RwLock<RefreshKeyCache>>,
) -> Result<bool, String> {
    let cache_dir_owned = cache_dir.to_path_buf();
    let rollup_hash_owned = rollup_hash.to_string();
    let identity = match &reason {
        Retraction::Empty { view, rollup, .. } => Some((view.clone(), rollup.clone())),
        Retraction::Wrong => None,
    };

    let removed = tokio::task::spawn_blocking(move || {
        let Some(mut manifest) = agentic_semantic::preagg::load_local_manifest(&cache_dir_owned)
        else {
            return Ok::<Vec<(String, String)>, String>(vec![]);
        };
        let mut removed: Vec<(String, String)> = Vec::new();
        manifest.rollups.retain(|r| {
            let target = r.rollup_hash == rollup_hash_owned
                || identity
                    .as_ref()
                    .is_some_and(|(view, rollup)| same_rollup_identity(r, view, rollup));
            if target {
                removed.push((r.rollup_hash.clone(), r.file.clone()));
            }
            !target
        });
        if removed.is_empty() {
            // Nothing changed, so the manifest is not rewritten: `pulled_at`
            // is what carries a retraction across the fleet, and bumping it
            // for a no-op would re-date every OTHER node's entries.
            return Ok(vec![]);
        }
        manifest.pulled_at = chrono::Utc::now().to_rfc3339();
        // Manifest first, file second: a reader that sees the entry gone never
        // looks for the file, whereas deleting first would leave a window where
        // the entry resolves to a path that no longer exists — the failure mode
        // `commit_manifest_and_cache` exists to avoid, in reverse.
        agentic_semantic::preagg::save_local_manifest(&cache_dir_owned, &manifest)
            .map_err(|e| e.to_string())?;
        // A file a surviving entry still names is not ours to delete. Belt and
        // braces — the `{view}__{hash}.parquet` shape rules out two identities
        // resolving to one file — but the cost of being wrong is deleting a
        // live rollup, so it is checked rather than argued. Same guard, same
        // reason, as the reap's.
        removed.retain(|(_, file)| !manifest.rollups.iter().any(|r| &r.file == file));
        Ok(removed)
    })
    .await
    .map_err(|e| format!("manifest retraction task panicked: {e}"))??;

    // The freshness record is settled whether or not the manifest still had an
    // entry: a zero-row rebuild that raced a concurrent retraction still ran,
    // and its answer is still the one the next tick should be measured against.
    match &reason {
        Retraction::Empty {
            view,
            rollup,
            refresh_key_value,
        } => {
            {
                // `invalidate`, like the `Wrong` arm — the LEDGER below is what
                // keeps the attempt on record.
                //
                // This used to `insert`, so the in-memory layer would vouch for
                // the empty answer and spare the next tick a ledger read. That
                // made the suppression window unbounded by anything this node
                // controls: `eval_every_refresh_key`'s layer 1 trusts an entry
                // for the full refresh interval, and the cycle only sweeps the
                // cache at `2 × renewal_threshold` — operator-configurable up to
                // an hour — so an empty rollup on a 6h key could stay suppressed
                // for two hours regardless of `EMPTY_RETRY_CEILING`. Layer 1
                // cannot tell an empty-seeded entry from a real build's without
                // reading the ledger first, which is the read the seed existed
                // to avoid. So the seed goes, and the ceiling becomes exact.
                //
                // Clearing is not merely neutral here: the entry this removes is
                // the PREVIOUS, non-empty build's, which layer 1 would otherwise
                // keep reading as build recency for a rollup whose artifact was
                // just deleted.
                let mut guard = cache.write().expect("preagg cache lock poisoned");
                guard.invalidate(rollup_hash);
            }
            preagg_ledger::record_empty(
                cache_dir,
                rollup_hash,
                generation,
                view,
                rollup,
                refresh_key_value.clone(),
            )
            .await;
        }
        Retraction::Wrong => {
            {
                let mut guard = cache.write().expect("preagg cache lock poisoned");
                guard.invalidate(rollup_hash);
            }
            preagg_ledger::forget(cache_dir, rollup_hash).await;
        }
    }

    if removed.is_empty() {
        return Ok(false);
    }

    let (why, view, rollup) = match &reason {
        Retraction::Empty { view, rollup, .. } => ("empty", view.as_str(), rollup.as_str()),
        Retraction::Wrong => ("previous builder's", "", ""),
    };
    for (hash, file) in &removed {
        // The Parquet is best-effort: nothing references it once the entry is
        // gone, so a failed unlink costs disk, not correctness. An ABSENT file
        // is ordinary — any node that did not run the build holds the
        // fleet-synced manifest and no Parquet.
        let path = cache_dir.join(file);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => tracing::info!(
                view = %view,
                rollup = %rollup,
                why,
                retracted_hash = %hash,
                rebuilt_hash = %rollup_hash,
                file = %file,
                "preagg: retracted a rollup — its manifest entry and local parquet are gone, \
                 and queries fall back to the warehouse"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => tracing::info!(
                view = %view,
                rollup = %rollup,
                why,
                retracted_hash = %hash,
                rebuilt_hash = %rollup_hash,
                file = %file,
                "preagg: retracted a rollup's manifest entry; its parquet was not on this \
                 node, which is normal for a manifest synced from the fleet"
            ),
            Err(e) => tracing::warn!(
                error = %e,
                retracted_hash = %hash,
                file = %file,
                "preagg: retracted the manifest entry but could not delete its parquet"
            ),
        }
    }

    Ok(true)
}

/// Does this node hold the Parquet the manifest's entry for `rollup_hash`
/// points at?
///
/// The question the failure-retraction has to ask before destroying a
/// fleet-wide entry. A node with no local file is serving nothing wrong from
/// it — whatever is being served comes from another node's build — so deleting
/// the entry buys nothing here and costs a good artifact everywhere.
pub(super) fn local_parquet_present(cache_dir: &std::path::Path, rollup_hash: &str) -> bool {
    agentic_semantic::preagg::load_local_manifest(cache_dir)
        .and_then(|m| {
            m.rollups
                .into_iter()
                .find(|r| r.rollup_hash == rollup_hash)
                .map(|r| cache_dir.join(r.file).is_file())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use agentic_semantic::refresh_key_cache::RefreshKeyCache;

    use super::{Retraction, local_parquet_present, retract_under_publish_lock};
    use crate::server::preagg_ledger::RollupLedger;

    fn manifest_json(entries: &[(&str, &str)]) -> String {
        let rollups: Vec<String> = entries
            .iter()
            .map(|(hash, file)| {
                format!(
                    r#"{{"view_name":"orders","rollup_name":"daily","rollup_hash":"{hash}",
                        "file":"{file}","dimensions":[],"measures":[],
                        "time_dimension":null,"granularity":null,
                        "build_date":"20260826T000000","refresh_key_value":"7",
                        "refresh_key_checked_at":null}}"#
                )
            })
            .collect();
        format!(
            r#"{{"pulled_at":"2026-08-26T00:00:00Z","source_database":"wh","rollups":[{}]}}"#,
            rollups.join(",")
        )
    }

    /// An empty rollup must stop being served, not keep answering from the last
    /// non-empty build.
    ///
    /// The ordering this function argues for — manifest entry before file — is
    /// NOT pinned here and cannot be from outside: both writes have landed by
    /// the time the call returns, so no observer placed here can see the window
    /// between them. The claim is load-bearing, so it is guarded by the comment
    /// at the write site rather than by this test's name.
    #[tokio::test]
    async fn retracting_removes_the_entry_and_its_parquet_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache_dir = dir.path();
        std::fs::write(
            cache_dir.join("manifest.json"),
            manifest_json(&[
                ("gone", "orders__gone.parquet"),
                ("kept", "orders__kept.parquet"),
            ]),
        )
        .expect("seed manifest");
        std::fs::write(cache_dir.join("orders__gone.parquet"), b"old").expect("seed parquet");
        std::fs::write(cache_dir.join("orders__kept.parquet"), b"keep").expect("seed parquet");

        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        let removed = retract_under_publish_lock("gone", cache_dir, 1, Retraction::Wrong, &cache)
            .await
            .expect("retraction succeeds");

        assert!(removed, "something was actually removed");
        let manifest = agentic_semantic::preagg::load_local_manifest(cache_dir)
            .expect("manifest still parses");
        let hashes: Vec<&str> = manifest
            .rollups
            .iter()
            .map(|r| r.rollup_hash.as_str())
            .collect();
        assert_eq!(hashes, vec!["kept"], "only the retracted entry is dropped");
        assert!(!cache_dir.join("orders__gone.parquet").exists());
        assert!(
            cache_dir.join("orders__kept.parquet").exists(),
            "the other rollup's file is untouched"
        );
    }

    /// Retracting what was never there is the state being asked for, not an
    /// error — and it must report that nothing changed, so the caller does not
    /// re-upload an unchanged manifest.
    #[tokio::test]
    async fn retracting_an_absent_rollup_is_not_an_error_and_changes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        let removed =
            retract_under_publish_lock("nothing", dir.path(), 1, Retraction::Wrong, &cache)
                .await
                .expect("no manifest at all is fine");
        assert!(!removed, "nothing to mirror");

        std::fs::write(
            dir.path().join("manifest.json"),
            manifest_json(&[("kept", "f.parquet")]),
        )
        .expect("seed manifest");
        let removed =
            retract_under_publish_lock("nothing", dir.path(), 1, Retraction::Wrong, &cache)
                .await
                .expect("a hash the manifest never had is fine");
        assert!(!removed, "an untouched manifest is not re-uploaded");
        assert_eq!(
            agentic_semantic::preagg::load_local_manifest(dir.path())
                .expect("manifest still parses")
                .rollups
                .len(),
            1
        );
    }

    /// The regression behind finding #1: an empty rollup that forgets its own
    /// attempt is rebuilt on every cadence tick forever.
    #[tokio::test]
    async fn an_empty_retraction_keeps_the_attempt_on_record() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("manifest.json"),
            manifest_json(&[("gone", "orders__gone.parquet")]),
        )
        .expect("seed manifest");
        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        // What the build that produced rows left behind. The retraction has to
        // clear it: the artifact it vouches for is the one being deleted.
        cache
            .write()
            .expect("cache lock")
            .insert("gone".to_string(), Some("6".to_string()));

        retract_under_publish_lock(
            "gone",
            dir.path(),
            1,
            Retraction::Empty {
                view: "orders".into(),
                rollup: "daily".into(),
                refresh_key_value: Some("7".into()),
            },
            &cache,
        )
        .await
        .expect("retraction succeeds");

        assert!(
            cache
                .read()
                .expect("cache lock")
                .get("gone", std::time::Duration::from_secs(3600))
                .is_none(),
            "the record is the ledger's, not the in-memory layer's — an entry here would \
             suppress the rebuild for the cache's own sweep window, which is configurable \
             past EMPTY_RETRY_CEILING"
        );
        let ledger = RollupLedger::load(dir.path());
        assert_eq!(
            ledger
                .empty_record("gone")
                .and_then(|e| e.refresh_key_value.clone()),
            Some("7".to_string()),
            "and it survives a restart"
        );
    }

    /// The other half: a retraction of a WRONG artifact must not leave anything
    /// behind that could suppress the rebuild that replaces it.
    #[tokio::test]
    async fn a_wrong_retraction_forgets_everything_about_the_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("manifest.json"),
            manifest_json(&[("gone", "orders__gone.parquet")]),
        )
        .expect("seed manifest");
        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        cache
            .write()
            .expect("cache lock")
            .insert("gone".to_string(), Some("7".to_string()));
        crate::server::preagg_ledger::record_built(dir.path(), "gone", 1, "orders", "daily").await;

        retract_under_publish_lock("gone", dir.path(), 1, Retraction::Wrong, &cache)
            .await
            .expect("retraction succeeds");

        assert!(
            cache
                .read()
                .expect("cache lock")
                .get("gone", std::time::Duration::from_secs(3600))
                .is_none(),
            "the refresh-key cache no longer vouches for a rollup that is gone"
        );
        assert!(
            !RollupLedger::load(dir.path()).is_at_generation("gone", 1),
            "and this node no longer claims to have built it"
        );
    }

    #[test]
    fn a_manifest_entry_whose_parquet_is_absent_is_not_local() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("manifest.json"),
            manifest_json(&[
                ("here", "orders__here.parquet"),
                ("there", "orders__there.parquet"),
            ]),
        )
        .expect("seed manifest");
        std::fs::write(dir.path().join("orders__here.parquet"), b"rows").expect("seed parquet");

        assert!(local_parquet_present(dir.path(), "here"));
        assert!(
            !local_parquet_present(dir.path(), "there"),
            "listed in the manifest another node mirrored, but not on this disk"
        );
        assert!(!local_parquet_present(dir.path(), "unknown"));
    }
}
