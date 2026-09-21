//! Semantic-model endpoints for the IDE:
//!
//! - `GET /semantic/topic/{pathb64}` — parse one `.topic.yml` and hydrate its views.
//! - `GET /semantic/view/{pathb64}` — parse one `.view.yml`.
//! - `POST /semantic/compile` — compile a `{ topic, dimensions, measures, … }`
//!   query into dialect-specific SQL via airlayer.
//! - `POST /semantic` — compile **and execute** the same query, returning
//!   rows (JSON) or a parquet file handle. Used by the IDE's "Run" button.
//!
//! Compile + execute both go through `agentic_automation::semantic_bridge`
//! and `agentic_connector` — same code paths the agentic pipeline's
//! `semantic_query` step uses, so IDE results stay in lockstep with
//! runtime results without re-introducing `oxy-workflow`.

use axum::{
    extract::{self, Path},
    http::StatusCode,
};
use base64::Engine;
use base64::prelude::BASE64_STANDARD;
use oxy_airlayer_compat::engine::promotions::Promotions;
use oxy_semantic::parse_semantic_layer_from_dir;
use oxy_semantic::parser::{ParserConfig, SemanticLayerParser};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use agentic_semantic::compile::{CompiledQuery, resolve_and_compile_cached};
use agentic_semantic::config::SemanticQueryConfig;
use oxy::adapters::session_filters::SessionFilters;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::model::ConnectionOverrides;
use oxy_auth::extractor::AuthenticatedUserExtractor;

use crate::server::api::data::{
    ResultFormat, SQLParams, SemanticQueryResponse, SqlErrorResponse, SqlExecuteError,
    agentic_error_response, run_via_agentic_connector,
};
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, PreaggCacheCtx, SemanticEngineCacheCtx, SemanticLayerCacheCtx,
    WorkspaceManagerReadOnly,
};
use crate::server::api::semantic_scan::{self, ScanDir, SemanticEntity};
use oxy::config::{ConfigManager, DiskSlot, ResolveWorkspaceFile};

#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    pub message: String,
}

#[derive(Deserialize)]
pub struct ViewPath {
    pub workspace_id: Uuid,
    pub file_path_b64: String,
}

#[derive(Deserialize)]
pub struct TopicPath {
    pub workspace_id: Uuid,
    pub file_path_b64: String,
}

#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct ViewResponse {
    pub view_name: String,
    pub name: String,
    pub description: Option<String>,
    pub datasource: Option<String>,
    pub table: Option<String>,
    #[schema(value_type = Vec<Object>)]
    pub dimensions: Vec<serde_json::Value>,
    #[schema(value_type = Vec<Object>)]
    pub measures: Vec<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct TopicResponse {
    pub name: String,
    pub description: Option<String>,
    pub views: Vec<String>,
    pub base_view: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, ToSchema)]
pub struct TopicDetailsResponse {
    pub topic: TopicResponse,
    pub views: Vec<ViewResponse>,
}

/// Decode a base64-encoded workspace-relative file path, accepting both
/// the standard padded form (`...==`) and the unpadded form the IDE
/// historically used. Mirrors the same `path_b64` tolerance the
/// agentic-workflow file route applies — keeps the routes consistent
/// for clients that pick one form or the other.
fn decode_b64_path(
    file_path_b64: &str,
) -> Result<String, (StatusCode, extract::Json<ErrorResponse>)> {
    let decoded = BASE64_STANDARD
        .decode(file_path_b64)
        .or_else(|_| {
            // Tolerate unpadded base64 by retrying with the standard padding rule.
            let pad = (4 - (file_path_b64.len() % 4)) % 4;
            let padded = format!("{file_path_b64}{}", "=".repeat(pad));
            BASE64_STANDARD.decode(padded)
        })
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                extract::Json(ErrorResponse {
                    message: format!("Invalid base64 file path: {e}"),
                }),
            )
        })?;
    String::from_utf8(decoded).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            extract::Json(ErrorResponse {
                message: format!("Invalid UTF-8 in file path: {e}"),
            }),
        )
    })
}

pub async fn get_view_details(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    layer_cache: SemanticLayerCacheCtx,
    Path(ViewPath {
        workspace_id: _,
        file_path_b64,
    }): Path<ViewPath>,
) -> Result<extract::Json<ViewResponse>, (StatusCode, extract::Json<ErrorResponse>)> {
    let file_path_str = decode_b64_path(&file_path_b64)?;
    // Boundary first (a stateless serve replica has no working copy), FS fallback
    // in local / not-yet-promoted mode. `_guard` keeps the materialised tempdir
    // alive until parsing finishes.
    let (scan_path, view_file, guard) = resolve_semantic_source(
        &workspace_manager,
        SemanticEntity::View,
        &file_path_str,
        "View",
    )
    .await?;
    let source_revision = scan_source_revision(&guard, &workspace_manager);

    let parser = SemanticLayerParser::new(ParserConfig::new(&scan_path));
    let view = parser.parse_view_file(&view_file).map_err(|e| {
        semantic_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to parse view file: {e}"),
        )
    })?;

    // Load the full airlayer semantic model to compute the promotion closure.
    // Induced measures are defined on fine-grain views but become queryable at
    // every coarser-grain ancestor — they don't exist in the single-file YAML.
    // `scan_path` is the whole promoted scan (boundary tempdir or FS fallback),
    // so loading it gives the cross-view graph the promotion closure needs.
    let airlayer_layer = layer_cache
        .get_or_load(source_revision, scan_path.clone())
        .await
        .map_err(|e| {
            semantic_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load semantic model: {e}"),
            )
        })?;
    let promotions = Promotions::build(&airlayer_layer.views).map_err(|e| {
        semantic_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to build promotion closure: {e}"),
        )
    })?;

    let mut measures = view
        .measures
        .map(|ms| {
            json_array(
                &ms.into_iter()
                    .filter(|m| !m.name.starts_with('_'))
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or_default();
    append_induced_measures(&mut measures, &view.name, &promotions, &airlayer_layer);

    Ok(extract::Json(ViewResponse {
        view_name: view.name.clone(),
        name: view.name,
        description: view.description,
        datasource: view.datasource,
        table: view.table,
        dimensions: json_array(&view.dimensions),
        measures,
    }))
}

pub async fn get_topic_details(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    layer_cache: SemanticLayerCacheCtx,
    Path(TopicPath {
        workspace_id: _,
        file_path_b64,
    }): Path<TopicPath>,
) -> Result<extract::Json<TopicDetailsResponse>, (StatusCode, extract::Json<ErrorResponse>)> {
    let file_path_str = decode_b64_path(&file_path_b64)?;
    // Boundary first, FS fallback. The WHOLE scan is materialised so the topic's
    // referenced views hydrate from the same dir (`parse_semantic_layer_from_dir`
    // below reads `semantics_path`). `_guard` holds the tempdir until then.
    let (semantics_path, topic_file, guard) = resolve_semantic_source(
        &workspace_manager,
        SemanticEntity::Topic,
        &file_path_str,
        "Topic",
    )
    .await?;
    let source_revision = scan_source_revision(&guard, &workspace_manager);

    let parser = SemanticLayerParser::new(ParserConfig::new(&semantics_path));
    let topic = parser.parse_topic_file(&topic_file).map_err(|e| {
        semantic_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to parse topic file: {e}"),
        )
    })?;

    // Hydrate referenced views from the full semantic model parse. A
    // missing reference is a real authoring error — return 400 with
    // the offending name rather than silently dropping the view, so the
    // IDE can surface it to the user.
    let parse_result = parse_semantic_layer_from_dir(&semantics_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            extract::Json(ErrorResponse {
                message: format!("Failed to parse semantic model: {e}"),
            }),
        )
    })?;

    let airlayer_layer = layer_cache
        .get_or_load(source_revision, semantics_path.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                extract::Json(ErrorResponse {
                    message: format!("Failed to load semantic model: {e}"),
                }),
            )
        })?;
    let promotions = Promotions::build(&airlayer_layer.views).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            extract::Json(ErrorResponse {
                message: format!("Failed to build promotion closure: {e}"),
            }),
        )
    })?;

    let mut views_with_data = Vec::with_capacity(topic.views.len());
    for view_name in &topic.views {
        let Some(view) = parse_result
            .semantic_layer
            .views
            .iter()
            .find(|v| v.name == *view_name)
        else {
            return Err((
                StatusCode::BAD_REQUEST,
                extract::Json(ErrorResponse {
                    message: format!("Could not find view {view_name} in semantic model"),
                }),
            ));
        };
        let visible_measures: Vec<_> = view
            .measures
            .as_ref()
            .map(|ms| ms.iter().filter(|m| !m.name.starts_with('_')).collect())
            .unwrap_or_default();
        let mut measures = json_array(&visible_measures);
        append_induced_measures(&mut measures, view_name, &promotions, &airlayer_layer);
        views_with_data.push(ViewResponse {
            view_name: view_name.clone(),
            name: view.name.clone(),
            description: view.description.clone(),
            datasource: view.datasource.clone(),
            table: view.table.clone(),
            dimensions: json_array(&view.dimensions),
            measures,
        });
    }

    Ok(extract::Json(TopicDetailsResponse {
        topic: TopicResponse {
            name: topic.name,
            description: topic.description,
            views: topic.views,
            base_view: topic.base_view,
        },
        views: views_with_data,
    }))
}

/// Resolve where to read a single view/topic: the compile boundary (materialised
/// into a tempdir) when the workspace is promoted, else the working-copy FS.
/// Returns `(scan_root, file_path, guard)` — HOLD `guard` until parsing finishes
/// (dropping it deletes the materialised tempdir).
async fn resolve_semantic_source<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    entity: SemanticEntity,
    file_path_str: &str,
    label: &str,
) -> Result<
    (std::path::PathBuf, std::path::PathBuf, Option<ScanDir>),
    (StatusCode, extract::Json<ErrorResponse>),
>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    if let Some((scan, file)) = semantic_scan::materialise_semantic_entity(
        &workspace_manager.config_manager,
        entity,
        file_path_str,
    )
    .await
    .map_err(|e| {
        semantic_err(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to materialise semantic scan: {e}"),
        )
    })? {
        return Ok((scan.path().to_path_buf(), file, Some(scan)));
    }

    // The boundary missed. `try_resolve_file` has an impl for each capability,
    // so a node without a working copy says so instead of failing to compile —
    // and instead of resolving a path under a root that is not there.
    let full_path_str = workspace_manager
        .config_manager
        .try_resolve_file(file_path_str)
        .await
        .map_err(|e| {
            semantic_err(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to resolve file path: {e}"),
            )
        })?;
    let full_path = std::path::PathBuf::from(full_path_str);
    if !full_path.exists() {
        return Err(semantic_err(
            StatusCode::NOT_FOUND,
            format!("{label} file {file_path_str} not found"),
        ));
    }
    // Reaching here means `try_resolve_file` succeeded, which only the `WorkingCopy` impl
    // can do — so there is a working copy and its root is the scan path.
    let scan_path = workspace_manager
        .config_manager
        .working_copy()
        .map(|fs| fs.root().to_path_buf())
        .ok_or_else(|| {
            semantic_err(
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "{label} {file_path_str} is not compiled and this node has no working copy"
                ),
            )
        })?;
    Ok((scan_path, full_path, None))
}

pub(crate) fn semantic_err(
    status: StatusCode,
    message: String,
) -> (StatusCode, extract::Json<ErrorResponse>) {
    (status, extract::Json(ErrorResponse { message }))
}

/// Where a semantic **query** (compile / execute) reads the layer from.
///
/// The query-shaped counterpart to [`resolve_semantic_source`], which does the
/// same for a single-file read. `_guard` owns the materialised tempdir — hold it
/// until compilation finishes or the scan root is deleted out from under it.
pub(crate) struct QueryScanSource {
    pub(crate) scan_path: std::path::PathBuf,
    _guard: Option<ScanDir>,
}

impl QueryScanSource {
    /// A scan rooted at a directory that is already on disk — the working-copy
    /// arm, where there is no materialised tempdir to guard.
    ///
    /// Lets a caller that resolved a root some other way (or a test) hand one
    /// to something that takes a `QueryScanSource`, without making `_guard`
    /// public and inviting a materialised scan to be built without its guard.
    pub(crate) fn at(scan_path: std::path::PathBuf) -> Self {
        Self {
            scan_path,
            _guard: None,
        }
    }

    /// The directory to scan. Valid only while `self` is alive: a materialised
    /// scan's tempdir is deleted when the guard drops.
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.scan_path
    }

    /// The revision this scan actually READ, or `None` for the working copy.
    pub(crate) fn source_revision<S: oxy::config::DiskSlot>(
        &self,
        workspace_manager: &WorkspaceManager<S>,
    ) -> Option<Uuid> {
        scan_source_revision(&self._guard, workspace_manager)
    }
}

/// The revision a resolved scan actually READ, or `None` for the working copy.
///
/// Not `config_manager.revision_id()`: that reports the revision the request is
/// pinned to, which is `Some` even on a node serving its own working copy.
/// Caching by the pin instead of the source lets a working-copy reader and a
/// revision reader share one layer, and one engine built from it.
///
/// A free function, not a method, because both resolvers in this module have to
/// answer it and they return different shapes: `resolve_query_scan_source`
/// wraps its guard in [`QueryScanSource`], `resolve_semantic_source` hands the
/// `Option<ScanDir>` back bare. One rule, one place.
pub(crate) fn scan_source_revision<S: oxy::config::DiskSlot>(
    guard: &Option<ScanDir>,
    workspace_manager: &WorkspaceManager<S>,
) -> Option<Uuid> {
    match guard {
        Some(scan) if scan.is_materialised() => workspace_manager.config_manager.revision_id(),
        _ => None,
    }
}

/// The workspace has no compiled semantic model and this node has no working
/// copy to fall back to.
pub(crate) struct ScanUnavailable {
    workspace_id: Uuid,
    /// The caller refused the working-copy fallback rather than lacking one.
    /// Same absence, different thing to tell the reader: one is "this node
    /// doesn't hold the files", the other is "these files are not the ones I am
    /// allowed to judge".
    compiled_only: bool,
}

impl ScanUnavailable {
    pub(crate) fn message(&self) -> String {
        if self.compiled_only {
            return format!(
                "workspace {} has no compiled semantic model; this check reads the promoted \
                 revision only and will not fall back to a working copy — a (re)compile has \
                 been enqueued",
                self.workspace_id
            );
        }
        format!(
            "workspace {} has no compiled semantic model available on this stateless \
             replica; a (re)compile has been enqueued — retry shortly",
            self.workspace_id
        )
    }
}

/// Resolve the scan root for a semantic query — compile boundary first, working
/// copy second.
///
/// Both of this module's query handlers used to read `semantics_scan_path()`
/// unconditionally, which is the workspace working copy. A stateless `serve`
/// replica has no working copy, and scanning a directory that isn't there
/// produced an EMPTY semantic model rather than an error — surfacing as
/// `Topic 'x' not found. Available: []`, i.e. a modelling mistake for what is
/// really a missing directory. Both routes are (correctly) classified `FleetOk`
/// in `role_manifest`, so they must serve from Postgres like their siblings:
/// `projects::semantic_query` (custom-app data plane) and
/// `resolve_semantic_source` (single-file IDE reads) already do exactly this.
///
/// The workspace-surface metric-tree handlers (`api::metric_tree`) are the
/// other caller, for the same reason and with a louder symptom: scanning the
/// missing directory failed outright there, 500ing every metric-tree call on
/// every workspace in cloud (oxy-hq/oxygen#878).
///
/// Branch semantics come for free: `workspace_middleware` pins the request to
/// one revision via `compiled_reader::resolve_request_revision`, which yields
/// `None` for a non-default branch on a node that HAS a working copy. The IDE
/// previewing uncommitted edits on a feature branch therefore still reads the
/// FS, exactly as before.
pub(crate) async fn resolve_query_scan_source<S: oxy::config::DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
) -> Result<QueryScanSource, ScanUnavailable> {
    let workspace_id = workspace_manager.workspace_id;
    // `scan_dir` covers both arms — the working copy's own path, or the compiled
    // rows written out — so it only fails when neither is available.
    match semantic_scan::scan_dir(&workspace_manager.config_manager).await {
        Ok(scan) => {
            return Ok(QueryScanSource {
                scan_path: scan.path().to_path_buf(),
                _guard: Some(scan),
            });
        }
        Err(e) => tracing::warn!(
            workspace_id = %workspace_id,
            error = %e,
            "semantic query: no scan directory available"
        ),
    }

    // Stateless-fleet guard, mirroring `projects::semantic_query`: refuse the FS
    // fallback on a node that has no working copy and enqueue a deduped compile
    // so the next request succeeds without operator action. Note
    // `materialise_semantic_scan` downgrades real DB errors to `None`, so this
    // also covers a transient DB failure — a retry is the right answer there too.
    //
    // One question, asked once — and asked of `semantics_scan_dir`, which goes
    // through `disk()`.
    //
    // This asked `working_copy()`, i.e. "is there a handle?", and on a replica
    // the answer is always yes: `effective_workspace_path` returns the database
    // column without stat-ing it and `ReadOnly` keeps the slot. So the `Ok` arm
    // always won, handing airlayer a directory the node does not have — the
    // "Topic not found. Available: []" this guard exists to prevent — and the
    // `ScanUnavailable` arm below, the one that enqueues the compile, could
    // never run.
    let Ok(scan_path) = workspace_manager.config_manager.semantics_scan_dir() else {
        if let Ok(db) = oxy::database::client::establish_connection().await {
            crate::server::api::middlewares::workspace_context::enqueue_lazy_compile(
                &db,
                workspace_id,
            )
            .await;
        }
        return Err(ScanUnavailable {
            workspace_id,
            compiled_only: false,
        });
    };

    Ok(QueryScanSource {
        scan_path,
        _guard: None,
    })
}

/// [`resolve_query_scan_source`] for a reader that must JUDGE the workspace
/// rather than serve it: the promoted revision only, never the working copy.
///
/// The workspace-health eval is that reader. It can run on a node that holds a
/// working copy — a single instance, or the factory doubling as the global-run
/// node — and that copy holds whatever someone is editing in the IDE right now.
/// Falling back to it means a half-written `.view.yml` on a feature branch
/// pages the on-call about a workspace whose promoted revision, and therefore
/// everything actually being served, is fine.
///
/// The `Err` arm is a real answer, not a failure to look: the workspace has no
/// compiled semantic model, so health has no opinion about its semantic layer
/// and says so (Degraded — "we learned nothing"). A compile is enqueued, so the
/// next pass succeeds without operator action. A workspace with no promoted
/// config has no health schedule row at all, so in practice this is the manual
/// `POST /workspace-health/{id}/eval` and the mid-deploy window.
pub(crate) async fn resolve_compiled_scan_source<S: oxy::config::DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
) -> Result<QueryScanSource, ScanUnavailable> {
    let workspace_id = workspace_manager.workspace_id;
    match oxy::config::scan::compiled_scan_dir(&workspace_manager.config_manager).await {
        Ok(scan) => Ok(QueryScanSource {
            scan_path: scan.path().to_path_buf(),
            _guard: Some(scan),
        }),
        Err(e) => {
            tracing::warn!(
                workspace_id = %workspace_id,
                error = %e,
                "compiled-only semantic scan: nothing compiled to read"
            );
            if let Ok(db) = oxy::database::client::establish_connection().await {
                crate::server::api::middlewares::workspace_context::enqueue_lazy_compile(
                    &db,
                    workspace_id,
                )
                .await;
            }
            Err(ScanUnavailable {
                workspace_id,
                compiled_only: true,
            })
        }
    }
}

// Pre-aggregation status moved to `api::preagg` (#2989), which also made a
// rollup built on one node readable from another through a blob bucket. The
// `Option` third state this file carried is gone with it: the list comes from
// the DECLARATIONS, so `[]` now means "declares none" and cannot mean "could
// not look".

/// Append induced (promoted) measures for `view_name` to `measures`.
///
/// Each entry mirrors the source measure's JSON shape with two extra fields:
/// `induced: true` and `promoted_from: "<source_view>"`. The frontend renders
/// them alongside explicit measures; callers can filter on `induced` for badges.
fn append_induced_measures(
    measures: &mut Vec<serde_json::Value>,
    view_name: &str,
    promotions: &Promotions,
    layer: &oxy_airlayer_compat::SemanticLayer,
) {
    for im in promotions.induced_for_view(view_name) {
        let mut json = layer
            .views
            .iter()
            .find(|v| v.name == im.source_view)
            .and_then(|v| v.measures.as_ref())
            .and_then(|ms| ms.iter().find(|m| m.name == im.source_measure))
            .and_then(|m| serde_json::to_value(m).ok())
            .unwrap_or_else(|| serde_json::json!({ "name": im.source_measure }));

        if let serde_json::Value::Object(ref mut map) = json {
            map.insert("induced".to_string(), serde_json::Value::Bool(true));
            map.insert(
                "promoted_from".to_string(),
                serde_json::Value::String(im.source_view.clone()),
            );
        }

        measures.push(json);
    }
}

// ── World Model — filter/instance types ──────────────────────────────────────

/// Serialize anything to a `Vec<serde_json::Value>`, treating a
/// non-array serialization as empty. Lets the response shape stay
/// `Vec<Object>` regardless of whether the semantic model exposes the
/// field as an array or an option/scalar.
fn json_array<T: Serialize>(value: &T) -> Vec<serde_json::Value> {
    match serde_json::to_value(value).unwrap_or(serde_json::Value::Null) {
        serde_json::Value::Array(a) => a,
        _ => Vec::new(),
    }
}

// ── World Model — instance / filter endpoints ────────────────────────────────

#[derive(Serialize, ToSchema)]
pub struct SemanticQueryCompileResponse {
    pub sql: String,
}

/// Compile a semantic query into dialect-specific SQL without executing
/// it. Used by the IDE's SQL preview panel — `SemanticExplorerContext`
/// fires this as the user clicks dimensions / measures / filters so the
/// preview stays in sync.
///
/// The request body is the same flat shape the FE has always sent
/// (`{ topic, dimensions, measures, time_dimensions, filters, orders,
/// limit, … }`); extra fields the FE includes for the execute path
/// (`variables`, `session_filters`, `connections`, `result_format`) are
/// tolerated by serde and ignored — compile doesn't need them.
///
/// Compilation goes through `agentic_automation::semantic_bridge`, which
/// drives airlayer end-to-end. Same code path the agentic-pipeline's
/// `semantic_query` step uses, so the IDE preview and runtime stay in
/// lockstep.
pub async fn compile_semantic_query(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    engine_cache: SemanticEngineCacheCtx,
    Path(WorkspacePath { workspace_id: _ }): Path<WorkspacePath>,
    extract::Json(query): extract::Json<SemanticQueryConfig>,
) -> Result<extract::Json<SemanticQueryCompileResponse>, (StatusCode, extract::Json<ErrorResponse>)>
{
    // Compile boundary first — this route is FleetOk and must not depend on a
    // working copy. `source` owns the materialised tempdir; keep it alive until
    // the blocking compile below has finished reading from it.
    let source = resolve_query_scan_source(&workspace_manager)
        .await
        .map_err(|e| semantic_err(StatusCode::SERVICE_UNAVAILABLE, e.message()))?;
    let scan_path = source.scan_path.clone();
    let databases: Vec<oxy_airlayer_compat::DatabaseConfig> = workspace_manager
        .config_manager
        .list_databases()
        .iter()
        .map(|db| oxy_airlayer_compat::database_config(db.name.clone(), db.dialect()))
        .collect();

    let engine_key = engine_cache.scan_key(source.source_revision(&workspace_manager), &databases);
    let cache = engine_cache.cache.clone();
    let compiled = tokio::task::spawn_blocking(move || {
        // Compile to warehouse SQL only (no preagg cache) — IDE preview
        // should always show the human-readable warehouse SQL, not the
        // local-Parquet rewrite.
        resolve_and_compile_cached(
            &cache, engine_key, &scan_path, &databases, &query, None, None,
        )
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            extract::Json(ErrorResponse {
                message: format!("compile task panicked: {e}"),
            }),
        )
    })?
    .map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            extract::Json(ErrorResponse {
                message: e.to_string(),
            }),
        )
    })?;

    let sql = match compiled {
        CompiledQuery::Warehouse { sql, .. } => sql,
        CompiledQuery::Preaggregation { preagg_sql, .. } => preagg_sql,
    };

    Ok(extract::Json(SemanticQueryCompileResponse { sql }))
}

#[derive(Deserialize)]
pub struct WorkspacePath {
    pub workspace_id: Uuid,
}

// ── POST /semantic ─────────────────────────────────────────────────────────

/// FE request body for the IDE's "Run" button. Mirrors the legacy
/// `SemanticQueryRequest` shape so `executeSemanticQuery` on the FE
/// keeps working untouched.
///
/// The `query` field carries the topic / dimensions / measures / filters
/// just like `/semantic/compile`. `session_filters` / `connections` /
/// `result_format` are the execute-side knobs that `/sql/query` already
/// honors; they get plumbed through to the connector verbatim.
// `SemanticQueryConfig` lives in `agentic-workflow` and doesn't impl
// `ToSchema`, so this request type stays out of the curated OpenAPI
// surface (utoipa). The handler still deserializes it via serde.
#[derive(Deserialize)]
pub struct SemanticQueryExecuteRequest {
    #[serde(flatten)]
    pub query: SemanticQueryConfig,
    #[serde(default)]
    pub session_filters: Option<SessionFilters>,
    #[serde(default)]
    pub connections: Option<ConnectionOverrides>,
    #[serde(default)]
    pub result_format: Option<ResultFormat>,
}

/// Compile a semantic query to SQL, then execute it via the same
/// connector path `/sql/query` uses. Single round-trip from the FE.
///
/// Server-side database selection: the topic's first view with a
/// `datasource:` set wins, so the FE can't accidentally route SQL to
/// the wrong warehouse — `resolve_and_compile` is the source of truth
/// for both the SQL and the database name.
pub async fn execute_semantic_query(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    PreaggCacheCtx {
        cache: preagg_cache,
        renewal_threshold_secs,
    }: PreaggCacheCtx,
    engine_cache: SemanticEngineCacheCtx,
    Path(WorkspacePath { workspace_id: _ }): Path<WorkspacePath>,
    extract::Json(payload): extract::Json<SemanticQueryExecuteRequest>,
) -> Result<extract::Json<SemanticQueryResponse>, (StatusCode, extract::Json<SqlErrorResponse>)> {
    // Compile boundary first — see `resolve_query_scan_source`. `source` owns the
    // materialised tempdir and must outlive the blocking compile below.
    let source = resolve_query_scan_source(&workspace_manager)
        .await
        .map_err(|e| sql_error_503(e.message()))?;
    let scan_path = source.scan_path.clone();
    let databases: Vec<oxy_airlayer_compat::DatabaseConfig> = workspace_manager
        .config_manager
        .list_databases()
        .iter()
        .map(|db| oxy_airlayer_compat::database_config(db.name.clone(), db.dialect()))
        .collect();

    let query = payload.query;
    // Compile through the same preagg-aware path the automation runtime
    // and analytics solver use. When a preagg cache is attached and a
    // rollup covers the request, `compiled` will be `LocalParquet` and
    // we serve from the on-disk Parquet via DuckDB instead of round-
    // tripping to the warehouse.
    let scan_path_for_compile = scan_path.clone();
    // Keyed on the workspace ID, never on `scan_path` — that's
    // `source.scan_path`, a per-request materialised tempdir on any workspace
    // with a promoted compile (see `resolve_query_scan_source`), and it is
    // also branch-dependent. Either one as a cache key sends the reader to a
    // directory nothing ever built: right rows, always from the warehouse, no
    // "Pre-aggregated" badge, no error. See `resolve_and_compile`'s doc.
    let preagg = crate::server::preagg_context::preagg_context(
        workspace_manager.workspace_id,
        preagg_cache.clone(),
        renewal_threshold_secs,
        crate::server::preagg_context::RollupFreshness::ServeStale,
    );
    // Keyed on what the scan actually READ, not on the pinned revision: this
    // node may hold a working copy and still be pinned to a revision, and the
    // world-model handlers read the working copy on exactly such a node. The
    // preagg key above is workspace-only by design; this one must not be.
    let engine_key = engine_cache.scan_key(source.source_revision(&workspace_manager), &databases);
    let cache = engine_cache.cache.clone();
    let compiled = tokio::task::spawn_blocking(move || {
        resolve_and_compile_cached(
            &cache,
            engine_key,
            &scan_path_for_compile,
            &databases,
            &query,
            preagg.as_ref(),
            None,
        )
    })
    .await
    .map_err(|e| sql_error_500(format!("compile task panicked: {e}")))?
    .map_err(|e| sql_error_400(e.to_string()))?;

    match compiled {
        CompiledQuery::Warehouse { sql, database_name } => {
            let sql_payload = SQLParams {
                sql,
                database: database_name,
                filters: payload.session_filters,
                connections: payload.connections,
                result_format: payload.result_format,
                untyped: false,
            };
            run_via_agentic_connector(&workspace_manager, user.id, role, &sql_payload)
                .await
                .map(extract::Json)
                .map_err(|e: SqlExecuteError| agentic_error_response(&sql_payload, e))
        }
        CompiledQuery::Preaggregation {
            preagg_sql,
            source,
            warehouse_sql,
            warehouse_database,
        } => {
            let started = std::time::Instant::now();
            let want_parquet = matches!(payload.result_format, Some(ResultFormat::Parquet));
            // A rollup that won't read is not a failed query: the same
            // question has a warehouse answer, and this variant carries the
            // SQL for it. Surfacing the DuckDB/HTTP error instead would turn
            // a routine state — a manifest listing a rollup whose object
            // hasn't been mirrored yet — into a 500 on exactly the nodes the
            // blob tier exists to serve.
            let fall_back_to_warehouse = |reason: String| {
                tracing::warn!(
                    remote = source.is_remote(),
                    error = %reason,
                    "preagg rollup read failed; answering from the warehouse instead"
                );
            };

            let rollup = if want_parquet {
                // The IDE Run button always requests Parquet — write the
                // DuckDB reagg result to a file in the workspace results
                // dir and return its handle so the FE can fetch it the
                // same way it does for warehouse queries.
                // `get_results_dir` lives on `ConfigManager<WorkingCopy>` — writing
                // a Parquet handle needs somewhere on this node to write it, and
                // this route is FleetOk. Ask for the capability explicitly and
                // refuse loudly when the node has none, rather than degrading to
                // a path that does not exist.
                let results_dir = workspace_manager
                    .config_manager
                    .workspace_file_resolver()
                    .ok_or_else(|| {
                        sql_error_503(
                            "this instance holds no working copy, so it cannot \
                             write a Parquet result here"
                                .to_string(),
                        )
                    })?
                    .get_results_dir()
                    .await
                    .map_err(|e| sql_error_500(format!("results dir: {e}")))?;
                tokio::fs::create_dir_all(&results_dir)
                    .await
                    .map_err(|e| sql_error_500(format!("mkdir results dir: {e}")))?;
                let file_name = format!("{}.parquet", uuid::Uuid::new_v4());
                let dest_path = results_dir.join(&file_name);
                let sql = preagg_sql.clone();
                let src = source.clone();

                tokio::task::spawn_blocking(move || {
                    // `COPY (…) TO '<dest>'` can fail mid-write, and the
                    // fallback returns a different `file_name` — so nothing
                    // would ever claim this partial file. Drop it here.
                    write_preagg_parquet(&sql, &src, &dest_path).inspect_err(|_| {
                        let _ = std::fs::remove_file(&dest_path);
                    })
                })
                .await
                .map_err(|e| sql_error_500(format!("preagg task panicked: {e}")))?
                .map(|()| {
                    extract::Json(SemanticQueryResponse::Parquet {
                        file_name,
                        is_preagg: true,
                        execution_time_ms: started.elapsed().as_millis() as u64,
                        // Preaggregation bypasses the ad-hoc row cap (it
                        // reads a bounded rollup), so it's never truncated.
                        truncated: false,
                    })
                })
            } else {
                let sql = preagg_sql.clone();
                let src = source.clone();
                tokio::task::spawn_blocking(move || {
                    agentic_semantic::preagg::execute_preagg_sql(&sql, &src)
                })
                .await
                .map_err(|e| sql_error_500(format!("preagg task panicked: {e}")))?
                .map(|result| extract::Json(preagg_json_to_response(result)))
                .map_err(|e| e.to_string())
            };

            match rollup {
                Ok(response) => Ok(response),
                Err(reason) => {
                    fall_back_to_warehouse(reason);
                    let sql_payload = SQLParams {
                        sql: warehouse_sql,
                        database: warehouse_database,
                        filters: payload.session_filters,
                        connections: payload.connections,
                        result_format: payload.result_format,
                        untyped: false,
                    };
                    run_via_agentic_connector(&workspace_manager, user.id, role, &sql_payload)
                        .await
                        .map(extract::Json)
                        .map_err(|e: SqlExecuteError| agentic_error_response(&sql_payload, e))
                }
            }
        }
    }
}

/// Write the result of `preagg_sql` (which `read_parquet(...)`s from the
/// local rollup cache) into `dest_path` as a Parquet file via DuckDB's
/// `COPY ... TO ... (FORMAT PARQUET)`. Keeps every byte inside DuckDB so
/// we never round-trip rows through Rust just to serialize them again.
fn write_preagg_parquet(
    preagg_sql: &str,
    source: &agentic_semantic::compile::PreaggSource,
    dest_path: &std::path::Path,
) -> Result<(), String> {
    // Same two-tier source as any other rollup read: a local file has to be
    // there, a blob source is read in place and has nothing to stat.
    let conn = agentic_semantic::preagg::prepared_duckdb_connection(source)
        .map_err(|e| format!("preagg duckdb pool: {e}"))?;
    let dest = dest_path
        .to_str()
        .ok_or_else(|| "non-UTF8 dest path".to_string())?
        .replace('\'', "''");
    let trimmed = preagg_sql.trim().trim_end_matches(';');
    let sql = format!("COPY ({trimmed}) TO '{dest}' (FORMAT PARQUET)");
    conn.execute_batch(&sql)
        .map_err(|e| format!("DuckDB parquet write failed: {e}"))?;
    Ok(())
}

/// Convert the JSON blob produced by `execute_preagg_sql` (shape:
/// `{columns: [..], rows: [{col: val, ...}], row_count, truncated}`) into
/// the `SemanticQueryResponse::Json(Vec<Vec<String>>)` shape the IDE
/// frontend already renders for warehouse results.
fn preagg_json_to_response(value: serde_json::Value) -> SemanticQueryResponse {
    let columns: Vec<String> = value
        .get("columns")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    // Mirror `typed_stream_to_json_array`: row 0 is the column header,
    // subsequent rows are stringified cell values in column order. The
    // IDE result table reads the header from `data[0]` — omitting it
    // makes the table render "No results to display" even when the
    // payload contains rows.
    let mut out: Vec<Vec<String>> = vec![columns.clone()];
    if let Some(arr) = value.get("rows").and_then(|r| r.as_array()) {
        for row in arr {
            out.push(
                columns
                    .iter()
                    .map(|col| match row.get(col) {
                        Some(serde_json::Value::Null) | None => String::new(),
                        Some(serde_json::Value::String(s)) => s.clone(),
                        Some(other) => other.to_string(),
                    })
                    .collect(),
            );
        }
    }
    SemanticQueryResponse::Json(out)
}

fn sql_error_400(message: String) -> (StatusCode, extract::Json<SqlErrorResponse>) {
    (
        StatusCode::BAD_REQUEST,
        extract::Json(SqlErrorResponse {
            message,
            code: None,
            detail: None,
            hint: None,
            position: None,
            sql: None,
        }),
    )
}

/// Retryable: the compiled semantic model isn't available on this replica yet.
/// Distinct from 400 (the caller's query is wrong) and 500 (we broke) — the
/// caller should retry rather than change anything.
fn sql_error_503(message: String) -> (StatusCode, extract::Json<SqlErrorResponse>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        extract::Json(SqlErrorResponse {
            message,
            code: None,
            detail: None,
            hint: None,
            position: None,
            sql: None,
        }),
    )
}

fn sql_error_500(message: String) -> (StatusCode, extract::Json<SqlErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        extract::Json(SqlErrorResponse {
            message,
            code: None,
            detail: None,
            hint: None,
            position: None,
            sql: None,
        }),
    )
}

#[cfg(test)]
mod scan_source_tests {
    use super::*;

    /// A node that does not hold the files must refuse, not hand back a path.
    ///
    /// This asked `working_copy()` — "is there a handle?" — and on a replica the
    /// answer is always yes: `effective_workspace_path` returns the database
    /// column without stat-ing it, and `ReadOnly` keeps the slot. So the `Ok`
    /// arm always won and airlayer scanned a directory that is not on the node,
    /// finding nothing: the "Topic not found. Available: []" this guard exists
    /// to prevent. The `ScanUnavailable` arm — the one that enqueues a compile
    /// so the next request succeeds — was unreachable in production.
    ///
    /// Built the replica way: slot FULL, directory absent.
    #[tokio::test]
    async fn a_node_without_the_files_refuses_instead_of_scanning_thin_air() {
        let parent = tempfile::tempdir().expect("tempdir");
        let absent = parent.path().join("never-cloned");

        let manager = oxy::adapters::workspace::builder::WorkspaceBuilder::new(uuid::Uuid::nil())
            .with_working_copy(&absent, None, oxy::config::OnMissing::Empty)
            .await
            .expect("a manager builds from the database column, unstat-ed")
            .build()
            .await
            .expect("workspace manager");

        assert!(
            manager.config_manager.working_copy().is_some(),
            "the slot is full — that is the trap this guards"
        );
        assert!(!absent.is_dir(), "and the directory is not there");

        let refused = resolve_query_scan_source(&manager).await;
        assert!(
            refused.is_err(),
            "a scan path from a node that holds nothing is worse than an error: \
             airlayer reports an empty layer and the caller blames the workspace"
        );
    }

    /// Every variant of the message keeps the phrase the agentic coordinator
    /// matches on.
    ///
    /// `agentic_runtime::…::coordinator::outcomes::is_pending_compile` demotes
    /// this failure from ERROR to WARN by substring, because the formatted
    /// string is all a domain-agnostic runtime ever receives — and
    /// `agentic-runtime` may not depend on this crate, so the two cannot share
    /// a const. That leaves the coupling invisible from the other side: reword
    /// either arm without this phrase and nothing fails, while the coordinator
    /// quietly goes back to reporting a self-healing retry as the loudest
    /// ERROR in production (993 events in the week to 2026-09-21). This test is
    /// the thing that fails instead.
    #[test]
    fn both_messages_carry_the_pending_compile_marker() {
        const MARKER: &str = "a (re)compile has been enqueued";
        for compiled_only in [false, true] {
            let message = ScanUnavailable {
                workspace_id: Uuid::nil(),
                compiled_only,
            }
            .message();
            assert!(
                message.contains(MARKER),
                "the compiled_only={compiled_only} message no longer contains {MARKER:?}, so \
                 the coordinator will log it at ERROR again: {message}"
            );
        }
    }
}
