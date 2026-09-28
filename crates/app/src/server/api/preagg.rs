//! Pre-aggregation surface for the IDE's Semantic Model tab.
//!
//! - `GET  /semantic/preagg-status`  — every rollup the layer DECLARES, joined
//!   with what this node's airlayer cache holds for each.
//! - `POST /semantic/preagg-rebuild` — rebuild them: all, or one.
//!
//! Both are node-local surfaces (the cache is a directory in the state dir),
//! so both are pinned `IdeOnly` in `role_manifest.rs`.

use std::collections::HashMap;

use axum::{
    Json,
    extract::{self, Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use oxy_app_core::app_state::AppState;

use crate::server::api::middlewares::workspace_context::{
    SemanticLayerCacheCtx, WorkspaceManagerReadOnly, WorkspacePath,
};
use crate::server::api::semantic::{ErrorResponse, resolve_query_scan_source, semantic_err};

// ── Preagg status ─────────────────────────────────────────────────────────────
//
// The list of rollups comes from the SEMANTIC MODEL CONFIG (`pre_aggregations:`
// in each `.view.yml`), never from the cache. Reading it off the manifest — the
// shape this started as — made the tab a view of what happened to be built,
// which is exactly backwards: a workspace that declares twelve rollups and has
// built none of them showed nothing at all, and the one fact an operator opens
// this surface to learn ("what is meant to be cached, and is it?") was the one
// it couldn't state. Config is the source of truth; the manifest supplies one
// column of it.

/// A rollup's measure, as the manifest stores it and as the tab reads it.
///
/// The rename is DESERIALIZE-ONLY on purpose. airlayer's `manifest.json`
/// writes the aggregation under `"type"`, so that key has to be accepted on
/// the way in; but the frontend's `PreaggMeasure` reads `measure_type`, so a
/// bare `rename` — which serde applies in both directions — shipped the field
/// under a name no reader looks for and every measure chip rendered without
/// its `(count)` suffix.
#[derive(serde::Deserialize, Serialize, Clone)]
pub struct ManifestMeasure {
    pub name: String,
    #[serde(rename(deserialize = "type"))]
    pub measure_type: String,
}

#[derive(serde::Deserialize)]
struct ManifestRollupEntry {
    view_name: String,
    rollup_name: String,
    /// The hash of the definition this artifact was built FROM — already on
    /// disk (airlayer's `ManifestEntry` writes it), just previously dropped
    /// on the floor here.
    ///
    /// Defaulted rather than required so a row missing it degrades to "not
    /// built" on its own — without the default, one such row makes the whole
    /// manifest unparsable and blanks the cache columns for every rollup.
    #[serde(default)]
    rollup_hash: String,
    file: String,
    build_date: Option<String>,
    refresh_key_checked_at: Option<String>,
}

#[derive(serde::Deserialize)]
struct LocalManifestJson {
    rollups: Vec<ManifestRollupEntry>,
}

#[derive(Serialize, Clone)]
pub struct PreaggRollupStatus {
    pub view_name: String,
    pub rollup_name: String,
    /// Whether the rollup has been BUILT — i.e. the manifest lists it. This
    /// is the fleet-wide fact and the one a reader cares about, because a
    /// built rollup serves queries from every node: the builder reads its
    /// local Parquet, everyone else reads the same object out of the blob
    /// store. False is a status, not an error — the query still runs, against
    /// the warehouse.
    pub is_built: bool,
    /// Whether THIS node holds the Parquet on local disk. Purely a locality
    /// fact, and reported separately because it is not the same question:
    /// conflating the two made a rollup another node built read as "Not
    /// cached" beside a real Built timestamp — two columns from one manifest
    /// disagreeing.
    pub has_parquet: bool,
    pub dimensions: Vec<String>,
    pub measures: Vec<ManifestMeasure>,
    pub time_dimension: Option<String>,
    pub granularity: Option<String>,
    /// `every:`/`sql:` refresh cadence as written in the YAML, view-level key
    /// included — what the rollup is *supposed* to do, which a never-built
    /// rollup has and the manifest cannot supply.
    pub refresh_key: Option<String>,
    pub build_date: Option<String>,
    pub refresh_key_checked_at: Option<String>,
    /// When this node's last rebuild of the rollup produced ZERO rows, so its
    /// entry and Parquet were retracted rather than left serving the previous
    /// build's numbers. RFC3339 UTC.
    ///
    /// Reported because "empty" and "never built" are otherwise the same row —
    /// both are `is_built: false` with no build time — and they are different
    /// facts: one means the cycle ran and the rollup has nothing in it right
    /// now, the other that nothing has been attempted. It is also the only
    /// thing that moves after a zero-row rebuild, so a client waiting on
    /// `build_date` to change would wait forever.
    pub empty_since: Option<String>,
}

#[derive(Serialize)]
pub struct PreaggStatusResponse {
    /// Every rollup the semantic model declares, cached or not. Empty means the
    /// workspace declares none — it is a statement about config, so a caller may
    /// render it as one.
    pub rollups: Vec<PreaggRollupStatus>,
    /// Whether a rollup built on ANOTHER node can be read from here — i.e. a
    /// blob bucket is configured. Without one the local file is the only copy,
    /// so `is_built && !has_parquet` means the warehouse answers, and a UI
    /// that said otherwise would be promising a fast path this deployment
    /// does not have.
    pub blob_reads_available: bool,
}

/// What the cache knows about one declared rollup.
pub(crate) struct CacheFacts {
    /// The manifest lists it, so some node has built it.
    is_built: bool,
    /// ...and this node also holds the file.
    has_parquet: bool,
    build_date: Option<String>,
    refresh_key_checked_at: Option<String>,
}

/// Normalize a manifest timestamp to RFC3339 UTC.
///
/// `refresh_key_checked_at` is already RFC3339 (`preagg_rebuild` writes
/// `Utc::now().to_rfc3339()`), but `build_date` comes from airlayer's
/// `manifest.json` in the naive shape `preagg_executor` parses it back out
/// with — `"%Y-%m-%d %H:%M:%S"`, no zone — and older manifests carry a bare
/// `"%Y-%m-%d"`. Shipping either verbatim hands the browser a string
/// `new Date()` reads as *local* time (or, for the bare date, as UTC midnight
/// that renders as the previous evening west of UTC), so a build time comes
/// out shifted or a day early under a column labelled "Built". This is the
/// same ISO-8601-UTC-on-the-wire rule the observability serving path learned.
///
/// An unrecognized shape returns `None` rather than a guess: the frontend
/// renders an em dash, which is honest, where `Invalid Date` is not.
fn normalize_manifest_timestamp(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    // Already zoned (what `refresh_key_checked_at` always is) — keep it, but
    // re-serialize so every timestamp on the wire has one shape.
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&chrono::Utc).to_rfc3339());
    }
    // Naive datetime — airlayer writes UTC, so read it as UTC.
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S") {
        return Some(naive.and_utc().to_rfc3339());
    }
    // Date only: midnight UTC. The frontend renders it through the same
    // `formatDate` as any other timestamp, hour and minute included, so a
    // bare-date build reads as "May 11, 12:00 AM" — imprecise, but no longer
    // the previous evening, which is what shipping the bare string did.
    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc().to_rfc3339());
    }
    None
}

/// Read the local cache manifest into per-rollup facts, keyed
/// `(view, rollup, hash)`.
///
/// A missing or unparsable manifest is an empty map, not an error: nothing has
/// been built here, and every declared rollup then reports "not cached" — which
/// is the truth, and is now visible instead of being an empty screen.
///
/// The hash is in the key, not carried alongside a `(view, rollup)` key,
/// because one identity can have two rows: publish reaps the superseded one,
/// but a manifest from an older builder or mirrored from S3 may still hold it,
/// and a rollup with no `refresh_key` is never republished to dedup it. A
/// name-keyed map collapsed those last-wins, so a stale row sitting after the
/// live one hid an artifact that was actively serving. `check_coverage` scans
/// every row and serves the declared hash; [`declared_rollups`] looks up that
/// same hash, so the two pick the same row whatever order they are in.
///
/// This used to stop at the name join and say so, on the reasoning that the
/// query path did not require hash equality either — `check_coverage` asks
/// whether the STORED rollup covers the request, so an artifact built before
/// an edit still legitimately answered. That stopped being true: every read
/// path now passes airlayer's `live` set, which declines a manifest row whose
/// hash the schema no longer declares, because a rename-free edit (`expr:`,
/// `type:`, a measure `filters:` entry) moves the hash while leaving every
/// name the coverage check inspects identical. A row nothing may serve must
/// not read as Cached with the old build time.
///
/// Blocking (one `read_to_string`, one `is_file` per entry) — call it inside
/// `spawn_blocking`.
pub(crate) fn read_cache_facts(
    cache_dir: &std::path::Path,
) -> HashMap<(String, String, String), CacheFacts> {
    let manifest_path = cache_dir.join("manifest.json");
    std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|s| serde_json::from_str::<LocalManifestJson>(&s).ok())
        .map(|manifest| {
            manifest
                .rollups
                .into_iter()
                .map(|entry| {
                    let facts = CacheFacts {
                        // Listed in the manifest at all — the manifest is
                        // synced fleet-wide, so this is "somebody built it".
                        is_built: true,
                        has_parquet: cache_dir.join(&entry.file).is_file(),
                        build_date: entry
                            .build_date
                            .as_deref()
                            .and_then(normalize_manifest_timestamp),
                        refresh_key_checked_at: entry
                            .refresh_key_checked_at
                            .as_deref()
                            .and_then(normalize_manifest_timestamp),
                    };
                    (
                        (entry.view_name, entry.rollup_name, entry.rollup_hash),
                        facts,
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// When this node's last rebuild of each rollup produced zero rows, by rollup
/// HASH — see `preagg_ledger::empty_rollups`.
///
/// Separate from [`read_cache_facts`] because it answers a different question
/// about a different key. The manifest is fleet-wide and says what SOMEBODY
/// built; this is node-local and says what this node found empty, and the two
/// can disagree: node A retracts a rollup as empty, node B rebuilds it with
/// rows, and A's next status read syncs B's manifest while A's own ledger still
/// holds the empty record. `is_built` wins there — the join below only consults
/// this for a rollup the manifest does not list — so a row can never ship
/// `is_built: true` alongside an `empty_since`.
///
/// Blocking (one `read_to_string`) — call it inside `spawn_blocking`.
pub(crate) fn read_empty_since(cache_dir: &std::path::Path) -> HashMap<String, String> {
    crate::server::preagg_ledger::empty_rollups(cache_dir)
}

/// Fetch the shared manifest through from S3 (mirrored there by whichever node
/// last rebuilt this workspace's pre-aggregations — see
/// `preagg_rebuild::mirror_manifest_to_s3`) and write it to disk so
/// `read_cache_facts` picks it up normally.
///
/// This used to return early whenever a local manifest existed, which made the
/// sync once-only: a node that ever wrote a manifest never consulted S3 again,
/// so every rebuild that landed on another node stayed invisible here forever.
/// The condition is now recency, not existence — the remote copy wins only
/// when its `pulled_at` is strictly newer, which is exactly the case where
/// another node published something this one hasn't seen. A local manifest
/// that is newer (this node just built) is never clobbered.
///
/// Best-effort in every direction: no bucket configured, nothing mirrored yet,
/// an unparseable remote copy, or a write failure all leave the local manifest
/// exactly as it already was — `read_cache_facts` treats a missing one as
/// "nothing cached here", which stays the correct fallback answer.
async fn sync_manifest_from_s3(cache_dir: &std::path::Path) {
    let manifest_path = cache_dir.join("manifest.json");
    let Some(cache_key) = cache_dir.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let Some(bytes) = oxy_compile::preagg_blob::fetch_manifest(cache_key).await else {
        return;
    };
    if !remote_manifest_is_newer(&manifest_path, &bytes).await {
        return;
    }
    if let Err(e) = tokio::fs::create_dir_all(cache_dir).await {
        tracing::warn!(error = %e, "preagg: could not create cache dir for S3 read-through");
        return;
    }
    if let Err(e) = tokio::fs::write(&manifest_path, bytes).await {
        tracing::warn!(error = %e, "preagg: could not write S3-fetched manifest to local disk");
    }
}

/// Whether the S3 copy carries a strictly newer `pulled_at` than the local
/// one. An absent or unreadable local manifest is "yes" (anything beats
/// nothing); an unparseable *remote* is "no", so a corrupt object can never
/// overwrite a good local file.
async fn remote_manifest_is_newer(manifest_path: &std::path::Path, remote: &[u8]) -> bool {
    let Some(remote_at) = manifest_pulled_at(remote) else {
        return false;
    };
    match tokio::fs::read(manifest_path).await {
        Ok(local) => match manifest_pulled_at(&local) {
            Some(local_at) => remote_at > local_at,
            // Local exists but has no usable `pulled_at` — prefer the copy we
            // can actually reason about.
            None => true,
        },
        Err(_) => true,
    }
}

fn manifest_pulled_at(bytes: &[u8]) -> Option<chrono::DateTime<chrono::Utc>> {
    #[derive(serde::Deserialize)]
    struct PulledAt {
        pulled_at: String,
    }
    let parsed: PulledAt = serde_json::from_slice(bytes).ok()?;
    chrono::DateTime::parse_from_rfc3339(&parsed.pulled_at)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

/// Render a rollup's refresh key the way the YAML writes it.
fn refresh_key_label(key: &oxy_airlayer_compat::schema::models::RefreshKey) -> Option<String> {
    use oxy_airlayer_compat::schema::models::RefreshKey;
    Some(match key {
        RefreshKey::Every(interval) => format!("every {interval}"),
        // The SQL itself is a probe, not a cadence, and can be long — say that
        // it's SQL-keyed and leave the expression to the YAML.
        RefreshKey::Sql(_) => "sql".to_string(),
    })
}

/// Join every rollup the layer declares with what the local cache holds for it.
///
/// Order is (view, rollup) so the table groups by view without a second pass,
/// and so two calls with the same config produce the same page.
///
/// On a feature branch that edits a rollup, that row reads "not built" and the
/// panel's Rebuild cannot flip it: `layer` comes from the branch checkout, while
/// the cache — and every rebuild cycle — is the default branch's, so what gets
/// built carries main's hash. Expected, not stuck: the analytics agent on that
/// branch declines the old artifact too, so status and read path agree, and
/// the row resolves once the edit merges and main rebuilds.
pub(crate) fn declared_rollups(
    layer: &oxy_airlayer_compat::SemanticLayer,
    cache: &HashMap<(String, String, String), CacheFacts>,
    // Zero-row instants by rollup HASH — see `read_empty_since`. Keyed by hash
    // rather than name on purpose: a rollup whose `dimensions:` were edited is
    // a different rollup, so its predecessor's empty answer must not describe it.
    empty_since: &HashMap<String, String>,
) -> Vec<PreaggRollupStatus> {
    let mut rollups: Vec<PreaggRollupStatus> = layer
        .views
        .iter()
        .flat_map(|view| {
            let declared = view.pre_aggregations.as_deref().unwrap_or_default();
            // The declaration carries no hash, so resolve it once per view and
            // look each rollup up by name. `resolve_rollups` is what the builder
            // and the sweep key on, so this cannot drift from the hash the
            // ledger actually recorded.
            //
            // Skipped for a view that declares nothing — most of them, in a
            // layer of any size, and this runs on every status poll.
            let hashes: HashMap<String, String> = if declared.is_empty() {
                HashMap::new()
            } else {
                oxy_airlayer_compat::preagg::resolve_rollups(view)
                    .into_iter()
                    .map(|r| (r.name, r.hash))
                    .collect()
            };
            declared.iter().map(move |rollup| {
                // A rollup names its measures; the types live on the view's own
                // measure definitions. Join them so a never-built rollup reads
                // the same as a built one — the manifest is not the only place
                // a measure's aggregation is knowable.
                let measures = rollup
                    .measures
                    .iter()
                    .map(|name| ManifestMeasure {
                        name: name.clone(),
                        measure_type: view
                            .measures
                            .as_deref()
                            .unwrap_or_default()
                            .iter()
                            .find(|m| &m.name == name)
                            .map(|m| m.measure_type.sql_function().to_lowercase())
                            .unwrap_or_default(),
                    })
                    .collect();
                // The hash has to agree, not just the name. A manifest row
                // under this identity's OLD hash describes the definition
                // before the last edit; the read path declines it (see
                // `read_cache_facts`), so reporting it as Built — with that
                // build's timestamp, under the current spec's dimensions and
                // measures — would describe an artifact nothing serves. The
                // row reads "not built" until the rebuild lands, which is the
                // state it is actually in.
                let facts = hashes.get(&rollup.name).and_then(|hash| {
                    cache.get(&(view.name.clone(), rollup.name.clone(), hash.clone()))
                });
                PreaggRollupStatus {
                    view_name: view.name.clone(),
                    rollup_name: rollup.name.clone(),
                    is_built: facts.is_some_and(|f| f.is_built),
                    has_parquet: facts.is_some_and(|f| f.has_parquet),
                    dimensions: rollup.dimensions.clone(),
                    measures,
                    time_dimension: rollup.time_dimension.clone(),
                    granularity: rollup.granularity.clone(),
                    // Per-rollup key wins; the view-level key is the fallback,
                    // matching how `preagg_executor::rollup_refresh_key` picks.
                    refresh_key: rollup
                        .refresh_key
                        .as_ref()
                        .or(view.refresh_key.as_ref())
                        .and_then(refresh_key_label),
                    build_date: facts.and_then(|f| f.build_date.clone()),
                    refresh_key_checked_at: facts.and_then(|f| f.refresh_key_checked_at.clone()),
                    // Only for a rollup the manifest does not list. A built
                    // rollup's rows are the fleet-wide fact and they win: this
                    // node's ledger can still hold an empty record for a hash
                    // another node has since rebuilt, and a row claiming both
                    // would contradict itself on the wire.
                    empty_since: if facts.is_some_and(|f| f.is_built) {
                        None
                    } else {
                        hashes
                            .get(&rollup.name)
                            .and_then(|hash| empty_since.get(hash))
                            .map(|at| at.as_str())
                            .and_then(normalize_manifest_timestamp)
                    },
                }
            })
        })
        .collect();
    rollups.sort_by(|a, b| {
        a.view_name
            .cmp(&b.view_name)
            .then_with(|| a.rollup_name.cmp(&b.rollup_name))
    });
    rollups
}

/// `GET /{workspace_id}/semantic/preagg-status`
///
/// Every rollup declared in the semantic model, joined with what the local
/// airlayer cache holds for it. Timestamps go out RFC3339 UTC (see
/// `normalize_manifest_timestamp`).
///
/// The list resolves through the compile boundary like the rest of this module,
/// so a workspace with no compiled layer and no working copy gets a *retryable*
/// error rather than a confident empty list. The cache columns, though, are
/// node-local — a replica that never built the cache would report every rollup
/// uncached — so the route stays pinned `IdeOnly` in `role_manifest.rs`.
pub async fn get_preagg_status(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    layer_cache: SemanticLayerCacheCtx,
    Path(WorkspacePath {
        workspace_id: _workspace_id,
    }): Path<WorkspacePath>,
) -> Result<extract::Json<PreaggStatusResponse>, (StatusCode, extract::Json<ErrorResponse>)> {
    let scan = resolve_query_scan_source(&workspace_manager)
        .await
        .map_err(|e| semantic_err(StatusCode::SERVICE_UNAVAILABLE, e.message()))?;
    let layer = layer_cache
        .get_or_load(
            scan.source_revision(&workspace_manager),
            scan.scan_path.clone(),
        )
        .await
        .map_err(|e| {
            semantic_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load semantic model: {e}"),
            )
        })?;

    // Keyed on the workspace ID, NOT on `workspace_path()`. A request carrying
    // `?branch=` — which the tab always sends — resolves that path to a
    // `.worktrees/<branch>` checkout, while the rebuild cycle always runs
    // against the default-branch root. A path-derived key therefore sent
    // reader and writer to two different directories, and on a feature branch,
    // the IDE's normal state, every declared rollup read "Not cached" no
    // matter what had been built.
    let cache_dir = oxy::state_dir::get_airlayer_cache_dir(workspace_manager.workspace_id);
    // Cross-node read-through: this node may never have built anything for
    // this workspace, while another node in the fleet has. Best-effort — a
    // failed or no-op fetch (no bucket configured, nothing mirrored yet)
    // leaves `read_cache_facts` reading a locally-absent manifest, which
    // already degrades correctly to "nothing cached".
    sync_manifest_from_s3(&cache_dir).await;
    // Blocking fs, so off the Tokio worker. A join failure degrades to "nothing
    // cached" rather than failing the request: the declared list is the point,
    // and it is already in hand.
    let (cache, empty_since) = tokio::task::spawn_blocking(move || {
        (read_cache_facts(&cache_dir), read_empty_since(&cache_dir))
    })
    .await
    .unwrap_or_default();

    Ok(extract::Json(PreaggStatusResponse {
        rollups: declared_rollups(&layer, &cache, &empty_since),
        blob_reads_available: crate::server::preagg_context::blob_config().is_some(),
    }))
}

#[cfg(test)]
mod preagg_tests {
    use super::*;

    fn layer_from_yaml(views: &[&str]) -> oxy_airlayer_compat::SemanticLayer {
        oxy_airlayer_compat::SemanticLayer::new(
            views
                .iter()
                .map(|y| serde_yaml::from_str::<oxy_airlayer_compat::View>(y).unwrap())
                .collect(),
            None,
        )
    }

    /// One view declaring two rollups, only one of which has ever been built.
    const ORDERS_VIEW: &str = r#"
name: orders
datasource: local
table: orders.csv
refresh_key:
  every: 1h
measures:
  - name: total_orders
    type: count
  - name: total_order_value
    type: sum
    sql: amount
pre_aggregations:
  - name: orders_by_month
    dimensions: [order_status]
    measures: [total_orders, total_order_value]
    time_dimension: order_date
    granularity: month
  - name: orders_summary
    measures: [total_orders]
    refresh_key:
      every: 15m
"#;

    fn write_manifest(cache_dir: &std::path::Path, manifest: serde_json::Value) {
        std::fs::create_dir_all(cache_dir).unwrap();
        std::fs::write(cache_dir.join("manifest.json"), manifest.to_string()).unwrap();
    }

    #[test]
    fn every_declared_rollup_is_listed_even_with_no_cache_at_all() {
        // The whole point of the surface: a workspace that has built nothing
        // still shows what it means to build.
        let layer = layer_from_yaml(&[ORDERS_VIEW]);
        let rollups = declared_rollups(&layer, &HashMap::new(), &HashMap::new());

        assert_eq!(rollups.len(), 2);
        assert!(rollups.iter().all(|r| !r.is_built && !r.has_parquet));
        assert_eq!(rollups[0].rollup_name, "orders_by_month");
        assert_eq!(rollups[1].rollup_name, "orders_summary");
    }

    #[test]
    fn declared_rollups_carry_their_config_not_the_manifest_s_copy_of_it() {
        let layer = layer_from_yaml(&[ORDERS_VIEW]);
        let rollups = declared_rollups(&layer, &HashMap::new(), &HashMap::new());
        let by_month = &rollups[0];

        assert_eq!(by_month.dimensions, ["order_status"]);
        assert_eq!(by_month.time_dimension.as_deref(), Some("order_date"));
        assert_eq!(by_month.granularity.as_deref(), Some("month"));
        // Measure types come from the view's own definitions, so they're known
        // for a rollup that has never been built.
        assert_eq!(by_month.measures[0].name, "total_orders");
        assert_eq!(by_month.measures[0].measure_type, "count");
        assert_eq!(by_month.measures[1].measure_type, "sum");
        // Per-rollup key wins; the view-level key is the fallback.
        assert_eq!(by_month.refresh_key.as_deref(), Some("every 1h"));
        assert_eq!(rollups[1].refresh_key.as_deref(), Some("every 15m"));
    }

    #[test]
    fn cache_facts_are_joined_onto_the_declared_rollup() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join(".airlayer").join("cache");
        let layer = layer_from_yaml(&[ORDERS_VIEW]);
        write_manifest(
            &cache_dir,
            serde_json::json!({
                "rollups": [{
                    "view_name": "orders",
                    "rollup_name": "orders_by_month",
                    "rollup_hash": declared_hash(&layer, "orders_by_month"),
                    "file": "orders__aabbccdd.parquet",
                    "build_date": "2026-05-11 14:03:22"
                }]
            }),
        );
        std::fs::write(cache_dir.join("orders__aabbccdd.parquet"), b"").unwrap();

        let rollups = declared_rollups(
            &layer,
            &read_cache_facts(&cache_dir),
            &read_empty_since(&cache_dir),
        );

        assert!(rollups[0].is_built);
        assert!(rollups[0].has_parquet);
        assert_eq!(
            rollups[0].build_date.as_deref(),
            Some("2026-05-11T14:03:22+00:00")
        );
        // Declared but never built — still listed, still described, uncached.
        assert!(!rollups[1].is_built);
        assert!(!rollups[1].has_parquet);
        assert_eq!(rollups[1].build_date, None);
    }

    /// A manifest written by an older builder, or mirrored from S3, can list one
    /// identity under two hashes — publish reaps the superseded row, but a rollup
    /// with no `refresh_key` is never republished. The read path scans every
    /// row and serves the one the schema declares; the status has to pick the
    /// same one, whatever order the rows sit in.
    #[test]
    fn a_superseded_duplicate_row_does_not_hide_the_one_being_served() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join(".airlayer").join("cache");
        let layer = layer_from_yaml(&[ORDERS_VIEW]);
        write_manifest(
            &cache_dir,
            serde_json::json!({
                "rollups": [
                    {
                        "view_name": "orders",
                        "rollup_name": "orders_by_month",
                        "rollup_hash": declared_hash(&layer, "orders_by_month"),
                        "file": "orders__live.parquet",
                        "build_date": "2026-05-11 14:03:22"
                    },
                    {
                        "view_name": "orders",
                        "rollup_name": "orders_by_month",
                        "rollup_hash": "a-hash-no-longer-declared",
                        "file": "orders__stale.parquet",
                        "build_date": "2026-04-01 09:00:00"
                    }
                ]
            }),
        );
        std::fs::write(cache_dir.join("orders__live.parquet"), b"").unwrap();

        let rollups = declared_rollups(
            &layer,
            &read_cache_facts(&cache_dir),
            &read_empty_since(&cache_dir),
        );

        assert!(rollups[0].is_built, "the declared hash's row is serving");
        assert!(rollups[0].has_parquet);
        assert_eq!(
            rollups[0].build_date.as_deref(),
            Some("2026-05-11T14:03:22+00:00")
        );
    }

    /// A rollup this node rebuilt to zero rows has NO manifest entry — the
    /// retraction is what removed it — so it is added from the ledger rather
    /// than joined. Reported as a plain never-built row it reads as a rebuild
    /// that never ran, against a run that reported doing exactly the right
    /// thing.
    #[tokio::test]
    async fn a_rollup_retracted_as_empty_reports_when_it_emptied() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join(".airlayer").join("cache");
        let layer = layer_from_yaml(&[ORDERS_VIEW]);
        write_manifest(&cache_dir, serde_json::json!({ "rollups": [] }));
        crate::server::preagg_ledger::record_empty(
            &cache_dir,
            &declared_hash(&layer, "orders_by_month"),
            1,
            "orders",
            "orders_by_month",
            None,
        )
        .await;

        let rollups = declared_rollups(
            &layer,
            &read_cache_facts(&cache_dir),
            &read_empty_since(&cache_dir),
        );

        let by_month = rollups
            .iter()
            .find(|r| r.rollup_name == "orders_by_month")
            .expect("declared rollup is listed");
        assert!(!by_month.is_built, "the entry really is gone");
        assert!(
            by_month.empty_since.is_some(),
            "but the cycle ran, and said so"
        );
        // The rollup that was never touched keeps saying nothing.
        assert!(rollups.iter().any(|r| r.empty_since.is_none()));
    }

    /// The hash the layer currently declares for `rollup`, which is what the
    /// ledger records and what the status join looks up.
    fn declared_hash(layer: &oxy_airlayer_compat::SemanticLayer, rollup: &str) -> String {
        layer
            .views
            .iter()
            .flat_map(oxy_airlayer_compat::preagg::resolve_rollups)
            .find(|r| r.name == rollup)
            .expect("declared rollup resolves")
            .hash
    }

    /// The window nit #2 named: between editing a rollup's `dimensions:` and
    /// the next cycle's prune, the old hash's record is the only ledger entry
    /// for that rollup. Joining on names would render "Empty — the last rebuild
    /// found no rows" for a spec nothing has attempted; joining on the
    /// currently-declared hash renders the truth.
    #[tokio::test]
    async fn a_record_from_a_superseded_hash_does_not_describe_the_new_spec() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join(".airlayer").join("cache");
        write_manifest(&cache_dir, serde_json::json!({ "rollups": [] }));
        crate::server::preagg_ledger::record_empty(
            &cache_dir,
            "a-hash-no-longer-declared",
            1,
            "orders",
            "orders_by_month",
            None,
        )
        .await;

        let layer = layer_from_yaml(&[ORDERS_VIEW]);
        let rollups = declared_rollups(
            &layer,
            &read_cache_facts(&cache_dir),
            &read_empty_since(&cache_dir),
        );
        assert!(
            rollups.iter().all(|r| r.empty_since.is_none()),
            "the record is about a rollup this layer no longer declares"
        );
    }

    /// A built rollup's rows are the fleet-wide fact and win over this node's
    /// own stale empty record — node A retracts as empty, node B rebuilds with
    /// rows, A syncs B's manifest. A row claiming both would contradict itself.
    #[tokio::test]
    async fn a_built_rollup_never_ships_an_empty_since() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join(".airlayer").join("cache");
        let layer = layer_from_yaml(&[ORDERS_VIEW]);
        crate::server::preagg_ledger::record_empty(
            &cache_dir,
            &declared_hash(&layer, "orders_by_month"),
            1,
            "orders",
            "orders_by_month",
            None,
        )
        .await;
        write_manifest(
            &cache_dir,
            serde_json::json!({
                "rollups": [{
                    "view_name": "orders",
                    "rollup_name": "orders_by_month",
                    "rollup_hash": declared_hash(&layer, "orders_by_month"),
                    "file": "orders__aabbccdd.parquet",
                    "build_date": "2026-05-11 14:03:22"
                }]
            }),
        );

        let rollups = declared_rollups(
            &layer,
            &read_cache_facts(&cache_dir),
            &read_empty_since(&cache_dir),
        );
        let by_month = rollups
            .iter()
            .find(|r| r.rollup_name == "orders_by_month")
            .expect("listed");
        assert!(by_month.is_built);
        assert!(
            by_month.empty_since.is_none(),
            "another node's rows outrank this node's stale empty record"
        );
    }

    /// The status side of finding #1: once a rollup has rows again, nothing
    /// may still report it empty — including a record left by the hash it had
    /// before its `dimensions:` were edited.
    #[tokio::test]
    async fn a_rollup_with_rows_again_stops_reporting_empty() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join(".airlayer").join("cache");
        let layer = layer_from_yaml(&[ORDERS_VIEW]);
        crate::server::preagg_ledger::record_empty(
            &cache_dir,
            "the-hash-before-the-edit",
            1,
            "orders",
            "orders_by_month",
            None,
        )
        .await;
        crate::server::preagg_ledger::record_built(
            &cache_dir,
            &declared_hash(&layer, "orders_by_month"),
            1,
            "orders",
            "orders_by_month",
        )
        .await;
        write_manifest(&cache_dir, serde_json::json!({ "rollups": [] }));

        let rollups = declared_rollups(
            &layer,
            &read_cache_facts(&cache_dir),
            &read_empty_since(&cache_dir),
        );
        assert!(
            rollups.iter().all(|r| r.empty_since.is_none()),
            "a stale sibling record must not outlive the rebuild that filled it"
        );
    }

    /// The fleet shape: another node ran the rebuild and mirrored the manifest
    /// here, but the Parquet is not on this disk. That is BUILT — the query
    /// path reads the object straight out of the blob store — and it must not
    /// be spelled the same as "never built", which is what a single
    /// `has_parquet` column did.
    #[test]
    fn a_manifest_entry_without_its_parquet_is_built_but_not_local() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join(".airlayer").join("cache");
        let layer = layer_from_yaml(&[ORDERS_VIEW]);
        write_manifest(
            &cache_dir,
            serde_json::json!({
                "rollups": [{
                    "view_name": "orders",
                    "rollup_name": "orders_by_month",
                    "rollup_hash": declared_hash(&layer, "orders_by_month"),
                    "file": "orders__aabbccdd.parquet"
                }]
            }),
        );

        let rollups = declared_rollups(
            &layer,
            &read_cache_facts(&cache_dir),
            &read_empty_since(&cache_dir),
        );
        assert!(
            rollups[0].is_built,
            "the manifest lists it, so it was built"
        );
        assert!(!rollups[0].has_parquet, "but not on this node's disk");
    }

    /// The edit window: the rollup's `expr:` changed, the manifest still holds
    /// the row the last build wrote, and nothing has rebuilt yet. No read path
    /// will serve that row — they all pass airlayer's `live` set now — so the
    /// status must not show it as Built with the old build time under the new
    /// spec's dimensions and measures. "Not built" is the state it is in.
    #[test]
    fn a_rollup_built_from_a_superseded_definition_reads_as_not_built() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join(".airlayer").join("cache");
        write_manifest(
            &cache_dir,
            serde_json::json!({
                "rollups": [{
                    "view_name": "orders",
                    "rollup_name": "orders_by_month",
                    // Every NAME still matches what the view declares — the
                    // hash is the only thing that moved, which is exactly
                    // what a rename-free edit does.
                    "rollup_hash": "deadbeef",
                    "file": "orders__deadbeef.parquet",
                    "build_date": "2026-05-11 14:03:22"
                }]
            }),
        );
        std::fs::write(cache_dir.join("orders__deadbeef.parquet"), b"").unwrap();

        let layer = layer_from_yaml(&[ORDERS_VIEW]);
        let rollups = declared_rollups(
            &layer,
            &read_cache_facts(&cache_dir),
            &read_empty_since(&cache_dir),
        );
        let by_month = rollups
            .iter()
            .find(|r| r.rollup_name == "orders_by_month")
            .expect("still declared, so still listed");
        assert!(!by_month.is_built, "nothing serves the superseded artifact");
        assert!(!by_month.has_parquet, "and its file is not this spec's");
        assert_eq!(by_month.build_date, None, "nor is its build time");
    }

    #[test]
    fn a_cached_rollup_no_longer_declared_is_dropped() {
        // Config is the source of truth. A stale Parquet from a rollup someone
        // deleted is cache debris, not a row — listing it would re-introduce
        // the "shows what was built" bug one entry at a time.
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join(".airlayer").join("cache");
        write_manifest(
            &cache_dir,
            serde_json::json!({
                "rollups": [{
                    "view_name": "orders",
                    "rollup_name": "deleted_rollup",
                    "file": "orders__deadbeef.parquet"
                }]
            }),
        );
        std::fs::write(cache_dir.join("orders__deadbeef.parquet"), b"").unwrap();

        let layer = layer_from_yaml(&[ORDERS_VIEW]);
        let rollups = declared_rollups(
            &layer,
            &read_cache_facts(&cache_dir),
            &read_empty_since(&cache_dir),
        );
        assert!(rollups.iter().all(|r| r.rollup_name != "deleted_rollup"));
    }

    #[test]
    fn a_view_declaring_no_rollups_contributes_none() {
        let layer = layer_from_yaml(&[r#"
name: customers
table: customers.csv
measures:
  - name: total_customers
    type: count
"#]);
        assert!(declared_rollups(&layer, &HashMap::new(), &HashMap::new()).is_empty());
    }

    #[test]
    fn missing_or_unparsable_manifest_reads_as_nothing_cached() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join(".airlayer").join("cache");
        assert!(read_cache_facts(&cache_dir).is_empty());

        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("manifest.json"), b"{ not json").unwrap();
        assert!(read_cache_facts(&cache_dir).is_empty());
    }

    /// Pins the literal key on the wire, not the Rust field name.
    ///
    /// The frontend reads `measure_type`; airlayer's `manifest.json` writes
    /// `type`. A bare `#[serde(rename = "type")]` satisfies the manifest and
    /// breaks the frontend, because serde applies it in both directions — and
    /// that shipped, so every measure chip rendered without its `(count)`.
    /// The TypeScript suite hand-builds its fixtures, so it asserts against
    /// the type rather than the wire and cannot catch a regression here; this
    /// is the test that can.
    #[test]
    fn a_measure_ships_as_measure_type_and_parses_from_type() {
        let response = PreaggStatusResponse {
            blob_reads_available: false,
            rollups: vec![PreaggRollupStatus {
                view_name: "orders".into(),
                rollup_name: "orders_by_month".into(),
                is_built: true,
                has_parquet: true,
                dimensions: vec![],
                measures: vec![ManifestMeasure {
                    name: "total_orders".into(),
                    measure_type: "count".into(),
                }],
                time_dimension: None,
                granularity: None,
                refresh_key: None,
                build_date: None,
                refresh_key_checked_at: None,
                empty_since: None,
            }],
        };
        let wire = serde_json::to_value(&response).unwrap();
        let measure = &wire["rollups"][0]["measures"][0];
        assert_eq!(measure["measure_type"], "count");
        assert!(
            measure.get("type").is_none(),
            "serializing under `type` is what broke the measure chips: {measure}"
        );

        // The other direction still has to accept the manifest's own key.
        let parsed: ManifestMeasure =
            serde_json::from_value(serde_json::json!({"name": "n", "type": "sum"})).unwrap();
        assert_eq!(parsed.measure_type, "sum");
    }

    #[test]
    fn timestamps_go_out_as_rfc3339_utc() {
        // Bare date — the shape the older manifests carry. Must not reach the
        // browser as `2026-05-11`, which `new Date()` reads as UTC midnight and
        // renders as May 10 evening anywhere west of UTC.
        assert_eq!(
            normalize_manifest_timestamp("2026-05-11").as_deref(),
            Some("2026-05-11T00:00:00+00:00")
        );
        // Naive datetime — what airlayer writes and `preagg_executor` parses
        // back with "%Y-%m-%d %H:%M:%S". Read as UTC, not as viewer-local.
        assert_eq!(
            normalize_manifest_timestamp("2026-05-11 14:03:22").as_deref(),
            Some("2026-05-11T14:03:22+00:00")
        );
        // Already zoned (`refresh_key_checked_at`) — normalized to UTC, kept.
        assert_eq!(
            normalize_manifest_timestamp("2026-05-11T16:03:22+02:00").as_deref(),
            Some("2026-05-11T14:03:22+00:00")
        );
        // Anything else is dropped rather than guessed: the frontend renders an
        // em dash, where a passthrough would render "Invalid Date".
        assert_eq!(normalize_manifest_timestamp("last tuesday"), None);
        assert_eq!(normalize_manifest_timestamp("   "), None);
    }
}

// ── Rebuild trigger ───────────────────────────────────────────────────────────

/// What the UI's Rebuild buttons ask for.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
pub struct PreaggRebuildRequest {
    /// Restrict to one rollup. Omitted (or `null`) rebuilds every declared one.
    #[serde(default)]
    pub view: Option<String>,
    #[serde(default)]
    pub rollup: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PreaggRebuildResponse {
    /// The run row this rebuild reports into, so a caller can follow it in the
    /// run history like any other agentic job.
    pub run_id: String,
    /// How many declared rollups the request covers. `1` for a targeted
    /// rebuild; the workspace's whole declared set otherwise.
    pub rollups: usize,
}

/// `POST /{workspace_id}/semantic/preagg-rebuild`
///
/// Rebuild pre-aggregations on demand — all of them, or the one named by
/// `{view, rollup}`. **Forced**: a rollup is rebuilt whether or not its refresh
/// key says it's stale, because a person pressing Rebuild is saying the key is
/// not the authority right now.
///
/// Enqueues a durable `TaskSpec::Custom { kind: "preagg_cycle" }`
/// (`agentic_pipeline::scheduler::enqueue_preagg_cycle`) — the same shape the
/// scheduled cycle uses, drained by `PreaggTaskExecutor` on the worker fleet.
/// Not pinned to this node: the executor rebuilds `WorkspaceManager` fresh from
/// `workspace_id`, so any fleet node can pick up the task, and the built
/// Parquet + manifest are mirrored to S3 (`preagg_rebuild::write_preagg_parquet`
/// / manifest write) so a later query on a DIFFERENT node still finds it.
///
/// Returns as soon as the rebuild is submitted, not once it finishes — a
/// rollup rebuild is a warehouse round-trip, worth minutes for a large one, and
/// an HTTP request shouldn't hold a connection open for it. The caller polls
/// `preagg-status` to watch the row fill in; the run row carries the outcome
/// for anyone who needs the whole story.
pub async fn rebuild_preagg(
    State(state): State<AppState>,
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    layer_cache: SemanticLayerCacheCtx,
    Path(WorkspacePath { workspace_id }): Path<WorkspacePath>,
    Json(req): Json<PreaggRebuildRequest>,
) -> Result<extract::Json<PreaggRebuildResponse>, (StatusCode, extract::Json<ErrorResponse>)> {
    let target = match (req.view.as_deref(), req.rollup.as_deref()) {
        (Some(view), Some(rollup)) => Some((view.to_string(), rollup.to_string())),
        (None, None) => None,
        // Half a target is a client bug, and silently rebuilding everything
        // because one field was dropped is the expensive way to find out.
        _ => {
            return Err(semantic_err(
                StatusCode::BAD_REQUEST,
                "rebuild takes both `view` and `rollup`, or neither".to_string(),
            ));
        }
    };

    // Validate against the declared set before spending a warehouse query on
    // it: a typo'd rollup name would otherwise submit a run that quietly
    // rebuilds nothing and reports success.
    let scan = resolve_query_scan_source(&workspace_manager)
        .await
        .map_err(|e| semantic_err(StatusCode::SERVICE_UNAVAILABLE, e.message()))?;
    let layer = layer_cache
        .get_or_load(
            scan.source_revision(&workspace_manager),
            scan.scan_path.clone(),
        )
        .await
        .map_err(|e| {
            semantic_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load semantic model: {e}"),
            )
        })?;
    let declared = declared_rollups(&layer, &HashMap::new(), &HashMap::new());
    let covered = match &target {
        Some((view, rollup)) => declared
            .iter()
            .filter(|r| &r.view_name == view && &r.rollup_name == rollup)
            .count(),
        None => declared.len(),
    };
    if covered == 0 {
        return Err(semantic_err(
            StatusCode::NOT_FOUND,
            match &target {
                Some((view, rollup)) => format!("no rollup `{rollup}` declared on view `{view}`"),
                None => "this workspace declares no pre-aggregations".to_string(),
            },
        ));
    }

    let agentic = state.agentic_state.as_ref().ok_or_else(|| {
        semantic_err(
            StatusCode::SERVICE_UNAVAILABLE,
            "agentic runtime unavailable on this instance".to_string(),
        )
    })?;
    let run_id =
        agentic_pipeline::scheduler::enqueue_preagg_cycle(&agentic.db, workspace_id, target)
            .await
            .map_err(|e| semantic_err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(extract::Json(PreaggRebuildResponse {
        run_id,
        rollups: covered,
    }))
}
