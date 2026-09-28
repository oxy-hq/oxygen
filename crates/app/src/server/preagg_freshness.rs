//! Deciding whether a rollup is STALE.
//!
//! One question, two kinds of answer, and a two-layer cache under each. An
//! `every:` key is a cadence — measured against the last build, wherever that
//! is recorded; a `sql:` key is a probe — a value compared against the one the
//! last build was for. Both read through the same three places, in the same
//! order, because the manifest is not the only record of a build: a rollup
//! that rebuilt to ZERO rows has no manifest entry at all (`preagg_retract`
//! removed it), and the node-local ledger is the only surviving evidence it
//! ran.
//!
//! Split out of `preagg_executor` for size, and because every function here
//! takes a `cache_dir` rather than reaching for the process's state dir —
//! which is what makes the "second cycle after a zero-row rebuild" case
//! testable at all.

use std::sync::{Arc, RwLock};

use agentic_automation::workspace::WorkspaceContext;
use agentic_semantic::refresh_key_cache::RefreshKeyCache;

use super::preagg_ledger;
use crate::agentic_wiring::OxyProjectContext;

/// The refresh key that governs one rollup: the per-rollup `refresh_key:` if
/// the declaration carries one, else the view-level key. `None` means the
/// worker skips this rollup — `api::semantic::build_preagg_status` filters on
/// the same rule so the IDE never lists a rollup that will never be built.
pub(crate) fn rollup_refresh_key<'a>(
    rollup: &'a oxy_airlayer_compat::preagg::RollupSpec,
    view: &'a oxy_airlayer_compat::View,
) -> Option<&'a oxy_airlayer_compat::RefreshKey> {
    if let Some(ref preaggs) = view.pre_aggregations {
        for pa in preaggs {
            if pa.name == rollup.name {
                if let Some(ref k) = pa.refresh_key {
                    return Some(k);
                }
                break;
            }
        }
    }
    view.refresh_key.as_ref()
}

/// Dispatch to the appropriate refresh-key evaluator based on key kind.
///
/// Returns `(current_value, is_stale, error_msg)`.
pub(super) async fn evaluate_refresh_key(
    rk: &oxy_airlayer_compat::RefreshKey,
    rollup_hash: &str,
    cache_dir: &std::path::Path,
    cache: &Arc<RwLock<RefreshKeyCache>>,
    ctx: &OxyProjectContext,
    database_name: &str,
) -> (Option<String>, bool, Option<String>) {
    match rk {
        oxy_airlayer_compat::RefreshKey::Every(interval_str) => {
            let (value, is_stale) =
                eval_every_refresh_key(interval_str, rollup_hash, cache_dir, cache);
            (value, is_stale, None)
        }
        oxy_airlayer_compat::RefreshKey::Sql(sql) => {
            eval_sql_refresh_key(sql, rollup_hash, cache_dir, ctx, database_name).await
        }
    }
}

/// Evaluate an `Every`-interval refresh key.
///
/// Returns `(None, is_stale)`. `is_stale` is false if the in-memory cache or
/// the manifest confirms the rollup was built within the interval.
pub(super) fn eval_every_refresh_key(
    interval_str: &str,
    rollup_hash: &str,
    cache_dir: &std::path::Path,
    cache: &Arc<RwLock<RefreshKeyCache>>,
) -> (Option<String>, bool) {
    let Ok(interval) = oxy_airlayer_compat::preagg::parse_interval(interval_str) else {
        // Unparsable interval → treat as always stale so operator notices.
        tracing::warn!(interval = %interval_str, rollup_hash, "preagg: unparsable Every interval");
        return (None, true);
    };

    // Layer 1: in-memory cache (survives heartbeats within the same process).
    // Only entries the BUILD side wrote count — a read-path seed says a query
    // looked at the rollup, not that anything rebuilt it, and treating the two
    // alike lets an actively-read rollup postpone its own cadence forever.
    {
        let guard = cache.read().expect("preagg cache lock poisoned");
        if guard
            .get(rollup_hash, interval)
            .is_some_and(|e| !e.seeded_by_read)
        {
            return (None, false);
        }
    }

    // Layer 2: manifest's build_date (survives server restarts).
    // If the rollup was built within the interval, seed the cache and skip rebuild.
    let manifest_build_date =
        agentic_semantic::preagg::load_local_manifest(cache_dir).and_then(|m| {
            m.rollups
                .iter()
                .find(|r| r.rollup_hash == rollup_hash)
                .map(|r| r.build_date.clone())
        });

    if let Some(build_date_str) = manifest_build_date
        && let Ok(built_at) =
            chrono::NaiveDateTime::parse_from_str(&build_date_str, "%Y-%m-%d %H:%M:%S")
    {
        let built_at_utc = built_at.and_utc();
        let age = chrono::Utc::now().signed_duration_since(built_at_utc);
        let chrono_interval = match chrono::Duration::from_std(interval) {
            Ok(d) => d,
            Err(_) => {
                tracing::warn!(
                    interval = %interval_str,
                    rollup_hash,
                    "preagg: configured Every interval overflows chrono::Duration; \
                     treating rollup as always fresh to avoid spurious rebuilds"
                );
                chrono::Duration::milliseconds(i64::MAX)
            }
        };
        if age < chrono_interval {
            let mut guard = cache.write().expect("preagg cache lock poisoned");
            guard.insert(rollup_hash.to_string(), None);
            return (None, false);
        }
    }

    // Layer 3: the node-local ledger's zero-row record (survives restarts).
    // A rollup that rebuilt to nothing has NO manifest entry — the retraction
    // removed it — so layer 2 reads it as never-built and layer 1 only covers
    // this process. Without this, a legitimately empty rollup would rebuild on
    // every cadence tick for as long as it stayed empty, reported as "Not
    // built" the whole time. The attempt is what the interval measures — but
    // capped, because an empty attempt is not the same evidence as a build.
    //
    // Deliberately does NOT seed layer 1 on the way out, unlike layer 2 above.
    // An entry there is read as build recency for the full `interval`, and
    // layer 1 has no way to tell one this branch wrote from one a real build
    // wrote — so seeding here would hand the empty case back the unbounded
    // window `EMPTY_RETRY_CEILING` exists to close. The ledger is the record;
    // re-reading it each tick is one small JSON read, which is what the seed
    // was saving.
    let empty_at = preagg_ledger::RollupLedger::load(cache_dir)
        .empty_record(rollup_hash)
        .and_then(|e| chrono::DateTime::parse_from_rfc3339(&e.at).ok());

    if let Some(at) = empty_at
        && let Ok(chrono_interval) = chrono::Duration::from_std(empty_retry_interval(interval))
        && chrono::Utc::now().signed_duration_since(at.with_timezone(&chrono::Utc))
            < chrono_interval
    {
        return (None, false);
    }

    (None, true)
}

/// Longest an empty build may suppress the next attempt.
///
/// A `refresh_key` says how long the rollup's DATA can be trusted, which is
/// the right question to ask of an artifact that exists. A zero-row build has
/// no artifact: the answer it recorded is "nothing here", and the reasons for
/// it — data that had not landed yet, a window that excluded everything, a
/// source mid-backfill — resolve on their own schedule and not on the one the
/// numbers were given. Measuring the retry against a 6h `every:` therefore
/// bought six hours of live scans under a "Not built" row for a rollup that
/// would have built on the next tick.
const EMPTY_RETRY_CEILING: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// How long a zero-row build suppresses the next one: the refresh interval,
/// but never more than [`EMPTY_RETRY_CEILING`].
///
/// The cap is exact because nothing on the empty path writes to layer 1: the
/// retraction invalidates instead of inserting, and layer 3 above returns
/// without seeding. That is load-bearing, not tidiness. Layer 1 trusts an
/// entry for the full `interval` and cannot tell where it came from, and the
/// only thing that evicts one is the cycle's `sweep(2 × renewal_threshold)` —
/// operator-configurable to an hour (`MAX_RENEWAL_SECS`), so a two-hour
/// window. An empty rollup on a 6h key would have been suppressed for those
/// two hours no matter what this function returned.
///
/// A short interval keeps its own cadence: the point is a floor on how often
/// an empty rollup is retried, not one on how often it is left alone.
fn empty_retry_interval(interval: std::time::Duration) -> std::time::Duration {
    interval.min(EMPTY_RETRY_CEILING)
}

/// Evaluate a SQL-based refresh key by running it against the warehouse.
///
/// Returns `(current_value, is_stale, error_msg)`. On connector/query error,
/// `error_msg` is `Some(...)` and the rollup is treated as fresh (not stale)
/// to avoid rebuild thrashing while the warehouse is unavailable.
pub(super) async fn eval_sql_refresh_key(
    sql: &str,
    rollup_hash: &str,
    cache_dir: &std::path::Path,
    ctx: &OxyProjectContext,
    database_name: &str,
) -> (Option<String>, bool, Option<String>) {
    let connector = match ctx.get_connector(database_name).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("get_connector failed for {database_name}: {e}");
            return (None, false, Some(format!("get_connector failed: {e}")));
        }
    };

    let current = match connector.execute_query(sql, 1).await {
        Ok(result) => result
            .result
            .rows
            .first()
            .and_then(|r| r.0.first())
            .map(|cell| match cell {
                agentic_core::result::CellValue::Text(s) => s.clone(),
                agentic_core::result::CellValue::Number(n) => n.to_string(),
                agentic_core::result::CellValue::Null => String::new(),
            }),
        Err(e) => {
            tracing::warn!("refresh_key SQL evaluation failed: {e}");
            return (None, false, Some(format!("refresh_key SQL failed: {e}")));
        }
    };

    let last_value = agentic_semantic::preagg::load_local_manifest(cache_dir)
        .and_then(|m| {
            m.rollups
                .iter()
                .find(|r| r.rollup_hash == rollup_hash)
                .and_then(|r| r.refresh_key_value.clone())
        })
        // No manifest entry does not mean nothing was ever built: a zero-row
        // rebuild retracts its entry, taking `refresh_key_value` with it. The
        // ledger keeps the probe that empty answer was for, so an unchanged
        // key still says "fresh" instead of rebuilding to nothing every tick.
        .or_else(|| {
            preagg_ledger::RollupLedger::load(cache_dir)
                .empty_record(rollup_hash)
                .and_then(|e| e.refresh_key_value.clone())
        });

    let is_stale = current.as_deref() != last_value.as_deref();
    (current, is_stale, None)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use agentic_semantic::refresh_key_cache::RefreshKeyCache;

    use super::preagg_ledger;

    // ── Freshness after a zero-row rebuild ────────────────────────────────

    /// The regression finding #1 named: the retraction that stops an empty
    /// rollup serving stale rows also erases the manifest fields both
    /// staleness evaluators read. Nothing else in the suite exercises a SECOND
    /// cycle after a zero-row rebuild, which is why a green run and a rollup
    /// rebuilding every 600s were consistent.
    #[tokio::test]
    async fn a_second_cycle_does_not_rebuild_a_rollup_that_just_came_back_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        // What `rebuild_rollup`'s zero-row path leaves behind: no manifest
        // entry at all, and the attempt on record.
        preagg_ledger::record_empty(dir.path(), "empty", 1, "orders", "daily", None).await;

        // A cold process, so only the durable record can answer.
        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        let (_, stale) = super::eval_every_refresh_key("24h", "empty", dir.path(), &cache);
        assert!(
            !stale,
            "the interval still gates the rollup; without the record it rebuilds every tick"
        );
    }

    /// ...and the interval still expires, or the record would be a permanent
    /// mute rather than a cadence.
    #[tokio::test]
    async fn an_empty_rollup_is_stale_again_once_its_interval_elapses() {
        let dir = tempfile::tempdir().expect("tempdir");
        preagg_ledger::record_empty(dir.path(), "empty", 1, "orders", "daily", None).await;

        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        let (_, stale) = super::eval_every_refresh_key("1ms", "empty", dir.path(), &cache);
        assert!(stale, "a 1ms interval has elapsed by now");
    }

    /// Write the ledger's zero-row record at an arbitrary instant. `record_empty`
    /// always stamps `Utc::now()`, and the ceiling is only observable across a
    /// gap longer than a test may sleep for.
    fn record_empty_at(cache_dir: &std::path::Path, hash: &str, minutes_ago: i64) {
        let at = (chrono::Utc::now() - chrono::Duration::minutes(minutes_ago)).to_rfc3339();
        std::fs::create_dir_all(cache_dir).expect("cache dir");
        std::fs::write(
            cache_dir.join("rollup_ledger.json"),
            serde_json::json!({
                "entries": {
                    hash: {
                        "generation": 1,
                        "view": "orders",
                        "rollup": "daily",
                        "empty": { "at": at, "refresh_key_value": null },
                    }
                }
            })
            .to_string(),
        )
        .expect("seed ledger");
    }

    /// The second half of the observation: a rollup whose FIRST build came back
    /// empty stayed unbuilt for the whole `refresh_key` interval — six hours of
    /// live scans behind a row reading "Not built". A zero-row attempt is
    /// evidence that the build ran, not that the data can be trusted for as
    /// long as real rows could, so the suppression is capped.
    #[tokio::test]
    async fn a_long_refresh_interval_does_not_suppress_an_empty_rollup_for_hours() {
        let dir = tempfile::tempdir().expect("tempdir");
        record_empty_at(dir.path(), "empty", 45);

        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        let (_, stale) = super::eval_every_refresh_key("6h", "empty", dir.path(), &cache);
        assert!(
            stale,
            "45 minutes is past the retry ceiling, well inside the 6h data interval"
        );
    }

    /// ...while the ceiling only ever shortens. A rollup on a cadence finer
    /// than the ceiling keeps its own — the cap is a floor on retry frequency,
    /// not a new minimum quiet period.
    #[tokio::test]
    async fn a_short_interval_still_governs_its_own_empty_retry() {
        let dir = tempfile::tempdir().expect("tempdir");
        record_empty_at(dir.path(), "empty", 2);

        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        let (_, stale) = super::eval_every_refresh_key("15m", "empty", dir.path(), &cache);
        assert!(!stale, "two minutes into a 15m cadence, still gated");
    }

    /// The ceiling is only exact while nothing on the empty path writes to
    /// layer 1 — an entry there is trusted for the FULL interval, and the only
    /// thing that evicts one is the cycle's `sweep(2 × renewal_threshold)`,
    /// which an operator may configure out to two hours. The three tests above
    /// all start from an empty `RefreshKeyCache`, so none of them would notice
    /// the seed coming back.
    #[tokio::test]
    async fn evaluating_an_empty_rollup_leaves_the_in_memory_cache_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        record_empty_at(dir.path(), "empty", 2);

        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        let (_, stale) = super::eval_every_refresh_key("6h", "empty", dir.path(), &cache);
        assert!(!stale, "two minutes in, the ledger still gates it");
        assert!(
            cache
                .read()
                .expect("preagg cache lock poisoned")
                .get("empty", std::time::Duration::from_secs(6 * 3600))
                .is_none(),
            "a cached entry would outlive EMPTY_RETRY_CEILING by the sweep window"
        );
    }

    /// ...and the previous cycle's evaluation cannot carry the suppression past
    /// the ceiling either, which is the case a fresh cache hides: run the
    /// evaluator twice over one empty record, the second time from past the
    /// ceiling, and it must come back stale.
    #[tokio::test]
    async fn a_prior_cycles_evaluation_does_not_extend_the_ceiling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));

        // Cycle one, ten minutes after the empty build: inside the ceiling.
        record_empty_at(dir.path(), "empty", 10);
        let (_, stale) = super::eval_every_refresh_key("6h", "empty", dir.path(), &cache);
        assert!(!stale);

        // Cycle two on the SAME cache, now forty minutes after that build.
        record_empty_at(dir.path(), "empty", 40);
        let (_, stale) = super::eval_every_refresh_key("6h", "empty", dir.path(), &cache);
        assert!(
            stale,
            "past the ceiling; whatever cycle one left behind must not answer for cycle two"
        );
    }

    /// The layer-1 cache still earns its place for a REAL build — dropping the
    /// seed on the empty path must not turn every fresh rollup into a manifest
    /// read on every tick.
    #[tokio::test]
    async fn a_built_rollups_cache_entry_is_still_honoured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        cache
            .write()
            .expect("preagg cache lock poisoned")
            .insert("built".to_string(), None);

        let (_, stale) = super::eval_every_refresh_key("6h", "built", dir.path(), &cache);
        assert!(!stale, "no ledger, no manifest — layer 1 answered");
    }

    /// A real build is untouched by the ceiling: its artifact is what the
    /// refresh key is about, and shortening that would rebuild every rollup on
    /// a long cadence twelve times more often than asked.
    #[tokio::test]
    async fn the_ceiling_does_not_touch_a_manifest_build() {
        let dir = tempfile::tempdir().expect("tempdir");
        let built_at = (chrono::Utc::now() - chrono::Duration::minutes(45))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        std::fs::write(
            dir.path().join("manifest.json"),
            serde_json::json!({
                "pulled_at": "2026-08-26T00:00:00Z",
                "source_database": "wh",
                "rollups": [{
                    "view_name": "orders",
                    "rollup_name": "daily",
                    "rollup_hash": "built",
                    "file": "orders__built.parquet",
                    "dimensions": [],
                    "measures": [],
                    "time_dimension": null,
                    "granularity": null,
                    "build_date": built_at,
                }]
            })
            .to_string(),
        )
        .expect("seed manifest");

        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        let (_, stale) = super::eval_every_refresh_key("6h", "built", dir.path(), &cache);
        assert!(
            !stale,
            "45 minutes into a 6h interval, the build still holds"
        );
    }

    // ── Read-path seeds vs. build recency ─────────────────────────────────

    /// One cache, two meanings — and only one of them is a build.
    ///
    /// `check_and_seed_freshness` writes an entry meaning *a reader looked at
    /// this rollup*; layer 1 below reads an entry as *the rollup was built
    /// within the interval*. Conflated, an actively-read rollup keeps a young
    /// entry forever, layer 1 says "not stale", and it never rebuilds — so the
    /// manifest never advances and the scan's `RequireFresh` gate keeps
    /// calling the ageing rollup fresh. The read path must not be able to
    /// postpone the cadence it is waiting on.
    #[tokio::test]
    async fn a_read_path_seed_does_not_stand_in_for_a_build() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));

        // Exactly what a read surface leaves behind when it serves a rollup.
        agentic_semantic::compile::check_and_seed_freshness(&cache, "hot", None, 120);

        let (_, stale) = super::eval_every_refresh_key("24h", "hot", dir.path(), &cache);
        assert!(
            stale,
            "a read seed is not evidence of a build; the rollup is still due"
        );
    }

    /// ...and the worker's own seed still is, or layer 1 would stop being a
    /// cache and every heartbeat would re-read the manifest.
    #[tokio::test]
    async fn a_build_seed_still_satisfies_the_interval() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        cache
            .write()
            .expect("preagg cache lock poisoned")
            .insert("built".to_string(), None);

        let (_, stale) = super::eval_every_refresh_key("24h", "built", dir.path(), &cache);
        assert!(!stale, "the rebuild worker's seed is the build record");
    }

    /// A rollup nothing has ever touched is stale, so a fresh workspace still
    /// builds on its first cycle.
    #[tokio::test]
    async fn a_never_built_rollup_is_stale() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
        let (_, stale) = super::eval_every_refresh_key("24h", "fresh", dir.path(), &cache);
        assert!(stale);
    }
}
