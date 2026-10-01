//! Builds the [`PreaggContext`] the compile path takes, including where to
//! read a rollup this node did not build.
//!
//! There is no download here and no async bridge, which is the point. The
//! rebuild mirrors each rollup Parquet to the compile-boundary blob bucket;
//! a node that never built it points DuckDB at the `s3://` object instead of
//! fetching it, exactly as `connector::duckdb` does for an S3-mirrored
//! local-file warehouse. The compile path stays synchronous and this module
//! stays a config lookup.

use std::sync::Arc;

use agentic_semantic::compile::{BlobConfig, PreaggContext};

/// Where this process's rollup objects live, or `None` when no blob bucket is
/// configured — in which case the local file is the only copy and a node that
/// did not build a rollup falls back to the warehouse.
///
/// Region and endpoint come from the same environment the AWS SDK reads, so a
/// MinIO / LocalStack dev box needs no extra configuration to work here.
pub fn blob_config() -> Option<BlobConfig> {
    Some(BlobConfig {
        bucket: oxy_compile::blob_store::bucket()?,
        region: env_non_empty("AWS_REGION").or_else(|| env_non_empty("AWS_DEFAULT_REGION")),
        endpoint_url: env_non_empty("AWS_ENDPOINT_URL"),
    })
}

fn env_non_empty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// What a caller wants done with a rollup the freshness check calls stale.
///
/// Named rather than a bare `bool`, because "this surface forgot to say" is
/// the exact failure this module keeps having: a `bool` reads as noise in an
/// argument list, and the wrong answer here is silent — a number that is
/// merely late on a chart, or a false anomaly in the inbox. The default is
/// deliberately the read-surface posture, so a call site that says nothing
/// gets the answer that is only ever *late*, never wrong in kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RollupFreshness {
    /// Serve it anyway. Right for every read surface: the rollup is a display
    /// of the data, the **Pre-aggregated** badge says so, and the freshness
    /// check has already seeded the rebuild.
    #[default]
    ServeStale,
    /// Fall through to the warehouse. Right where the number becomes an
    /// assertion *about* the data — the anomaly scan and its explain, which
    /// persist to the Insights Inbox and can page Slack.
    ///
    /// A **cold-cache guard, not a lag detector.** `check_and_seed_freshness`
    /// answers "was this rollup's refresh key checked within the renewal
    /// threshold, and does it still match the manifest?" — and since a miss
    /// seeds the cache from that same manifest, it cannot tell how far behind
    /// the rollup's `preagg_cycle` is. So this declines on a cold or expired
    /// entry and on a manifest that moved under a live one; a rollup that is
    /// uniformly stale still gets served. Measuring real lag means comparing
    /// the manifest's `build_date` against the rollup's refresh interval,
    /// which the compile-path context does not carry.
    RequireFresh,
}

/// Assemble the rollup short-circuit for one request.
///
/// `None` when the node has no Layer-1 cache (and therefore no rebuild
/// worker): without one there is no guarantee a rollup is current, so the
/// query compiles to warehouse SQL — the same posture the CLI and the builder
/// validator take.
///
/// `renewal_threshold_secs` comes from the workspace's own `preagg:` block,
/// resolved per request (see `PreaggCacheCtx::renewal_threshold_secs_or`), not
/// from a process-wide default.
pub fn preagg_context(
    workspace_id: uuid::Uuid,
    cache: Option<Arc<std::sync::RwLock<agentic_semantic::refresh_key_cache::RefreshKeyCache>>>,
    renewal_threshold_secs: Option<u64>,
    freshness: RollupFreshness,
) -> Option<PreaggContext> {
    // Never under a staging pin. Rollups are built from the PROMOTED model and
    // matched by `(view, rollup, rollup_hash)`, so a branch that changes a
    // view's SQL but not its rollup declaration would be answered from a rollup
    // of the old model. A pinned request compiles to warehouse SQL: slower,
    // always right. (`custom_apps_staging_pin::with_staging_pin`.)
    if crate::server::api::custom_apps_staging_pin::current_staging_pin().is_some() {
        return None;
    }
    Some(PreaggContext {
        workspace_id,
        cache: cache?,
        renewal_threshold_secs: renewal_threshold_secs
            .unwrap_or(oxy::config::preagg_check::DEFAULT_RENEWAL_SECS),
        blob: blob_config(),
        require_fresh: freshness == RollupFreshness::RequireFresh,
    })
}

#[cfg(test)]
mod tests {
    use agentic_semantic::compile::{BlobConfig, PreaggContext, PreaggSource, try_resolve_preagg};

    /// Removes the manifest this test writes even when an assertion panics —
    /// the cache dir is the real process-wide state dir, so a cleanup on the
    /// happy path only would leave debris in a developer's `~/.local/share/oxy`
    /// on exactly the runs they're debugging.
    struct CacheDirGuard(std::path::PathBuf);
    impl Drop for CacheDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A manifest listing one rollup, with no Parquet beside it — the state of
    /// every node that did not run the rebuild.
    fn write_one_rollup_manifest(cache_dir: &std::path::Path) {
        std::fs::write(
            cache_dir.join("manifest.json"),
            serde_json::json!({
                "pulled_at": "2026-08-25T00:00:00Z",
                "source_database": "local",
                "rollups": [{
                    "view_name": "orders",
                    "rollup_name": "by_month",
                    "rollup_hash": "deadbeef",
                    "file": "orders__deadbeef.parquet",
                    "dimensions": [],
                    "measures": [{"name": "total_orders", "type": "count"}],
                    "time_dimension": null,
                    "granularity": null,
                    "build_date": "2026-08-25 00:00:00"
                }]
            })
            .to_string(),
        )
        .expect("manifest writes");
    }

    fn covering_request() -> oxy_airlayer_compat::engine::query::QueryRequest {
        serde_json::from_value(serde_json::json!({
            "measures": ["orders.total_orders"],
            "dimensions": []
        }))
        .expect("request parses")
    }

    /// A blob-backed context over a **cold** cache: nothing has been seeded, so
    /// `check_and_seed_freshness` reports not-fresh on the first look. That is
    /// the state a node is in right after a restart, and the one where the two
    /// freshness postures diverge.
    fn blob_preagg(workspace_id: uuid::Uuid, require_fresh: bool) -> PreaggContext {
        PreaggContext {
            workspace_id,
            cache: std::sync::Arc::new(std::sync::RwLock::new(
                agentic_semantic::refresh_key_cache::RefreshKeyCache::new(),
            )),
            renewal_threshold_secs: 0,
            require_fresh,
            blob: Some(BlobConfig {
                bucket: "oxy-blobs".to_string(),
                region: None,
                endpoint_url: None,
            }),
        }
    }

    /// The anomaly scan and its explain persist what they compute — a stale
    /// rollup's missing tail buckets read as a drop, land in the Insights Inbox
    /// and can page Slack. `RequireFresh` must decline so the caller takes the
    /// warehouse SQL the compiler hands back alongside.
    #[test]
    fn require_fresh_declines_a_rollup_the_freshness_check_calls_stale() {
        let workspace_id = uuid::Uuid::from_u128(1235);
        let cache_dir = oxy::state_dir::get_airlayer_cache_dir(workspace_id);
        std::fs::create_dir_all(&cache_dir).expect("cache dir");
        let _cleanup = CacheDirGuard(cache_dir.clone());
        write_one_rollup_manifest(&cache_dir);
        let request = covering_request();

        assert!(
            try_resolve_preagg(
                &blob_preagg(workspace_id, false),
                &request,
                "SELECT 'unused'",
                "local",
                None,
            )
            .is_some(),
            "a read surface serves the stale rollup and lets the rebuild catch up"
        );
        assert!(
            try_resolve_preagg(
                &blob_preagg(workspace_id, true),
                &request,
                "SELECT 'unused'",
                "local",
                None,
            )
            .is_none(),
            "require_fresh must fall through to the warehouse on the same rollup"
        );
    }

    /// Same context, once the cache agrees the rollup is current: `RequireFresh`
    /// is a freshness gate, not an opt-out of pre-aggregation.
    #[test]
    fn require_fresh_still_serves_a_rollup_the_check_calls_fresh() {
        let workspace_id = uuid::Uuid::from_u128(1236);
        let cache_dir = oxy::state_dir::get_airlayer_cache_dir(workspace_id);
        std::fs::create_dir_all(&cache_dir).expect("cache dir");
        let _cleanup = CacheDirGuard(cache_dir.clone());
        write_one_rollup_manifest(&cache_dir);
        let request = covering_request();

        // A threshold wide enough that the entry seeded by the first look is
        // still within it on the second — which is what "fresh" means here.
        let mut preagg = blob_preagg(workspace_id, true);
        preagg.renewal_threshold_secs = 3600;
        assert!(
            try_resolve_preagg(&preagg, &request, "SELECT 'unused'", "local", None).is_none(),
            "first look seeds the cache and reports not-fresh"
        );
        assert!(
            try_resolve_preagg(&preagg, &request, "SELECT 'unused'", "local", None).is_some(),
            "second look hits the seeded entry and serves the rollup"
        );
    }

    /// The read side and the write side address the same object.
    ///
    /// `agentic-semantic` builds the `s3://` URI it reads from, and
    /// `oxy-compile::preagg_blob` builds the key it mirrors to. Neither crate
    /// can see the other — that separation is deliberate — so this is the one
    /// place both are in scope, and the only thing holding them together. If
    /// it fails, every cross-node rollup read 404s while both sides look
    /// individually correct.
    #[test]
    fn the_uri_the_read_path_builds_matches_the_key_the_mirror_writes() {
        let workspace_id = uuid::Uuid::from_u128(1234);
        let cache_dir = oxy::state_dir::get_airlayer_cache_dir(workspace_id);
        std::fs::create_dir_all(&cache_dir).expect("cache dir");
        let _cleanup = CacheDirGuard(cache_dir.clone());
        write_one_rollup_manifest(&cache_dir);
        let request = covering_request();
        let preagg = blob_preagg(workspace_id, false);
        let compiled = try_resolve_preagg(&preagg, &request, "SELECT 'unused'", "local", None)
            .expect("the blob tier resolves");
        let agentic_semantic::compile::CompiledQuery::Preaggregation { source, .. } = compiled
        else {
            panic!("expected a rollup resolution");
        };
        let PreaggSource::Blob { uri, .. } = &source else {
            panic!("expected a blob source, got {source:?}");
        };

        let mirror_key = oxy_compile::preagg_blob::parquet_key(
            &oxy::state_dir::airlayer_cache_key(workspace_id),
            "orders__deadbeef.parquet",
        );
        assert_eq!(uri, &format!("s3://oxy-blobs/{mirror_key}"));
    }
}
