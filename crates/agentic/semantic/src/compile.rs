//! Semantic query compilation using airlayer.
//!
//! `.view.yml` / `.topic.yml` discovery + parsing goes through the canonical
//! `oxy-airlayer-compat` shim (NOT airlayer's native directory loader, which
//! rejects oxy's `data_source` alias) so the automation path agrees with
//! analytics and the builder validator. See
//! `internal-docs/semantic-validation-standardization.md`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::{Local, NaiveDate};
use oxy_airlayer_compat::engine::query::{
    FilterOperator, OrderBy, QueryFilter, QueryRequest, TimeDimensionQuery,
};
use oxy_airlayer_compat::schema::models::TopicFilterType;
use serde_json::Value as JsonValue;

use oxy_airlayer_compat::{request_dialect, substitute_params};

use crate::config::{SemanticFilterType, SemanticQueryConfig, TimeGranularity};
use crate::error::SemanticError;
use crate::refresh_key_cache::RefreshKeyCache;

// Public types

/// Result of compiling a semantic query.
///
/// - `Warehouse` — run the SQL against the named warehouse connector.
/// - `Preaggregation` — run `preagg_sql` against an in-memory DuckDB reading
///   the pre-aggregated Parquet, from local disk or straight from the blob
///   store (see [`PreaggSource`]).
#[derive(Debug, Clone)]
pub enum CompiledQuery {
    Warehouse {
        sql: String,
        database_name: String,
    },
    Preaggregation {
        /// DuckDB rewrite whose `read_parquet(...)` targets `source`.
        preagg_sql: String,
        /// Where the Parquet `preagg_sql` reads actually lives.
        source: PreaggSource,
        /// Warehouse SQL that would have been executed without the preagg
        /// short-circuit. Surfaced to users/agents so they see the logical
        /// query, not the DuckDB rewrite.
        warehouse_sql: String,
        /// Logical warehouse the query targets (from the view datasource).
        /// Surfaced for display even when execution short-circuits to
        /// local DuckDB.
        warehouse_database: String,
    },
}

/// Where a rollup's Parquet is being read from.
///
/// The two arms are the same rollup, and the answer is identical either way —
/// they differ only in whether this node happens to hold the bytes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum PreaggSource {
    /// A file in this node's own cache directory. The fast path, and the only
    /// one a single-node deployment ever takes.
    Local(PathBuf),
    /// Read directly out of the blob store over DuckDB's `httpfs`, because
    /// another node built this rollup and this one never has.
    ///
    /// Deliberately NOT a download. DuckDB reads Parquet over `s3://` lazily
    /// and pushes projections and filters down, so a rollup another node built
    /// is queryable here without copying it — the same trick
    /// `connector::duckdb`'s S3 mirror uses to serve a local-file warehouse
    /// from the stateless fleet.
    Blob { uri: String, config: BlobConfig },
}

impl PreaggSource {
    /// The local file this source reads, if it is a local one. `None` for a
    /// blob source — there is no file to stat, which is the point.
    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Self::Local(path) => Some(path),
            Self::Blob { .. } => None,
        }
    }

    /// Whether serving this source needs a warehouse round-trip's worth of
    /// network. Used for logging, so "Pre-aggregated" can be told apart from
    /// "Pre-aggregated, but over the network".
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Blob { .. })
    }
}

/// What a DuckDB connection needs to read this workspace's rollups from the
/// blob store: the bucket, and the endpoint details `httpfs` wants.
///
/// Credentials are deliberately absent — the connection uses the pod's own
/// credential chain (see `oxy_shared::duckdb_s3`), so nothing secret travels
/// through the compile path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlobConfig {
    pub bucket: String,
    pub region: Option<String>,
    pub endpoint_url: Option<String>,
}

/// Everything the rollup short-circuit needs, in one value.
///
/// Bundled rather than passed as four positional arguments because they are
/// one decision — "may this query be served from a rollup, and how do I find
/// one?" — and because the pieces were previously easy to mismatch: the cache
/// key in particular used to be a *path*, which a caller with a materialised
/// scan path in scope could (and did) supply by mistake.
#[derive(Clone)]
pub struct PreaggContext {
    /// The cache key. Not a path: the request path resolves `?branch=` to a
    /// `.worktrees/<branch>` checkout while the rebuild cycle runs against the
    /// default-branch root, so a path-derived key sent reader and writer to
    /// two different directories and every rollup read as uncached on a
    /// feature branch.
    pub workspace_id: uuid::Uuid,
    /// Layer-1 in-process refresh-key cache, shared with the rebuild worker.
    pub cache: Arc<RwLock<RefreshKeyCache>>,
    pub renewal_threshold_secs: u64,
    /// Where to read a rollup this node did not build. `None` → local disk
    /// only, which is correct for a single-node deployment and for any
    /// deployment with no blob bucket configured.
    pub blob: Option<BlobConfig>,
    /// Decline a rollup [`check_and_seed_freshness`] does not call fresh,
    /// instead of serving it while the rebuild catches up.
    ///
    /// **Know what the gate measures.** It is a *cold-cache* guard, not a lag
    /// detector: "fresh" means this rollup's refresh key was checked within
    /// `renewal_threshold_secs` and still matches the manifest's value. It
    /// cannot see how far behind the rollup's own `preagg_cycle` is — a cache
    /// hit seeded from that same manifest agrees with it by construction. So
    /// `true` declines on a cold or expired entry (including the first look of
    /// any process, which seeds it) and on a manifest the rebuild has moved
    /// under a live entry; it does not decline a rollup that is uniformly and
    /// quietly behind. Widening it to real lag means comparing the manifest's
    /// `build_date` against the rollup's refresh interval, which this context
    /// does not carry.
    ///
    /// `false` — the read-surface posture — is right for a chart or a chat
    /// answer: a rollup a few minutes behind beats a warehouse scan, the
    /// **Pre-aggregated** badge says where the number came from, and the
    /// seeding this check does is what triggers the rebuild.
    ///
    /// `true` is for the surfaces that turn a number into an *assertion about
    /// the data* rather than a display of it — the anomaly scan and its
    /// explain. A rollup whose newest buckets are missing or partial reads as
    /// a drop, `persist_scan` writes that to the Insights Inbox, and an
    /// unhealthy transition pages Slack. `.monitor.yml`'s `freshness` horizon
    /// guards *warehouse* lag and knows nothing about rollup lag, so the cheap
    /// guard is worth having — it costs those paths a warehouse scan on the
    /// ticks where the cache cannot vouch for the rollup at all.
    pub require_fresh: bool,
}

impl PreaggContext {
    /// Prefix every one of this workspace's rollup objects lives under.
    /// Mirrors `oxy_compile::preagg_blob`, which writes them.
    fn blob_key(&self, file_name: &str) -> String {
        format!(
            "runtime/preagg/{}/{}",
            oxy_shared::state_dir::airlayer_cache_key(self.workspace_id),
            file_name
        )
    }
}

// Public API

/// Resolve a semantic query against the semantic model and compile to SQL.
///
/// `preagg` is the optional local-rollup short-circuit. When `None` (CLI,
/// tests, the builder validator) the query always compiles to warehouse SQL:
/// without a rebuild worker there is no guarantee a local Parquet is current.
///
/// `scan_path` is where the semantic model is PARSED FROM
/// (compile-boundary-safe — a materialised tempdir works fine here, and is
/// required on a stateless node with no working copy). It is deliberately NOT
/// the pre-aggregation cache key; that is `PreaggContext::workspace_id`.
pub fn resolve_and_compile(
    scan_path: &Path,
    databases: &[oxy_airlayer_compat::DatabaseConfig],
    task: &SemanticQueryConfig,
    preagg: Option<&PreaggContext>,
    pre_loaded_layer: Option<oxy_airlayer_compat::SemanticLayer>,
) -> Result<CompiledQuery, SemanticError> {
    let engine = load_and_build(scan_path, databases, pre_loaded_layer)?;
    compile_against(&engine, task, preagg)
}

/// [`resolve_and_compile`], but taking the engine from `cache`.
///
/// Prefer this wherever the caller can name the workspace and the layer source
/// it is serving. The uncached form re-reads the semantic directory and
/// rebuilds the join graph on every call, which on the per-request surfaces —
/// the custom-app data plane, `/projects/:id/semantic-query`, the IDE
/// `/semantic` endpoint — was the single largest cost in compiling a query.
///
/// `key` must describe the engine, not just the workspace: two callers on the
/// same workspace reading different layer SOURCES, or deriving dialects
/// differently, need different entries. See `oxy_airlayer_compat::EngineKey`.
///
/// Note `pre_loaded_layer` is consumed only on a miss. That is safe because
/// the caller compiles against the returned engine, never against its own
/// copy of the layer — see `compile_against`, which reads
/// `engine.semantic_layer()`.
pub fn resolve_and_compile_cached(
    cache: &oxy_airlayer_compat::SemanticEngineCache,
    key: oxy_airlayer_compat::EngineKey,
    scan_path: &Path,
    databases: &[oxy_airlayer_compat::DatabaseConfig],
    task: &SemanticQueryConfig,
    preagg: Option<&PreaggContext>,
    pre_loaded_layer: Option<oxy_airlayer_compat::SemanticLayer>,
) -> Result<CompiledQuery, SemanticError> {
    let engine = cache
        .get_or_build(key, || {
            load_and_build(scan_path, databases, pre_loaded_layer)
                .map_err(|e| oxy_airlayer_compat::SemanticError::Engine(e.to_string()))
        })
        .map_err(|e| SemanticError::Runtime(format!("semantic engine error: {e}")))?;
    compile_against(&engine, task, preagg)
}

/// Parse the layer (unless the caller pre-loaded it), then build the engine.
///
/// The engine build itself is `oxy_airlayer_compat::build_engine`, which owns
/// deriving the dialect map from the same database list — doing that here
/// separately is how the call sites drifted apart before #3034.
fn load_and_build(
    scan_path: &Path,
    databases: &[oxy_airlayer_compat::DatabaseConfig],
    pre_loaded_layer: Option<oxy_airlayer_compat::SemanticLayer>,
) -> Result<oxy_airlayer_compat::SemanticEngine, SemanticError> {
    // Use the caller-supplied layer when available (avoids re-reading the
    // workspace from disk on hot paths). Fall back to the canonical
    // shim-based discovery + parse when not provided.
    let layer = match pre_loaded_layer {
        Some(l) => l,
        None => oxy_airlayer_compat::load_layer_from_dir(scan_path)
            .map_err(|e| SemanticError::Runtime(format!("semantic engine error: {e}")))?,
    };
    oxy_airlayer_compat::build_engine(layer, databases)
        .map_err(|e| SemanticError::Runtime(format!("semantic engine error: {e}")))
}

/// Resolve the topic, build the request, compile it, and check the local
/// pre-aggregation cache — everything both entry points do once they hold an
/// engine.
fn compile_against(
    engine: &oxy_airlayer_compat::SemanticEngine,
    task: &SemanticQueryConfig,
    preagg: Option<&PreaggContext>,
) -> Result<CompiledQuery, SemanticError> {
    let semantic_layer = engine.semantic_layer();

    let topic = resolve_topic(semantic_layer, task)?;

    // Get database from views.
    let views: Vec<&oxy_airlayer_compat::View> = semantic_layer
        .views
        .iter()
        .filter(|v| topic.views.contains(&v.name))
        .collect();

    let database_name = views
        .iter()
        .find_map(|v| v.datasource.clone())
        .ok_or_else(|| {
            SemanticError::Validation(format!("No datasource found for topic '{}'", topic.name))
        })?;

    // Build date fields for filter normalization.
    let date_fields = collect_date_fields(&views);

    let request = build_query_request(
        task,
        &topic.name,
        topic.base_view.as_ref(),
        topic.default_filters.as_ref(),
        &date_fields,
    )?;

    let result = engine
        .compile_query(&request)
        .map_err(|e| SemanticError::Runtime(format!("query compilation error: {e}")))?;

    let sql = substitute_params(
        &request_dialect(engine, &request),
        &result.sql,
        &result.params,
    );

    // Check local Parquet cache with freshness validation (Layer 1).
    let live: Vec<&oxy_airlayer_compat::View> = semantic_layer.views.iter().collect();
    let live = oxy_airlayer_compat::preagg::live_rollups(&live);
    if let Some(preagg) = preagg
        && let Some(local) = try_resolve_preagg(preagg, &request, &sql, &database_name, Some(&live))
    {
        return Ok(local);
    }

    Ok(CompiledQuery::Warehouse { sql, database_name })
}

/// Compile a semantic query using a pre-built `SemanticEngine`.
///
/// Use this when compiling multiple queries against the same schema — build
/// the engine once with [`oxy_airlayer_compat::SemanticEngine::from_semantic_layer`] and
/// call this for each query so the expensive engine-build cost is paid once.
/// Returns only the SQL string; no preagg check is performed.
pub fn compile_with_engine(
    engine: &oxy_airlayer_compat::SemanticEngine,
    task: &SemanticQueryConfig,
) -> Result<String, SemanticError> {
    let semantic_layer = engine.semantic_layer();
    let topic = resolve_topic(semantic_layer, task)?;
    let views: Vec<&oxy_airlayer_compat::View> = semantic_layer
        .views
        .iter()
        .filter(|v| topic.views.contains(&v.name))
        .collect();
    let date_fields = collect_date_fields(&views);
    let request = build_query_request(
        task,
        &topic.name,
        topic.base_view.as_ref(),
        topic.default_filters.as_ref(),
        &date_fields,
    )?;
    let result = engine
        .compile_query(&request)
        .map_err(|e| SemanticError::Runtime(format!("query compilation error: {e}")))?;
    Ok(substitute_params(
        &request_dialect(engine, &request),
        &result.sql,
        &result.params,
    ))
}

/// Resolve a query against this workspace's rollups: local Parquet first, the
/// blob store second, the warehouse if neither covers it.
///
/// Extracted so the analytics solver can reuse the freshness/seed dance
/// without duplicating the manifest-loading code.
///
/// The two-tier source is what makes this work on a fleet. Only the node that
/// ran the rebuild holds the Parquet; every other node holds at most the
/// manifest (the status handler syncs it). Rather than downloading the file to
/// make the local read succeed, tier 2 points the same generated SQL at
/// `s3://…` and lets DuckDB read it in place — no copy, no staging file, no
/// blocking the caller on a download, and no divergence between what the two
/// tiers answer, since it is the same object and the same SQL.
///
/// `live` is the set of rollups the loaded schema still declares, from
/// [`oxy_airlayer_compat::preagg::live_rollups`]. A manifest row absent from it describes
/// a rollup nobody asks for any more — a measure renamed, a `pre_aggregation:`
/// block deleted — and is declined rather than served, since the Parquet
/// beside it still holds the old definition's numbers. Pass `None` only where
/// the schema genuinely isn't in scope: it means "don't check", not "nothing
/// is live". An EMPTY set is the opposite of `None` and declines everything.
pub fn try_resolve_preagg(
    preagg: &PreaggContext,
    request: &QueryRequest,
    warehouse_sql: &str,
    warehouse_database: &str,
    live: Option<&oxy_airlayer_compat::preagg::LiveRollups>,
) -> Option<CompiledQuery> {
    let cache_dir = oxy_shared::state_dir::get_airlayer_cache_dir(preagg.workspace_id);
    let manifest_path = cache_dir.join("manifest.json");
    let content = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest: oxy_airlayer_compat::preagg::LocalManifest =
        serde_json::from_str(&content).ok()?;

    let entry = oxy_airlayer_compat::preagg::check_coverage(request, &manifest.rollups, live)?;
    let is_fresh = check_and_seed_freshness(
        &preagg.cache,
        &entry.rollup_hash,
        entry.refresh_key_value.as_deref(),
        preagg.renewal_threshold_secs,
    );
    if !is_fresh {
        if preagg.require_fresh {
            // The freshness check above already seeded the cache, so the
            // rebuild is pending either way — this caller just declines to
            // answer from the rollup while it is.
            tracing::debug!(
                rollup_hash = %entry.rollup_hash,
                "preagg: declining stale rollup, caller requires a fresh one"
            );
            return None;
        }
        tracing::debug!(
            rollup_hash = %entry.rollup_hash,
            "preagg: serving stale rollup, background rebuild pending"
        );
    }

    let source = resolve_source(preagg, &cache_dir, &entry.file)?;
    // `generate_reagg_sql` takes the FROM clause as a string, so the same
    // request produces the same projection, grouping and re-aggregation for
    // either tier — only the source differs.
    let preagg_sql =
        oxy_airlayer_compat::preagg::generate_reagg_sql(request, entry, &from_clause(&source));
    if source.is_remote() {
        tracing::debug!(
            rollup_hash = %entry.rollup_hash,
            "preagg: serving from the blob store; this node has not built this rollup"
        );
    }
    Some(CompiledQuery::Preaggregation {
        preagg_sql,
        source,
        warehouse_sql: warehouse_sql.to_string(),
        warehouse_database: warehouse_database.to_string(),
    })
}

/// Local file if this node holds it, blob store if it is configured, nothing
/// otherwise — in which case the caller takes the warehouse, which is always a
/// correct answer, just a slower one.
fn resolve_source(
    preagg: &PreaggContext,
    cache_dir: &Path,
    file_name: &str,
) -> Option<PreaggSource> {
    let local = cache_dir.join(file_name);
    if local.is_file() {
        return Some(PreaggSource::Local(local));
    }
    let config = preagg.blob.clone()?;
    Some(PreaggSource::Blob {
        // Stored RAW. Escaping at construction would mean every other reader
        // of this field — a log line, the `PreaggHandle` JSON that crosses the
        // builder boundary, a future HEAD request — got doubled quotes. Escape
        // where it is interpolated into SQL, and only there.
        uri: format!("s3://{}/{}", config.bucket, preagg.blob_key(file_name)),
        config,
    })
}

/// The `FROM` expression for a source, as `generate_reagg_sql` wants it.
fn from_clause(source: &PreaggSource) -> String {
    match source {
        PreaggSource::Local(path) => format!(
            "read_parquet('{}')",
            oxy_shared::duckdb_s3::escape_string(&path.to_string_lossy())
        ),
        PreaggSource::Blob { uri, .. } => format!(
            "read_parquet('{}')",
            oxy_shared::duckdb_s3::escape_string(uri)
        ),
    }
}

/// Decide whether the cached refresh-key entry is still fresh against the
/// manifest's stored value. On a cache miss, seeds the cache from the
/// manifest and reports stale so the background worker picks it up next
/// heartbeat.
///
/// "Fresh" here is *recency of the check*, not currency of the rollup: a hit
/// means somebody resolved this `rollup_hash` within `renewal_threshold_secs`
/// and the manifest has not moved since. Because a miss seeds from the
/// manifest, cached and manifest values agree by construction after any touch
/// — so this answers "is the memoised check still usable?", and no caller may
/// read it as "the rollup is up to date". The seed is a *read* seed; see
/// [`RefreshKeyCache::insert_read_seed`].
pub fn check_and_seed_freshness(
    cache: &Arc<RwLock<RefreshKeyCache>>,
    rollup_hash: &str,
    manifest_value: Option<&str>,
    renewal_threshold_secs: u64,
) -> bool {
    let threshold = Duration::from_secs(renewal_threshold_secs);
    let guard = cache.read().expect("preagg cache lock poisoned");
    if let Some(entry) = guard.get(rollup_hash, threshold) {
        return entry.value.as_deref() == manifest_value;
    }
    drop(guard);
    let mut wguard = cache.write().expect("preagg cache lock poisoned");
    // A READ seed: it memoises this check for the next reader, and deliberately
    // does not read as build recency to the rebuild worker's `Every` evaluator,
    // which shares this cache.
    wguard.insert_read_seed(rollup_hash.to_string(), manifest_value.map(String::from));
    false
}

/// Get the database (datasource) name from the first view that has one.
pub fn get_database_from_views(views: &[oxy_airlayer_compat::View]) -> Option<String> {
    views.iter().find_map(|v| v.datasource.clone())
}

// Internal: topic resolution

fn resolve_topic(
    semantic_layer: &oxy_airlayer_compat::SemanticLayer,
    task: &SemanticQueryConfig,
) -> Result<oxy_airlayer_compat::Topic, SemanticError> {
    let empty = Vec::new();
    let topics = semantic_layer.topics.as_ref().unwrap_or(&empty);

    if let Some(topic_name) = &task.topic {
        topics
            .iter()
            .find(|t| t.name == *topic_name)
            .cloned()
            .ok_or_else(|| {
                let available: Vec<_> = topics.iter().map(|t| t.name.clone()).collect();
                SemanticError::Validation(format!(
                    "Topic '{}' not found. Available: {:?}",
                    topic_name, available
                ))
            })
    } else {
        let mut view_names = HashSet::new();
        for dim in &task.dimensions {
            if let Some((view, _)) = dim.split_once('.') {
                view_names.insert(view.to_string());
            }
        }
        for td in &task.time_dimensions {
            if let Some((view, _)) = td.dimension.split_once('.') {
                view_names.insert(view.to_string());
            }
        }
        for m in &task.measures {
            if let Some((view, _)) = m.split_once('.') {
                view_names.insert(view.to_string());
            }
        }
        if view_names.is_empty() {
            return Err(SemanticError::Validation(
                "No dimensions or measures specified".to_string(),
            ));
        }
        Ok(oxy_airlayer_compat::Topic {
            name: "adhoc_query".to_string(),
            description: Some("Ad-hoc query inferred from views".to_string()),
            views: view_names.into_iter().collect(),
            base_view: None,
            retrieval: None,
            default_filters: None,
            meta: None,
        })
    }
}

// Internal: date field tracking

fn collect_date_fields(views: &[&oxy_airlayer_compat::View]) -> HashSet<String> {
    use oxy_airlayer_compat::schema::models::DimensionType;
    let mut date_fields = HashSet::new();
    for view in views {
        for dim in &view.dimensions {
            if matches!(
                dim.dimension_type,
                DimensionType::Date | DimensionType::Datetime
            ) {
                date_fields.insert(format!("{}.{}", view.name, dim.name));
            }
        }
    }
    date_fields
}

fn normalize_date_value(date: &str) -> Result<String, SemanticError> {
    if NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok() {
        return Ok(date.to_string());
    }
    let result = chrono_english::parse_date_string(date, Local::now(), chrono_english::Dialect::Us)
        .map_err(|e| {
            SemanticError::Runtime(format!(
                "Failed to parse date '{}': {}. Expected YYYY-MM-DD or relative expression.",
                date, e
            ))
        })?;
    Ok(result.format("%Y-%m-%d").to_string())
}

// Internal: query request building

fn build_query_request(
    task: &SemanticQueryConfig,
    topic_name: &str,
    base_view: Option<&String>,
    default_filters: Option<&Vec<oxy_airlayer_compat::schema::models::TopicFilter>>,
    date_fields: &HashSet<String>,
) -> Result<QueryRequest, SemanticError> {
    let mut filters = Vec::new();

    if let Some(defaults) = default_filters {
        for df in defaults {
            let field = qualify_field(&df.field, topic_name);
            let (op, vals) = convert_topic_filter_type(&df.filter_type, &field, date_fields)?;
            filters.push(QueryFilter {
                member: Some(field),
                operator: Some(op),
                values: vals,
                and: None,
                or: None,
            });
        }
    }

    for f in &task.filters {
        let field = qualify_field(&f.field, topic_name);
        let (op, vals) = convert_semantic_filter_type(&f.filter_type, &field, date_fields)?;
        filters.push(QueryFilter {
            member: Some(field),
            operator: Some(op),
            values: vals,
            and: None,
            or: None,
        });
    }

    let order: Vec<OrderBy> = task
        .orders
        .iter()
        .map(|o| OrderBy {
            id: qualify_field(&o.field, topic_name),
            desc: o.direction.to_lowercase() == "desc",
        })
        .collect();

    let time_dimensions: Vec<TimeDimensionQuery> = task
        .time_dimensions
        .iter()
        .map(|td| TimeDimensionQuery {
            dimension: qualify_field(&td.dimension, topic_name),
            granularity: td.granularity.as_ref().map(granularity_to_string),
            date_range: None,
        })
        .collect();

    let through = base_view.map(|bv| vec![bv.clone()]).unwrap_or_default();

    Ok(QueryRequest {
        measures: task.measures.clone(),
        dimensions: task.dimensions.clone(),
        filters,
        segments: vec![],
        time_dimensions,
        order,
        limit: task.limit,
        offset: task.offset,
        timezone: None,
        ungrouped: false,
        through,
        motif: None,
        motif_params: Default::default(),
    })
}

fn qualify_field(field: &str, topic_name: &str) -> String {
    if field.contains('.') {
        field.to_string()
    } else {
        format!("{topic_name}.{field}")
    }
}

fn granularity_to_string(g: &TimeGranularity) -> String {
    match g {
        TimeGranularity::Year => "year",
        TimeGranularity::Quarter => "quarter",
        TimeGranularity::Month => "month",
        TimeGranularity::Week => "week",
        TimeGranularity::Day => "day",
        TimeGranularity::Hour => "hour",
        TimeGranularity::Minute => "minute",
        TimeGranularity::Second => "second",
    }
    .to_string()
}

// Internal: filter conversion

fn convert_topic_filter_type(
    ft: &TopicFilterType,
    field: &str,
    date_fields: &HashSet<String>,
) -> Result<(FilterOperator, Vec<String>), SemanticError> {
    match ft {
        TopicFilterType::Eq(f) => Ok((
            FilterOperator::Equals,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
        TopicFilterType::Neq(f) => Ok((
            FilterOperator::NotEquals,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
        TopicFilterType::Gt(f) => Ok((
            FilterOperator::Gt,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
        TopicFilterType::Gte(f) => Ok((
            FilterOperator::Gte,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
        TopicFilterType::Lt(f) => Ok((
            FilterOperator::Lt,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
        TopicFilterType::Lte(f) => Ok((
            FilterOperator::Lte,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
        TopicFilterType::In(f) => {
            let v = f
                .values
                .iter()
                .map(|v| jv2s(v, field, date_fields))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((FilterOperator::Equals, v))
        }
        TopicFilterType::NotIn(f) => {
            let v = f
                .values
                .iter()
                .map(|v| jv2s(v, field, date_fields))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((FilterOperator::NotEquals, v))
        }
        TopicFilterType::InDateRange(f) => Ok((
            FilterOperator::InDateRange,
            vec![
                jv2s(&f.from, field, date_fields)?,
                jv2s(&f.to, field, date_fields)?,
            ],
        )),
        TopicFilterType::NotInDateRange(f) => Ok((
            FilterOperator::NotInDateRange,
            vec![
                jv2s(&f.from, field, date_fields)?,
                jv2s(&f.to, field, date_fields)?,
            ],
        )),
    }
}

fn convert_semantic_filter_type(
    ft: &SemanticFilterType,
    field: &str,
    date_fields: &HashSet<String>,
) -> Result<(FilterOperator, Vec<String>), SemanticError> {
    match ft {
        SemanticFilterType::Eq(f) => Ok((
            FilterOperator::Equals,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
        SemanticFilterType::Neq(f) => Ok((
            FilterOperator::NotEquals,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
        SemanticFilterType::Gt(f) => Ok((
            FilterOperator::Gt,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
        SemanticFilterType::Gte(f) => Ok((
            FilterOperator::Gte,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
        SemanticFilterType::Lt(f) => Ok((
            FilterOperator::Lt,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
        SemanticFilterType::Lte(f) => Ok((
            FilterOperator::Lte,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
        SemanticFilterType::In(f) => {
            let v = f
                .values
                .iter()
                .map(|v| jv2s(v, field, date_fields))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((FilterOperator::Equals, v))
        }
        SemanticFilterType::NotIn(f) => {
            let v = f
                .values
                .iter()
                .map(|v| jv2s(v, field, date_fields))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((FilterOperator::NotEquals, v))
        }
        SemanticFilterType::InDateRange(f) => Ok((
            FilterOperator::InDateRange,
            vec![
                jv2s(&f.from, field, date_fields)?,
                jv2s(&f.to, field, date_fields)?,
            ],
        )),
        SemanticFilterType::NotInDateRange(f) => Ok((
            FilterOperator::NotInDateRange,
            vec![
                jv2s(&f.from, field, date_fields)?,
                jv2s(&f.to, field, date_fields)?,
            ],
        )),
        SemanticFilterType::Contains(f) => Ok((
            FilterOperator::Contains,
            vec![jv2s(&f.value, field, date_fields)?],
        )),
    }
}

/// JSON value to string, with date normalization for date fields.
fn jv2s(
    value: &JsonValue,
    field: &str,
    date_fields: &HashSet<String>,
) -> Result<String, SemanticError> {
    let s = match value {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => {
            return Err(SemanticError::Runtime(format!(
                "NULL filter value for '{field}'"
            )));
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    if date_fields.contains(field) {
        return normalize_date_value(&s);
    }
    Ok(s)
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qualify_field() {
        assert_eq!(qualify_field("revenue", "orders"), "orders.revenue");
        assert_eq!(qualify_field("orders.revenue", "orders"), "orders.revenue");
    }

    #[test]
    fn test_get_database_from_views() {
        let views = vec![
            oxy_airlayer_compat::View {
                name: "v1".into(),
                description: None,
                label: None,
                datasource: None,
                dialect: None,
                table: Some("t".into()),
                sql: None,
                entities: vec![],
                dimensions: vec![],
                measures: None,
                segments: vec![],
                pre_aggregations: None,
                refresh_key: None,
                meta: None,
            },
            oxy_airlayer_compat::View {
                name: "v2".into(),
                description: None,
                label: None,
                datasource: Some("my_db".into()),
                dialect: None,
                table: Some("t".into()),
                sql: None,
                entities: vec![],
                dimensions: vec![],
                measures: None,
                segments: vec![],
                pre_aggregations: None,
                refresh_key: None,
                meta: None,
            },
        ];
        assert_eq!(get_database_from_views(&views), Some("my_db".to_string()));
    }

    fn count_request() -> QueryRequest {
        serde_json::from_value(serde_json::json!({
            "measures": ["customers.total_customers"],
            "dimensions": []
        }))
        .unwrap()
    }

    fn write_manifest(cache_dir: &Path) {
        std::fs::create_dir_all(cache_dir).unwrap();
        let manifest = serde_json::json!({
            "pulled_at": "2026-08-24T00:00:00Z",
            "source_database": "local",
            "rollups": [{
                "view_name": "customers",
                "rollup_name": "by_count",
                "rollup_hash": "abc123",
                "file": "customers__abc123.parquet",
                "dimensions": [],
                "measures": [{"name": "total_customers", "type": "count"}],
                "time_dimension": null,
                "granularity": null,
                "build_date": "2026-08-24 00:00:00"
            }]
        });
        std::fs::write(cache_dir.join("manifest.json"), manifest.to_string()).unwrap();
    }

    /// A cache directory for one throwaway workspace, removed on drop.
    ///
    /// `get_airlayer_cache_dir` resolves the process-wide state dir, so these
    /// tests genuinely write under `~/.local/share/oxy/airlayer/cache/`. A
    /// tail cleanup only runs on the happy path; a failing assertion unwinds
    /// past it and leaves debris in a developer's real state dir, so the
    /// cleanup is a `Drop` instead.
    struct ScratchCache {
        workspace_id: uuid::Uuid,
        dir: std::path::PathBuf,
    }

    impl ScratchCache {
        fn new() -> Self {
            let workspace_id = uuid::Uuid::new_v4();
            Self {
                workspace_id,
                dir: oxy_shared::state_dir::get_airlayer_cache_dir(workspace_id),
            }
        }
    }

    impl Drop for ScratchCache {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn ctx(workspace_id: uuid::Uuid, blob: Option<BlobConfig>) -> PreaggContext {
        PreaggContext {
            workspace_id,
            cache: Arc::new(RwLock::new(RefreshKeyCache::new())),
            renewal_threshold_secs: 120,
            blob,
            // These tests exercise source resolution and SQL generation, not
            // the freshness gate; declining on staleness would take the rollup
            // out of every one of them.
            require_fresh: false,
        }
    }

    fn blob_config() -> BlobConfig {
        BlobConfig {
            bucket: "oxy-blobs".to_string(),
            region: Some("us-east-1".to_string()),
            endpoint_url: None,
        }
    }

    /// The cache key is the workspace id, so a rollup this workspace built
    /// resolves and another workspace's cache is never borrowed.
    ///
    /// The key used to be a hash of the workspace PATH, which broke two ways:
    /// a per-request materialised scan path hashed to a directory nothing had
    /// ever built, and a `?branch=`-scoped request hashed to the worktree
    /// checkout while the rebuild wrote under the default-branch root.
    #[test]
    fn local_parquet_resolves_under_the_workspace_id_key() {
        let scratch = ScratchCache::new();
        let (workspace_id, cache_dir) = (scratch.workspace_id, scratch.dir.clone());
        write_manifest(&cache_dir);
        let parquet_path = cache_dir.join("customers__abc123.parquet");
        std::fs::write(&parquet_path, b"").unwrap();

        let found = try_resolve_preagg(
            &ctx(workspace_id, None),
            &count_request(),
            "SELECT 1",
            "local",
            None,
        );
        match found {
            Some(CompiledQuery::Preaggregation { source, .. }) => {
                assert_eq!(source.local_path(), Some(parquet_path.as_path()));
                assert!(!source.is_remote());
            }
            other => panic!("expected a local resolution, got {other:?}"),
        }

        // A different workspace has built nothing — it must not borrow this
        // one's Parquet, and with no blob configured it has nowhere else to
        // look.
        assert!(
            try_resolve_preagg(
                &ctx(uuid::Uuid::new_v4(), None),
                &count_request(),
                "SELECT 1",
                "local",
                None,
            )
            .is_none(),
            "a workspace that has built nothing must not resolve to another's Parquet"
        );
    }

    /// A node holding the manifest but not the Parquet — the shape every node
    /// but the builder is in — reads the rollup straight out of the blob store
    /// rather than downloading it or dropping to the warehouse.
    #[test]
    fn a_rollup_this_node_never_built_is_read_from_the_blob_store() {
        let scratch = ScratchCache::new();
        let (workspace_id, cache_dir) = (scratch.workspace_id, scratch.dir.clone());
        write_manifest(&cache_dir); // manifest only — no Parquet next to it

        // No blob configured: local disk is the only copy, so this declines
        // and the caller takes the warehouse.
        assert!(
            try_resolve_preagg(
                &ctx(workspace_id, None),
                &count_request(),
                "SELECT 1",
                "local",
                None,
            )
            .is_none(),
            "a manifest entry with no local file and no blob store must not resolve"
        );

        let resolved = try_resolve_preagg(
            &ctx(workspace_id, Some(blob_config())),
            &count_request(),
            "SELECT 1",
            "local",
            None,
        );
        let Some(CompiledQuery::Preaggregation {
            preagg_sql, source, ..
        }) = resolved
        else {
            panic!("expected a blob resolution");
        };
        assert!(source.is_remote());
        assert_eq!(source.local_path(), None, "nothing is downloaded");
        let expected_uri =
            format!("s3://oxy-blobs/runtime/preagg/{workspace_id}/customers__abc123.parquet");
        match &source {
            PreaggSource::Blob { uri, config } => {
                assert_eq!(uri, &expected_uri);
                assert_eq!(config, &blob_config());
            }
            other => panic!("expected a blob source, got {other:?}"),
        }
        assert!(
            preagg_sql.contains(&format!("read_parquet('{expected_uri}')")),
            "the generated SQL must read the object in place: {preagg_sql}"
        );
        assert!(
            !cache_dir.join("customers__abc123.parquet").exists(),
            "reading from the blob store must not write a local copy"
        );
    }

    /// The shape of the key this crate reads under. That it MATCHES what
    /// `oxy_compile::preagg_blob` writes under cannot be asserted here —
    /// `agentic-semantic` can't depend on `oxy-compile` — so the cross-crate
    /// half lives in `oxy-app`'s `preagg_context` tests, where both are
    /// reachable. This one only pins that the shape doesn't drift silently.
    #[test]
    fn the_blob_key_is_workspace_scoped_under_the_runtime_prefix() {
        let workspace_id = uuid::Uuid::from_u128(42);
        let preagg = ctx(workspace_id, Some(blob_config()));
        assert_eq!(
            preagg.blob_key("orders__deadbeef.parquet"),
            format!("runtime/preagg/{workspace_id}/orders__deadbeef.parquet")
        );
    }
}
