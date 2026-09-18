//! `ProjectContext` implementation backed by Oxy's [`WorkspaceManager`].
//!
//! All `oxy::*` imports for connector/model resolution live in this file.
//! This is the concrete adapter — the agentic stack sees only the trait.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use agentic_analytics::config::{LlmVendor, ResolvedModelInfo};
use agentic_connector::{
    BigQueryConfig, ClickHouseConfig, ConnectorConfig, DatabaseConnector, DomoConfig, DuckDbConfig,
    DuckDbLoadStrategy, DuckDbRawConfig, DuckDbUrlConfig, MysqlConfig, PostgresConfig,
    SnowflakeAuth, SnowflakeConfig,
};
use agentic_pipeline::platform::ProjectContext;
use entity::workspace_members::WorkspaceRole;
// Same trait the classic `.agent.yml` path uses to turn a `{ env_var: … }`
// header reference into a literal, so both paths resolve headers identically.
use oxy::adapters::header_value::HeaderValueExt;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::WorkingCopy;
use oxy::config::model::{DatabaseType, DuckDBOptions, Model, SnowflakeAuthType};
use oxy::config::{ConfigManager, DiskSlot, ResolveWorkspaceFile};
use oxy_shared::errors::OxyError;

// The three big trait impls live in sibling child modules; as `impl` blocks
// they need no re-export — the trait impls are visible wherever the trait and
// `OxyProjectContext` are in scope.
mod function_context;
mod monitor_scan;
mod project_context;
mod workspace_context;

/// Adapter that exposes a [`WorkspaceManager`] as a [`ProjectContext`] and
/// (for the automation runner) an [`agentic_automation::WorkspaceContext`].
///
/// Built connectors are cached via a [`tokio::sync::OnceCell`] so the automation
/// runner's `get_connector` calls don't re-resolve/re-build per step. The
/// cache is per-instance; callers that want to share it should wrap in `Arc`
/// before handing the context around.
pub struct OxyProjectContext {
    workspace_manager: WorkspaceManager<WorkingCopy>,
    /// Oxy user id of the requester, when known. Threaded into
    /// `airhouse_managed` credential resolution so each query runs as that
    /// user's provisioned Airhouse role. `None` falls back to single-row
    /// resolution (local-mode only).
    subject: Option<uuid::Uuid>,
    /// Effective workspace role of the requester. Determines the Airhouse
    /// role any minted credential carries (Owner→admin, Admin→writer,
    /// Member/Viewer→reader). `None` outside HTTP-handled paths.
    role: Option<WorkspaceRole>,
    /// Per-database lazy connector cache. Each name maps to its own
    /// `OnceCell` so building a slow connector (Snowflake handshake,
    /// BigQuery auth, …) does not block requests that only need an
    /// unrelated database. Concurrent `get_connector(name)` callers for
    /// the same name share the in-flight build.
    connectors:
        tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::OnceCell<Arc<dyn DatabaseConnector>>>>>,
    /// Shared Layer-1 refresh-key cache. The same Arc is held by the
    /// background preagg worker so both layers observe each other's writes.
    /// `None` in CLI, tests, and builder contexts (no background worker).
    preagg_cache: Option<Arc<RwLock<agentic_semantic::refresh_key_cache::RefreshKeyCache>>>,
    /// Renewal threshold (seconds) for the preagg refresh-key freshness
    /// check on the query read-path. Mirrors the worker's
    /// `pre_aggregations.refresh_worker.renewal_threshold` so operators who
    /// change one see the matching read-side behaviour.
    ///
    /// `None` means **nobody set one**, not "120". The distinction is what lets
    /// a context handed to `with_preagg` fall back to the workspace's own
    /// config instead of a literal: a `u64` field cannot say "unset", so every
    /// derived `PreaggCacheCtx` would carry the struct default and defeat that
    /// fallback. `preagg_renewal_threshold_secs()` still answers 120 for the
    /// trait, which is where a concrete number is actually required.
    preagg_renewal_threshold_secs: Option<u64>,
    /// The shared semantic-engine cache. `None` in CLI, tests, and builder
    /// contexts, which then compile against an engine built per call.
    ///
    /// No revision alongside it: an engine's identity is the layer SOURCE, and
    /// only the caller scanning a path knows that. See
    /// `WorkspaceContext::semantic_engine_cache`.
    semantic_engine_cache: Option<Arc<oxy_airlayer_compat::SemanticEngineCache>>,
    /// Database connection. Set from `AgenticState.db` by the HTTP workspace
    /// middleware AND the in-process global driver (`recovery.rs`). Powers
    /// anomaly-store queries and `compile_dispatcher()` — so any path that
    /// executes `TaskSpec::Compile` (the global driver) MUST set it, or every
    /// compile fails with "compile_dispatcher() returned None". Pure CLI / test
    /// paths may leave it unset; anomaly tools + compile are then disabled.
    db: Option<Arc<sea_orm::DatabaseConnection>>,
    /// The semantic scan root resolved through the compile boundary, and the
    /// guard keeping it alive.
    ///
    /// `None` means "nobody resolved one", and the metric-tree runners then
    /// fall back to this node's working copy. That fallback is right for the
    /// IDE and wrong everywhere else: a background context built on the
    /// stateless fleet holds a workspace *row* whose *directory* is on another
    /// machine. Set it on any path that builds a context away from an HTTP
    /// request — see `recovery::build_workspace_ctx`.
    semantic_scan: Option<Arc<crate::server::api::semantic::QueryScanSource>>,
    /// Whether this context may fall back to the node's working copy when no
    /// scan was resolved. `true` for a reader that JUDGES the workspace rather
    /// than serving it — workspace health — where the working copy is whatever
    /// someone is editing in the IDE right now and an uncommitted edit must
    /// never page the on-call about a promoted revision that is fine.
    semantic_scan_required: bool,
}

impl OxyProjectContext {
    pub fn new(workspace_manager: WorkspaceManager<WorkingCopy>) -> Self {
        Self {
            workspace_manager,
            subject: None,
            role: None,
            connectors: tokio::sync::Mutex::new(HashMap::new()),
            preagg_cache: None,
            preagg_renewal_threshold_secs: None,
            semantic_engine_cache: None,
            db: None,
            semantic_scan: None,
            semantic_scan_required: false,
        }
    }

    /// The compile-boundary revision this context's `Config` came from, or
    /// `None` when it was built from the working copy.
    ///
    pub(super) async fn compiled_automation(
        &self,
        file_path: &str,
    ) -> Result<Option<serde_json::Value>, oxy::config::ArtifactError> {
        self.workspace_manager
            .config_manager
            .automation_definition(file_path)
            .await
    }

    pub(super) async fn compiled_agent(
        &self,
        name: &str,
    ) -> Result<Option<serde_json::Value>, oxy::config::ArtifactError> {
        self.workspace_manager
            .config_manager
            .agent_definition(name)
            .await
    }

    /// Attach the database connection. The HTTP workspace middleware and the
    /// in-process global driver (`recovery.rs`) set this from `AgenticState.db`
    /// so anomaly tools can query `metric_anomalies` and `compile_dispatcher()`
    /// can drive `TaskSpec::Compile`. Pure CLI / test paths leave it unset.
    pub fn with_db(mut self, db: Arc<sea_orm::DatabaseConnection>) -> Self {
        self.db = Some(db);
        self
    }

    /// Attach the requesting user's id. The HTTP workspace middleware sets
    /// this from `AuthenticatedUserExtractor`; background paths (scheduler,
    /// schema crawl) leave it unset.
    pub fn with_subject(mut self, subject: uuid::Uuid) -> Self {
        self.subject = Some(subject);
        self
    }

    /// Attach the requesting user's effective workspace role. The HTTP
    /// workspace middleware sets this from `EffectiveWorkspaceRole`;
    /// background paths leave it unset (the airhouse path then mints with
    /// admin via `mint_for_system`).
    pub fn with_role(mut self, role: WorkspaceRole) -> Self {
        self.role = Some(role);
        self
    }

    /// Attach the shared preagg refresh-key cache. Both the HTTP middleware
    /// and the background worker must use the same Arc so Layer 1 and Layer 2
    /// observe each other's inserts and invalidations.
    pub fn with_preagg_cache(
        mut self,
        cache: Arc<RwLock<agentic_semantic::refresh_key_cache::RefreshKeyCache>>,
    ) -> Self {
        self.preagg_cache = Some(cache);
        self
    }

    /// Attach the shared semantic-engine cache.
    ///
    /// The same `Arc` the HTTP middleware hands to handlers, so an automation
    /// step and a `/semantic` request reading the same source share one
    /// compiled engine. The key's other half — the layer source — is supplied
    /// by whoever scans, not stored here.
    pub fn with_semantic_engine_cache(
        mut self,
        cache: Arc<oxy_airlayer_compat::SemanticEngineCache>,
    ) -> Self {
        self.semantic_engine_cache = Some(cache);
        self
    }

    /// Attach the semantic scan root this context's metric-tree runners must
    /// read, resolved through the compile boundary
    /// (`semantic::resolve_query_scan_source`).
    ///
    /// The `QueryScanSource` is held for the life of the context because a
    /// materialised scan is a tempdir that vanishes when its guard drops.
    pub(crate) fn with_semantic_scan(
        mut self,
        scan: crate::server::api::semantic::QueryScanSource,
    ) -> Self {
        self.semantic_scan = Some(Arc::new(scan));
        self
    }

    /// [`Self::with_semantic_scan`] for a root that is already on disk.
    pub(crate) fn with_semantic_scan_path(self, scan_path: PathBuf) -> Self {
        self.with_semantic_scan(crate::server::api::semantic::QueryScanSource::at(scan_path))
    }

    /// Refuse the working-copy fallback: with no resolved scan, the metric-tree
    /// runners built from this context read nothing rather than reading this
    /// node's disk. Set by `recovery::build_workspace_ctx` for the
    /// workspace-health eval.
    pub(crate) fn requiring_compiled_semantic_scan(mut self) -> Self {
        self.semantic_scan_required = true;
        self
    }

    /// Where the metric-tree runners built from this context read the semantic
    /// model.
    pub(crate) fn semantic_scan_root(&self) -> crate::agentic_wiring::metric_tree_runner::ScanRoot {
        use crate::agentic_wiring::metric_tree_runner::ScanRoot;
        match (&self.semantic_scan, self.semantic_scan_required) {
            (Some(scan), _) => ScanRoot::Compiled(scan.path().to_path_buf()),
            (None, true) => ScanRoot::Unavailable,
            (None, false) => ScanRoot::WorkingCopy,
        }
    }

    /// Attach the preagg renewal threshold (seconds). Must match the
    /// background worker's `refresh_worker.renewal_threshold`.
    pub fn with_preagg_renewal_threshold_secs(mut self, secs: u64) -> Self {
        self.preagg_renewal_threshold_secs = Some(secs);
        self
    }

    /// This context's rollup short-circuit as a `PreaggCacheCtx`, for handing
    /// to another context's `with_preagg`.
    ///
    /// The threshold stays `None` when nothing explicitly set one, so the
    /// receiving context resolves it from the workspace config rather than
    /// inheriting a default that was never a decision.
    pub fn preagg_cache_ctx(
        &self,
    ) -> crate::server::api::middlewares::workspace_context::PreaggCacheCtx {
        crate::server::api::middlewares::workspace_context::PreaggCacheCtx {
            cache: self.preagg_cache.clone(),
            renewal_threshold_secs: self.preagg_renewal_threshold_secs,
        }
    }

    /// Attach the node's Layer-1 rollup short-circuit in one call.
    ///
    /// The cache and the threshold belong together and had started being paired
    /// by hand at every composition root that builds a context outside an HTTP
    /// request — the background driver, the Slack bridge, the Data App runner.
    /// One helper is what keeps a new one from attaching the cache and
    /// forgetting the threshold, which silently gives that path 120s while the
    /// rebuild cycle honours the workspace's own setting.
    ///
    /// A context with no cache resolves no rollup: every semantic query it runs
    /// compiles to warehouse SQL, which is always a correct answer and only a
    /// slower one.
    pub fn with_preagg(
        self,
        preagg: &crate::server::api::middlewares::workspace_context::PreaggCacheCtx,
    ) -> Self {
        let Some(cache) = preagg.cache.clone() else {
            return self;
        };
        // Resolved from THIS workspace's own
        // `pre_aggregations.refresh_worker.renewal_threshold` when the process
        // publishes no global value — the same key the rebuild cycle reads, and
        // the same fallback `workspace_context` applies on the request path.
        let secs = preagg.renewal_threshold_secs_or(&self.workspace_manager().config_manager);
        self.with_preagg_cache(cache)
            .with_preagg_renewal_threshold_secs(secs)
    }

    /// This context's rollup short-circuit, shaped for a metric-tree runner.
    ///
    /// `freshness` is the caller's, not the context's: the same context builds
    /// the request-driven runner (which serves a stale rollup — it is a
    /// display, badged as pre-aggregated) and the system runner behind the
    /// scheduled monitor scan (which must not, because it persists what it
    /// computes). A threshold nobody set resolves from this workspace's own
    /// config rather than a literal, same as `with_preagg`.
    pub(super) fn runner_preagg(
        &self,
        freshness: crate::server::preagg_context::RollupFreshness,
    ) -> crate::agentic_wiring::metric_tree_runner::RunnerPreagg {
        crate::agentic_wiring::metric_tree_runner::RunnerPreagg {
            cache: self.preagg_cache.clone(),
            renewal_threshold_secs: self
                .preagg_cache_ctx()
                .renewal_threshold_secs_or(&self.workspace_manager.config_manager),
            freshness,
        }
    }

    pub fn workspace_manager(&self) -> &WorkspaceManager<WorkingCopy> {
        &self.workspace_manager
    }

    pub fn subject(&self) -> Option<uuid::Uuid> {
        self.subject
    }

    pub fn role(&self) -> Option<WorkspaceRole> {
        self.role.clone()
    }

    /// True if `name` resolves to a database configured in this workspace
    /// (including runtime-overlay databases registered after a run).
    pub fn is_database_configured(&self, name: &str) -> bool {
        // `resolve_database`'s only error is the not-found case (a poisoned
        // overlay lock falls back to the static-config lookup rather than
        // erroring), so `is_ok()` here cleanly means "configured".
        self.workspace_manager
            .config_manager
            .resolve_database(name)
            .is_ok()
    }

    /// Resolve and build a `DatabaseConnector` for `db_name`, surfacing the
    /// real reason on failure (config missing, secret unresolved, host
    /// unreachable, …) instead of collapsing every failure into `None`.
    ///
    /// This is the API non-agentic callers (e.g. the Dev Portal SQL IDE)
    /// should reach for. Agentic-pipeline internals stay on
    /// `agentic_pipeline::platform::resolve_connectors`, which uses the
    /// `ProjectContext` trait's `Option`-returning methods and silently
    /// skips databases that fail to resolve — fine for batch resolution
    /// where individual failures shouldn't kill the whole pipeline, but
    /// useless when a user typed a SQL query against a specific database
    /// and wants to know why it didn't work.
    ///
    /// Dispatches the same way `resolve_pre_built_airhouse` +
    /// `database_to_connector_config` would (so the routing stays in one
    /// place), but propagates errors through `OxyError` instead of
    /// dropping them on the floor.
    /// See [`build_connector_for_db`]. The body lives in a free function generic
    /// over the capability so a handler holding a read-only manager can build a
    /// connector without genericizing `OxyProjectContext` (26 construction
    /// sites). This stays a delegation so every existing caller is untouched.
    pub async fn build_connector_for(
        &self,
        db_name: &str,
    ) -> Result<Arc<dyn DatabaseConnector>, OxyError> {
        build_connector_for_db(
            &self.workspace_manager,
            db_name,
            self.subject,
            self.role.clone(),
        )
        .await
    }
}

/// Build a connector for one database, on either capability.
///
/// The three arms that need a disk go through `try_resolve_file`
/// ([`ResolveWorkspaceFile`]), which has an impl for each capability: on a node
/// with a working copy it resolves; on one without it returns the error naming
/// the fix. Every other database — remote warehouses, DuckDB with an S3 mirror,
/// airhouse — never touches a file and works anywhere.
pub async fn build_connector_for_db<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    db_name: &str,
    subject: Option<uuid::Uuid>,
    role: Option<WorkspaceRole>,
) -> Result<Arc<dyn DatabaseConnector>, OxyError>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    let db = workspace_manager
        .config_manager
        .resolve_database(db_name)
        .map_err(|e| {
            OxyError::ConfigurationError(format!(
                "database '{db_name}' not found in config.yml: {e}"
            ))
        })?;

    // Airhouse types live outside `agentic-connector`'s vendor-neutral
    // dispatcher; the helper builds them directly with full error
    // propagation. Other types go through the config + dispatcher path.
    match &db.database_type {
        DatabaseType::Airhouse(_) | DatabaseType::AirhouseManaged(_) => {
            build_airhouse_connector(&db, workspace_manager, subject, role).await
        }
        // DuckDB Local: use the process-wide pool so CSV/Parquet files are
        // loaded only once and subsequent calls get a cheap try_clone().
        // This is the same pool that oxy-core's DuckDB connector uses,
        // avoiding the fresh-connection-per-request cost.
        // `s3_mirror.is_none()` is the whole point of this guard. A mirrored
        // DuckDB must NOT take the local-file path: it falls through to the
        // wildcard arm below, which delegates to `database_to_connector_config`,
        // whose own mirror arm attaches from S3 when the process is Serve or
        // Worker. Without this clause the `Local` pattern matched first and a
        // mirrored database on a diskless node resolved a file that is not
        // there — contradicting this function's own doc comment above, which
        // already promises that "DuckDB with an S3 mirror … never touches a
        // file and works anywhere".
        DatabaseType::DuckDB(duck)
            if duck.s3_mirror.is_none() && matches!(&duck.options, DuckDBOptions::Local { .. }) =>
        {
            let DuckDBOptions::Local { file_search_path } = &duck.options else {
                unreachable!()
            };
            let resolved = workspace_manager
                .config_manager
                .try_resolve_file(file_search_path)
                .await
                .map_err(|e| {
                    OxyError::ConfigurationError(format!(
                        "DuckDB '{db_name}': cannot resolve path '{file_search_path}': {e}"
                    ))
                })?;
            let conn = tokio::task::spawn_blocking(move || {
                oxy::connector::checkout_local_connection(&resolved)
            })
            .await
            .map_err(|e| OxyError::DBError(format!("DuckDB pool join error: {e}")))?
            .map_err(|e| OxyError::DBError(format!("DuckDB pool checkout failed: {e}")))?;
            Ok(Arc::new(agentic_connector::DuckDbConnector::new(conn))
                as Arc<dyn DatabaseConnector>)
        }
        _ => {
            let cfg = database_to_connector_config(&db, workspace_manager)
                .await
                .ok_or_else(|| {
                    OxyError::ConfigurationError(format!(
                        "could not build connector config for database '{db_name}' (type {})",
                        db.database_type_name()
                    ))
                })?;
            let built = agentic_connector::build_connector_async(cfg)
                .await
                .map_err(|e| {
                    OxyError::DBError(format!("failed to build connector for '{db_name}': {e}"))
                })?;
            Ok(Arc::from(built))
        }
    }
}

impl OxyProjectContext {
    /// Build (and cache) a single connector by name. Other databases in
    /// the workspace config are not touched, so a slow Snowflake handshake
    /// no longer delays a request that only needs `local` (DuckDB).
    async fn build_connector_lazy(&self, name: &str) -> Result<Arc<dyn DatabaseConnector>, String> {
        let cell = {
            let mut guard = self.connectors.lock().await;
            guard
                .entry(name.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
                .clone()
        };

        cell.get_or_try_init(|| async {
            if let Some(conn) = self.resolve_pre_built_connector(name).await {
                return Ok(conn);
            }
            let cfg = self
                .resolve_connector(name)
                .await
                .ok_or_else(|| format!("database '{name}' not found in workspace config"))?;
            agentic_connector::build_connector_async(cfg)
                .await
                .map(Arc::from)
                .map_err(|e| {
                    tracing::warn!(
                        target: "workspace_context",
                        db = %name,
                        error = %e,
                        "failed to build connector"
                    );
                    format!("failed to build connector for '{name}': {e}")
                })
        })
        .await
        .cloned()
    }
}

/// Validate that `workflow_ref` is a single relative path that stays under
/// the workspace root, then return the absolute path to read from.
///
/// Rejects: absolute paths, parent-dir traversal, empty refs. Errors do
/// **not** quote the resolved absolute path — only the caller-supplied
/// `workflow_ref` — so a failed traversal attempt cannot be used to probe
/// workspace layout.
async fn resolve_workspace_relative(
    workspace_manager: &WorkspaceManager<WorkingCopy>,
    workflow_ref: &str,
) -> Result<PathBuf, String> {
    validate_automation_ref_syntax(workflow_ref)?;
    let resolved = workspace_manager
        .config_manager
        .resolve_file(workflow_ref)
        .await
        .map_err(|e| format!("invalid workflow_ref {workflow_ref:?}: {e}"))?;
    Ok(PathBuf::from(resolved))
}

/// Cheap syntactic reject for obviously-unsafe `workflow_ref`s. The
/// canonicalisation-based check in `validate_path_within_project` would
/// catch absolute paths and `..` traversal too, but its error messages
/// vary by OS and by whether the path happens to exist; failing here
/// keeps the diagnostics stable and avoids touching the filesystem at all
/// for the easy cases.
fn validate_automation_ref_syntax(workflow_ref: &str) -> Result<(), String> {
    if workflow_ref.is_empty() {
        return Err("workflow_ref is empty".to_string());
    }
    let candidate = Path::new(workflow_ref);
    if candidate.is_absolute() {
        return Err(format!(
            "workflow_ref {workflow_ref:?} must be relative to the workspace"
        ));
    }
    if candidate
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!(
            "workflow_ref {workflow_ref:?} must not contain `..` segments"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{airhouse_pool_key, validate_automation_ref_syntax};

    #[test]
    fn airhouse_pool_key_is_stable_per_identity() {
        let ws = uuid::Uuid::nil();
        let user = uuid::Uuid::from_u128(1);
        let role = entity::workspace_members::WorkspaceRole::Admin;

        // Managed: keyed by (workspace, subject, role) — NOT the minted user,
        // so it survives credential rotation.
        let k1 = airhouse_pool_key(true, ws, Some(user), Some(&role), "wh");
        let k2 = airhouse_pool_key(true, ws, Some(user), Some(&role), "wh");
        assert_eq!(k1, k2);
        assert!(k1.starts_with("mgd:"));

        // Different subject / role / no-subject → distinct keys (no cross-tenant
        // or cross-role connection sharing).
        let other_user =
            airhouse_pool_key(true, ws, Some(uuid::Uuid::from_u128(2)), Some(&role), "wh");
        let other_role = airhouse_pool_key(
            true,
            ws,
            Some(user),
            Some(&entity::workspace_members::WorkspaceRole::Member),
            "wh",
        );
        let system = airhouse_pool_key(true, ws, None, None, "wh");
        assert_ne!(k1, other_user);
        assert_ne!(k1, other_role);
        assert!(system.contains("system"));

        // Static airhouse keys by (workspace, db) and ignores subject/role.
        let s1 = airhouse_pool_key(false, ws, Some(user), Some(&role), "wh");
        let s2 = airhouse_pool_key(false, ws, None, None, "wh");
        assert_eq!(s1, s2);
        assert_eq!(s1, format!("static:{ws}:wh"));
    }

    #[test]
    fn rejects_empty_ref() {
        assert!(validate_automation_ref_syntax("").is_err());
    }

    #[test]
    fn rejects_parent_dir_traversal() {
        for ref_str in [
            "../etc/passwd",
            "workflows/../../../etc/passwd",
            "..",
            "a/../b",
        ] {
            assert!(
                validate_automation_ref_syntax(ref_str).is_err(),
                "should reject {ref_str:?}"
            );
        }
    }

    #[test]
    fn rejects_absolute_paths() {
        assert!(validate_automation_ref_syntax("/etc/passwd").is_err());
    }

    #[test]
    fn accepts_relative_ref() {
        for ref_str in [
            "foo.automation.yml",
            "workflows/foo.automation.yml",
            "./foo.automation.yml",
        ] {
            assert!(
                validate_automation_ref_syntax(ref_str).is_ok(),
                "should accept {ref_str:?}"
            );
        }
    }

    /// Regression guard for the compile-boundary global-driver bug: a
    /// `TaskSpec::Compile` runs through an `OxyProjectContext`, and
    /// `compile_dispatcher()` only resolves when `db` is wired. The HTTP
    /// workspace middleware sets it; the in-process global driver
    /// (`server::router::recovery`) once did NOT, so every queued compile
    /// failed with "compile_dispatcher() returned None". This asserts the
    /// invariant both ways so a future refactor can't silently drop `db`
    /// from a compile-executing path again.
    #[tokio::test]
    async fn compile_dispatcher_resolves_only_when_db_is_wired() {
        use agentic_pipeline::platform::ProjectContext;
        use oxy::adapters::workspace::builder::WorkspaceBuilder;
        use oxy_test_utils::fixtures::TestFixture;
        use std::sync::Arc;

        let fixture = TestFixture::new().expect("tempdir");
        // Same builder path the global driver uses — falls back to a default
        // config, so an empty workspace dir is enough for this unit test.
        let wm = WorkspaceBuilder::new(uuid::Uuid::new_v4())
            .with_working_copy(fixture.path(), None, oxy::config::OnMissing::Empty)
            .await
            .expect("config builder")
            .build()
            .await
            .expect("workspace manager");

        // No db (the bug): the global driver would get None and the compile
        // executor fails before dispatching.
        let ctx_no_db = super::OxyProjectContext::new(wm.clone());
        assert!(
            ctx_no_db.compile_dispatcher().is_none(),
            "a db-less context must not resolve a compile dispatcher"
        );

        // db wired (the fix): the dispatcher resolves so compiles can run.
        // The default (disconnected) handle is fine — the dispatcher only
        // stores it here, it never opens a connection.
        let ctx_db = super::OxyProjectContext::new(wm)
            .with_db(Arc::new(sea_orm::DatabaseConnection::default()));
        assert!(
            ctx_db.compile_dispatcher().is_some(),
            "a db-wired context (HTTP middleware or global driver) must resolve a compile dispatcher"
        );
    }

    /// Regression guard for the reconcile raw-`sql:` bug: a `sql:` operand
    /// against a non-airhouse warehouse (ClickHouse / Snowflake / BigQuery /
    /// Postgres / DuckDB) failed with "database '<name>' not configured" because
    /// `reconcile::runner::run_sql` resolved through the airhouse-ONLY
    /// [`ProjectContext::resolve_pre_built_connector`], which returns `None` for
    /// any non-airhouse type — even when the database IS declared in config.yml.
    /// The fix switches `run_sql` to [`OxyProjectContext::build_connector_for`],
    /// which dispatches every warehouse type. This locks the two halves of the
    /// root cause: a configured non-airhouse database is genuinely resolvable
    /// (`resolve_database` -> Ok) yet invisible to the airhouse-only pre-built
    /// path (`resolve_pre_built_connector` -> None), so that path must not gate
    /// raw SQL.
    #[tokio::test]
    async fn non_airhouse_db_is_configured_but_not_pre_built() {
        use agentic_pipeline::platform::ProjectContext;
        use oxy::adapters::workspace::builder::WorkspaceBuilder;
        use oxy_test_utils::fixtures::TestFixture;

        let fixture = TestFixture::new().expect("tempdir");
        // A real non-airhouse database (all ClickHouse fields are optional, so a
        // bare declaration is valid). We never build the connector here — only
        // resolve it — so no network handshake happens.
        fixture
            .create_file(
                "config.yml",
                "models: []\ndatabases:\n  - name: clickhouse\n    type: clickhouse\n",
            )
            .expect("write config.yml");
        let wm = WorkspaceBuilder::new(uuid::Uuid::new_v4())
            .with_working_copy(fixture.path(), None, oxy::config::OnMissing::Fail)
            .await
            .expect("config builder")
            .build()
            .await
            .expect("workspace manager");
        let ctx = super::OxyProjectContext::new(wm);

        // The database IS configured — resolving it from config.yml succeeds.
        assert!(
            ctx.workspace_manager()
                .config_manager
                .resolve_database("clickhouse")
                .is_ok(),
            "the non-airhouse database must be present in config.yml"
        );

        // ...yet the airhouse-only pre-built path misses it. `run_sql` used to
        // map this `None` to the misleading "not configured" error; it now uses
        // `build_connector_for`, which resolves every warehouse type.
        assert!(
            ctx.resolve_pre_built_connector("clickhouse")
                .await
                .is_none(),
            "airhouse-only pre-built resolution must miss a non-airhouse database"
        );
    }
}

// ── Connector translation ───────────────────────────────────────────────────

/// Take a resolved connection field, or fall back — reporting WHY it fell back.
///
/// Both errors `SecretsManager::resolve_config_value` returns name a real
/// misconfiguration: `SecretNotFound(var)` when a `*_var` is declared but the
/// secrets store has no such secret, and `ConfigurationError` when neither the
/// field nor its `*_var` is set. The fallbacks themselves are legitimate — an
/// unset ClickHouse user, password or database means "use the server's default"
/// — so this does not change which configurations connect. What it changes is
/// that the reason is no longer discarded: an unresolvable `host_var` used to
/// become a plausible-looking connection to `http://localhost:8123` with
/// `database = ''`, and the failure surfaced as a connection error naming a
/// host nobody had configured, with the missing var mentioned nowhere.
fn or_default(
    db_name: &str,
    field: &str,
    resolved: Result<String, OxyError>,
    fallback: &str,
) -> String {
    match resolved {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                db = %db_name,
                "ClickHouse {field}: {e} — falling back to the default"
            );
            fallback.to_string()
        }
    }
}

async fn resolve_connector_impl(
    db_name: &str,
    workspace_manager: &WorkspaceManager<WorkingCopy>,
) -> Option<ConnectorConfig> {
    let db = match workspace_manager.config_manager.resolve_database(db_name) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(
                db = %db_name,
                "databases: '{}' not found in config.yml: {}",
                db_name,
                e
            );
            return None;
        }
    };
    database_to_connector_config(&db, workspace_manager).await
}

/// Translate an already-resolved [`oxy::config::model::Database`] into an
/// [`agentic_connector::ConnectorConfig`].
///
/// This is the secret-resolving counterpart of [`ConnectorConfig`]'s
/// constructors: it reads the per-type host / port / user / password /
/// database_var / developer_token_var values off `workspace_manager`'s
/// `SecretsManager`, and for DuckDB + BigQuery also walks the
/// workspace-relative file paths through `config_manager.resolve_file`.
///
/// Returns `None` for database shapes that [`agentic-connector`] does not yet
/// handle — today, Snowflake browser-auth and Snowflake private-key auth
/// both fall into this bucket. The test-connection handler uses that as a
/// signal to fall back to the legacy `oxy::connector::Connector` path.
pub async fn database_to_connector_config<S: DiskSlot>(
    db: &oxy::config::model::Database,
    workspace_manager: &WorkspaceManager<S>,
) -> Option<ConnectorConfig>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    match &db.database_type {
        // Stateless fleet only (Serve/Worker): the local DuckDB data file is
        // absent here. The compile worker mirrored this local-file warehouse to
        // S3 and recorded an `s3_mirror` block; attach from S3 via the same
        // httpfs SQL the core connector uses, instead of resolving the
        // (nonexistent) local path. Without this the connector silently drops
        // the DB and the agent run fails "no databases configured" on cloud
        // while working locally. (DuckLake / MotherDuck are already remote and
        // carry no mirror.)
        //
        // Gated on the stateless roles: a node WITH the working copy (ide /
        // all-in-one) reads the SAME compiled config (which carries `s3_mirror`)
        // but DOES have the files, so it must fall through to the `Local` arm
        // below. The S3-mirror connector exposes data as views only and has no
        // `file_search_path`, so it can't resolve authored file-path SQL
        // (`FROM 'oxymart.csv'`) — which workflow/automation steps emit and which
        // is exactly what runs on the ide once /agentic-workflows is ide-pinned.
        DatabaseType::DuckDB(duck)
            if duck.s3_mirror.is_some()
                && matches!(
                    crate::server::role_manifest::current_process_role(),
                    crate::server::role_manifest::Role::Serve
                        | crate::server::role_manifest::Role::Worker
                ) =>
        {
            let mirror = duck.s3_mirror.as_ref().expect("guarded by is_some()");
            Some(ConnectorConfig::DuckDbRaw(DuckDbRawConfig {
                init_statements: oxy::connector::build_s3_mirror_sql(mirror),
            }))
        }
        DatabaseType::DuckDB(duck) => match &duck.options {
            DuckDBOptions::Local { file_search_path } => {
                let path = match workspace_manager
                    .config_manager
                    .try_resolve_file(file_search_path)
                    .await
                {
                    Ok(p) => std::path::PathBuf::from(p),
                    Err(e) => {
                        tracing::warn!(db = %db.name, "DuckDB: cannot resolve path: {e}");
                        return None;
                    }
                };
                Some(ConnectorConfig::DuckDb(DuckDbConfig {
                    data_dir: path,
                    load_strategy: DuckDbLoadStrategy::View,
                }))
            }
            DuckDBOptions::File { path } => {
                let resolved = match workspace_manager
                    .config_manager
                    .try_resolve_file(path)
                    .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(db = %db.name, "DuckDB file: cannot resolve path: {e}");
                        return None;
                    }
                };
                Some(ConnectorConfig::DuckDbUrl(DuckDbUrlConfig {
                    url: resolved,
                }))
            }
            DuckDBOptions::DuckLake(ducklake_config) => {
                let stmts = match ducklake_config
                    .to_duckdb_attach_stmt(&workspace_manager.secrets_manager)
                    .await
                {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(db = %db.name, "DuckLake attach: {e}");
                        return None;
                    }
                };
                Some(ConnectorConfig::DuckDbRaw(DuckDbRawConfig {
                    init_statements: stmts,
                }))
            }
        },

        DatabaseType::Postgres(pg) => {
            let host = pg
                .get_host(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_else(|_| "localhost".into());
            let port: u16 = pg
                .get_port(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_else(|_| "5432".into())
                .parse()
                .unwrap_or(5432);
            let user = pg
                .get_user(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_default();
            let password = pg
                .get_password(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_default();
            let database = pg
                .get_database(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_default();
            Some(ConnectorConfig::Postgres(PostgresConfig {
                host,
                port,
                user,
                password,
                database,
                // An author-configured database; Oxy has no opinion on its TLS,
                // so this keeps tokio_postgres's `prefer`. `postgres_managed`
                // below is the one where Oxy DID decide.
                sslmode: None,
            }))
        }

        // `Airhouse` and `AirhouseManaged` are handled by the host's
        // `resolve_pre_built_connector` path — see `resolve_pre_built_airhouse`
        // below — because the connector lives in the standalone `airhouse`
        // crate, not in `agentic-connector::ConnectorConfig`.
        DatabaseType::Airhouse(_) | DatabaseType::AirhouseManaged(_) => None,

        // `postgres_managed` carries no connection details in config.yml —
        // they come from the org's `oltp_tenants` row, reached through
        // `workspace_manager.workspace_id`.
        //
        // Returning `None` here does NOT fall through to another builder, as
        // an earlier comment claimed: the caller turns `None` into
        // "could not build connector config", which is what broke schema
        // introspection in the IDE. Airhouse gets away with `None` only
        // because it has a genuine second path (`resolve_pre_built_airhouse`).
        //
        // Always the read-only analyst — see `oxy_oltp::resolver`.
        DatabaseType::PostgresManaged(_) => {
            let db_conn = oxy::database::client::establish_connection().await.ok()?;
            match oxy_oltp::resolver::resolve_analyst_connection(
                &db_conn,
                workspace_manager.workspace_id,
            )
            .await
            {
                // TLS posture is carried, not dropped. The resolver computes it
                // per provider precisely so a managed tenant cannot fall back to
                // plaintext, and this struct was where that decision used to die
                // — leaving `postgres_managed` at `prefer`, i.e. TLS only if the
                // server insists.
                //
                // `require` alone only ENCRYPTS: the connector maps it to
                // `verify_tls = false` (a no-op cert verifier), so an active MITM
                // could present any certificate and harvest the analyst password
                // and every row. When the tenant demands verification we hand the
                // connector `verify-full` instead, which checks the chain against
                // `webpki_roots` — Neon's public cert is covered. Local stays on
                // `disable` (loopback, no certificate). The DSN string keeps
                // `c.sslmode` (`require`/`disable`), the only spellings libpq's
                // parser accepts; this stronger mode lives only on the connector.
                Ok(c) => Some(ConnectorConfig::Postgres(PostgresConfig {
                    host: c.host,
                    port: c.port,
                    user: c.user,
                    password: c.password,
                    database: c.database,
                    sslmode: Some(if c.verify_tls {
                        "verify-full".to_string()
                    } else {
                        c.sslmode
                    }),
                })),
                Err(e) => {
                    tracing::warn!(
                        db = %db.name,
                        workspace_id = %workspace_manager.workspace_id,
                        "postgres_managed resolve failed: {e}"
                    );
                    None
                }
            }
        }

        DatabaseType::Redshift(rds) => {
            let host = rds
                .get_host(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_else(|_| "localhost".into());
            let port: u16 = rds
                .get_port(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_else(|_| "5439".into())
                .parse()
                .unwrap_or(5439);
            let user = rds
                .get_user(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_default();
            let password = rds
                .get_password(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_default();
            let database = rds
                .get_database(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_default();
            Some(ConnectorConfig::Redshift(PostgresConfig {
                host,
                port,
                user,
                password,
                database,
                sslmode: None,
            }))
        }

        DatabaseType::ClickHouse(ch) => {
            // Same fallbacks as before, but the reason for one is no longer
            // discarded — see [`or_default`].
            let sm = &workspace_manager.secrets_manager;
            let host = or_default(&db.name, "host", ch.get_host(sm).await, "localhost");
            let user = or_default(&db.name, "user", ch.get_user(sm).await, "");
            let password = or_default(&db.name, "password", ch.get_password(sm).await, "");
            let database = or_default(&db.name, "database", ch.get_database(sm).await, "");
            let url = if host.contains("://") {
                // Caller already provided a full URL (scheme + host + optional
                // port + optional path).  Pass it through verbatim — common for
                // ClickHouse Cloud (HTTPS, port 8443) and port-forwarded hosts
                // where the user enters the full address.
                host
            } else if host.contains(':') {
                // Bare host:port — synthesize the scheme only.
                format!("http://{host}")
            } else {
                // Bare hostname — fall back to the default ClickHouse HTTP port.
                format!("http://{host}:8123")
            };
            Some(ConnectorConfig::ClickHouse(ClickHouseConfig {
                url,
                user,
                password,
                database,
            }))
        }

        DatabaseType::Snowflake(sf) => {
            let auth = match &sf.auth_type {
                SnowflakeAuthType::BrowserAuth {
                    browser_timeout_secs,
                    cache_dir,
                } => SnowflakeAuth::Browser {
                    timeout_secs: *browser_timeout_secs,
                    cache_dir: cache_dir.clone(),
                    sso_url_callback: None,
                },
                SnowflakeAuthType::PrivateKey { .. } => {
                    tracing::warn!(
                        db = %db.name,
                        "Snowflake: private-key auth not yet supported in agentic connector"
                    );
                    return None;
                }
                _ => {
                    let password = match sf.get_password(&workspace_manager.secrets_manager).await {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!(db = %db.name, "Snowflake: cannot resolve password: {e}");
                            return None;
                        }
                    };
                    SnowflakeAuth::Password { password }
                }
            };
            Some(ConnectorConfig::Snowflake(SnowflakeConfig {
                account: sf.account.clone(),
                username: sf.username.clone(),
                auth,
                role: sf.role.clone(),
                warehouse: sf.warehouse.clone(),
                database: Some(sf.database.clone()),
                schema: sf.schema.clone(),
            }))
        }

        DatabaseType::Bigquery(bq) => {
            let key_path = match bq.get_key_path(&workspace_manager.secrets_manager).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(db = %db.name, "BigQuery: {e}");
                    return None;
                }
            };
            let key_path = workspace_manager
                .config_manager
                .try_resolve_file(&key_path)
                .await
                .unwrap_or(key_path);
            let project_id = extract_project_id_from_key(&key_path).unwrap_or_default();
            {
                // Merge legacy single `dataset` + multi `datasets` map keys.
                let mut datasets: Vec<String> = bq.datasets.keys().cloned().collect();
                if let Some(ref ds) = bq.dataset
                    && !datasets.contains(ds)
                {
                    datasets.push(ds.clone());
                }
                Some(ConnectorConfig::BigQuery(BigQueryConfig {
                    key_path,
                    project_id,
                    datasets,
                }))
            }
        }

        DatabaseType::MotherDuck(md) => {
            let token = match md.get_token(&workspace_manager.secrets_manager).await {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(db = %db.name, "MotherDuck token: {e}");
                    return None;
                }
            };
            let url = match &md.database {
                Some(db) => format!("md:{}?motherduck_token={}", db, token),
                None => format!("md:?motherduck_token={}", token),
            };
            Some(ConnectorConfig::DuckDbUrl(DuckDbUrlConfig { url }))
        }

        DatabaseType::Mysql(my) => {
            let host = my
                .get_host(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_else(|_| "localhost".into());
            let port: u16 = my
                .get_port(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_else(|_| "3306".into())
                .parse()
                .unwrap_or(3306);
            let user = my
                .get_user(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_default();
            let password = my
                .get_password(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_default();
            let database = my
                .get_database(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_default();
            Some(ConnectorConfig::Mysql(MysqlConfig {
                host,
                port,
                user,
                password,
                database,
            }))
        }

        DatabaseType::DOMO(d) => {
            let developer_token = match workspace_manager
                .secrets_manager
                .resolve_secret(&d.developer_token_var)
                .await
            {
                Ok(Some(t)) => t,
                Ok(None) => {
                    tracing::warn!(
                        db = %db.name,
                        var = %d.developer_token_var,
                        "DOMO developer-token secret not found"
                    );
                    return None;
                }
                Err(e) => {
                    tracing::warn!(db = %db.name, "DOMO developer-token resolution failed: {e}");
                    return None;
                }
            };
            Some(ConnectorConfig::Domo(DomoConfig::from_instance(
                &d.instance,
                developer_token,
                &d.dataset_id,
            )))
        }
    }
}

fn extract_project_id_from_key(key_path: &str) -> Option<String> {
    let contents = std::fs::read_to_string(key_path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&contents).ok()?;
    v.get("project_id")?.as_str().map(|s| s.to_string())
}

/// Pre-build an `airhouse::AirhouseConnector` for `airhouse` /
/// `airhouse_managed` databases. Errors are swallowed into a warn log and
/// returned as `None` because this is the agentic-pipeline path: a single
/// failing database shouldn't kill batch resolution. Non-agentic callers
/// (the SQL IDE, the builder bridge) use [`OxyProjectContext::build_connector_for`]
/// instead, which propagates the same errors so the user sees what's wrong.
async fn resolve_pre_built_airhouse(
    db_name: &str,
    workspace_manager: &WorkspaceManager<WorkingCopy>,
    subject: Option<uuid::Uuid>,
    role: Option<WorkspaceRole>,
) -> Option<Arc<dyn DatabaseConnector>> {
    let db = workspace_manager
        .config_manager
        .resolve_database(db_name)
        .ok()?;
    if !matches!(
        db.database_type,
        DatabaseType::Airhouse(_) | DatabaseType::AirhouseManaged(_)
    ) {
        return None;
    }
    match build_airhouse_connector(&db, workspace_manager, subject, role).await {
        Ok(conn) => Some(conn),
        Err(e) => {
            tracing::warn!(db = %db.name, "airhouse connector build failed: {e}");
            None
        }
    }
}

/// Error-propagating airhouse connector builder. Used by both
/// [`resolve_pre_built_airhouse`] (which discards the error) and
/// [`OxyProjectContext::build_connector_for`] (which surfaces it). Keeps
/// the per-`DatabaseType` arms in one place.
///
/// The actual `AirhouseConnector` is **reused across requests** via
/// [`crate::agentic_wiring::airhouse_pool`] keyed by logical identity, so the
/// per-request `OxyProjectContext` no longer opens a fresh pgwire connection
/// (and a fresh server-side DuckLake session) per warehouse query. On a warm
/// hit no credential is minted and no connection is opened; the closure below
/// runs only on a miss or dead-connection rebuild.
async fn build_airhouse_connector<S: DiskSlot>(
    db: &oxy::config::model::Database,
    workspace_manager: &WorkspaceManager<S>,
    subject: Option<uuid::Uuid>,
    role: Option<WorkspaceRole>,
) -> Result<Arc<dyn DatabaseConnector>, OxyError> {
    let is_managed = matches!(db.database_type, DatabaseType::AirhouseManaged(_));
    let key = airhouse_pool_key(
        is_managed,
        workspace_manager.workspace_id,
        subject,
        role.as_ref(),
        &db.name,
    );

    crate::agentic_wiring::airhouse_pool::get_or_build(key, || async {
        let (mut host, mut port, user, password, database) =
            airhouse_wire_params(db, workspace_manager, subject, role.clone()).await?;

        // Route query workloads — analytics agent, Data-App, SQL-IDE: everything
        // funneled through build_airhouse_connector — to the dedicated analytics
        // DP pool when one is configured, isolating heavy OLAP from the
        // latency-sensitive serving/ingest DP. airway ingest shares
        // `airhouse_wire_params` directly (not this path) and cameras has its own
        // client, so their writes stay on the main serving endpoint. Unset
        // (AIRHOUSE_ANALYTICS_WIRE_HOST) → keep the serving endpoint (no change).
        if let Some(ep) = airhouse::analytics_wire_endpoint()
            && (ep.host.as_str(), ep.port) != (host.as_str(), port) {
                tracing::debug!(
                    from = %format!("{host}:{port}"),
                    to = %format!("{}:{}", ep.host, ep.port),
                    "airhouse query connector routed to the analytics pool"
                );
                host = ep.host;
                port = ep.port;
            }

        let conn = airhouse::AirhouseConnector::new(&host, port, &user, &password, &database)
            .await
            .map_err(|e| {
                OxyError::DBError(format!(
                    "airhouse connector failed to connect to {host}:{port}/{database} as {user}: {e}"
                ))
            })?;
        Ok(Arc::new(conn))
    })
    .await
}

/// Logical pool identity for an airhouse connector. Deliberately keyed on the
/// **requester**, not the minted username: the broker re-mints a fresh
/// ephemeral username roughly every 15 min, but the live connection (and thus
/// this key) must persist across that rotation. `airhouse_managed` is per
/// `(workspace, subject|system, role)`; static airhouse shares fixed creds so
/// it keys per `(workspace, db)`.
fn airhouse_pool_key(
    is_managed: bool,
    workspace_id: uuid::Uuid,
    subject: Option<uuid::Uuid>,
    role: Option<&WorkspaceRole>,
    db_name: &str,
) -> String {
    if is_managed {
        let subj = subject
            .map(|u| u.to_string())
            .unwrap_or_else(|| "system".to_string());
        let r = role
            .map(|r| format!("{r:?}"))
            .unwrap_or_else(|| "none".to_string());
        format!("mgd:{workspace_id}:{subj}:{r}")
    } else {
        format!("static:{workspace_id}:{db_name}")
    }
}

/// Build a `postgresql://` URL, percent-encoding the userinfo and path
/// so credentials containing `@ : / ?` survive parsing by airway's
/// Postgres/airhouse destination.
fn pg_wire_dsn(user: &str, password: &str, host: &str, port: u16, database: &str) -> String {
    format!(
        "postgresql://{}:{}@{}:{}/{}",
        urlencoding::encode(user),
        urlencoding::encode(password),
        host,
        port,
        urlencoding::encode(database),
    )
}

/// Resolve the Postgres-wire connection parameters for an `airhouse` /
/// `airhouse_managed` database. For `airhouse_managed` this mints an
/// ephemeral per-subject credential via the SA broker. Shared by the
/// connector builder and the airway destination resolver.
async fn airhouse_wire_params<S>(
    db: &oxy::config::model::Database,
    workspace_manager: &WorkspaceManager<S>,
    subject: Option<uuid::Uuid>,
    role: Option<WorkspaceRole>,
) -> Result<(String, u16, String, String, String), OxyError> {
    let params = match &db.database_type {
        DatabaseType::Airhouse(ah) => {
            let host = ah
                .get_host(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_else(|_| "localhost".into());
            let port: u16 = ah
                .get_port(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_else(|_| "5445".into())
                .parse()
                .unwrap_or(5445);
            let user = ah
                .get_user(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_else(|_| "admin".into());
            let password = ah
                .get_password(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_else(|_| "airhouse".into());
            let database = ah
                .get_database(&workspace_manager.secrets_manager)
                .await
                .unwrap_or_else(|_| "airhouse".into());
            (host, port, user, password, database)
        }
        DatabaseType::AirhouseManaged(_) => {
            mint_airhouse_managed_creds(workspace_manager, subject, role).await?
        }
        other => {
            return Err(OxyError::ConfigurationError(format!(
                "build_airhouse_connector: unexpected database_type {other:?}"
            )));
        }
    };

    Ok(params)
}

/// Mint an ephemeral wire-protocol credential for an `airhouse_managed`
/// database via the SA-backed broker. Returns the
/// (host, port, username, password, dbname) tuple ready for the connector
/// constructor.
///
/// HTTP-handled paths arrive here with both `subject` (the requesting user
/// id, threaded by the workspace middleware) and `role` (the effective
/// workspace role) populated; the broker mints with the matching airhouse
/// role per the standard mapping. Background paths (scheduler, schema
/// crawl, agentic background runs) leave `subject = None` and fall through
/// to `mint_for_system` with a `system:workspace:<uuid>:agentic-bg`
/// audit subject.
async fn mint_airhouse_managed_creds<S>(
    workspace_manager: &WorkspaceManager<S>,
    subject: Option<uuid::Uuid>,
    role: Option<WorkspaceRole>,
) -> Result<(String, u16, String, String, String), OxyError> {
    let workspace_id = workspace_manager.workspace_id;
    let endpoint = airhouse::wire_endpoint().ok_or_else(|| {
        OxyError::ConfigurationError(
            "airhouse_managed: AIRHOUSE_WIRE_HOST is not configured; the airhouse \
             integration must be enabled (set AIRHOUSE_BASE_URL, AIRHOUSE_ADMIN_TOKEN, \
             AIRHOUSE_WIRE_HOST, AIRHOUSE_WIRE_PORT)"
                .into(),
        )
    })?;
    let broker = airhouse::token_broker().ok_or_else(|| {
        OxyError::ConfigurationError(
            "airhouse_managed: token broker not initialised; check the airhouse env \
             vars (AIRHOUSE_BASE_URL / AIRHOUSE_ADMIN_TOKEN / AIRHOUSE_WIRE_HOST)"
                .into(),
        )
    })?;

    let cred = match subject {
        Some(uid) => {
            let airhouse_role = role
                .map(airhouse::airhouse_role_for)
                // No effective role in scope (e.g. handler that didn't extract
                // EffectiveWorkspaceRole). Default to reader — least-privilege.
                .unwrap_or(airhouse::UserRole::Reader);
            broker
                .mint_for_user(
                    workspace_id,
                    uid,
                    airhouse_role,
                    airhouse::DEFAULT_INTERNAL_TTL,
                )
                .await
                .map_err(OxyError::from)?
        }
        None => broker
            .mint_for_system(
                workspace_id,
                airhouse::SystemPurpose::AgenticBackground,
                airhouse::UserRole::Admin,
                airhouse::DEFAULT_INTERNAL_TTL,
            )
            .await
            .map_err(OxyError::from)?,
    };

    Ok((
        endpoint.host,
        endpoint.port,
        cred.username,
        cred.password,
        cred.tenant,
    ))
}

// ── Model translation ───────────────────────────────────────────────────────

async fn resolve_model_impl(
    model_ref: Option<&str>,
    has_explicit_model: bool,
    workspace_manager: &WorkspaceManager<WorkingCopy>,
) -> Option<ResolvedModelInfo> {
    let name = if let Some(ref_name) = model_ref {
        ref_name
    } else if !has_explicit_model {
        workspace_manager.config_manager.default_model()?
    } else {
        return None;
    };

    match workspace_manager.config_manager.resolve_model(name) {
        Ok(model) => {
            let model_name = model.model_name().to_string();
            let key_var = model.key_var().map(|s| s.to_string());

            let (vendor, base_url, extra_api_key, azure_deployment_id, azure_api_version) =
                match model {
                    Model::Anthropic { config: m } => {
                        (LlmVendor::Anthropic, m.api_url.clone(), None, None, None)
                    }
                    Model::OpenAI { config: m } => {
                        let (dep_id, api_ver) = m
                            .azure
                            .as_ref()
                            .map(|a| {
                                (
                                    Some(a.azure_deployment_id.clone()),
                                    Some(a.azure_api_version.clone()),
                                )
                            })
                            .unwrap_or((None, None));
                        (LlmVendor::OpenAi, m.api_url.clone(), None, dep_id, api_ver)
                    }
                    Model::Ollama { config: m } => (
                        LlmVendor::OpenAiCompat,
                        Some(m.api_url.clone()),
                        Some(m.api_key.clone()),
                        None,
                        None,
                    ),
                    // Same Chat Completions wire protocol as Ollama, but with
                    // `extra_api_key: None` so resolution falls through to
                    // `key_var` below and picks up the managed workspace
                    // secret. Ollama's inline `api_key` short-circuits that,
                    // which is why it can't back a cloud-managed credential.
                    Model::OpenAICompat { config: m } => {
                        // Azure deployment routing is specific to Azure OpenAI
                        // and isn't applied here; the fields exist only because
                        // `OpenAIModelConfig` is shared with `vendor: openai`.
                        // Warn rather than silently no-op — the likely way to
                        // land here is copying an Azure model and flipping the
                        // vendor, where a silent drop looks like it worked.
                        if m.azure.is_some() {
                            tracing::warn!(
                                model = name,
                                "azure_deployment_id / azure_api_version are ignored on an \
                                 openai_compat model; use vendor: openai for Azure OpenAI"
                            );
                        }
                        (LlmVendor::OpenAiCompat, m.api_url.clone(), None, None, None)
                    }
                    Model::Google { .. } => {
                        tracing::warn!(
                            model = name,
                            "Google/Gemini models are not yet supported in analytics agents"
                        );
                        return None;
                    }
                };

            // Resolve api_key via secrets_manager first, env fallback. Ollama
            // carries its key inline via the config — honor that.
            let api_key = if let Some(inline) = extra_api_key {
                Some(inline)
            } else if let Some(kv) = key_var.as_deref() {
                workspace_manager
                    .secrets_manager
                    .resolve_secret(kv)
                    .await
                    .ok()
                    .flatten()
                    .or_else(|| std::env::var(kv).ok())
            } else {
                None
            };

            // Custom headers, resolved here rather than in the agentic crate:
            // a `{ env_var: … }` entry needs the secret store, which lives on
            // this side of the boundary. Gateways like Portkey and Helicone
            // authenticate on a header instead of the bearer token.
            let headers = match model.headers() {
                Some(map) if !map.is_empty() => {
                    let mut resolved = HashMap::with_capacity(map.len());
                    for (header_name, value) in map {
                        match value.resolve(&workspace_manager.secrets_manager).await {
                            Ok(v) => {
                                resolved.insert(header_name.clone(), v);
                            }
                            // Skip rather than fail the whole model: a missing
                            // header secret shouldn't make the agent
                            // unresolvable, and the gateway's own error is far
                            // more actionable than a config-load failure here.
                            Err(e) => tracing::warn!(
                                model = name,
                                header = %header_name,
                                "could not resolve header value, sending request without it: {e}"
                            ),
                        }
                    }
                    (!resolved.is_empty()).then_some(resolved)
                }
                _ => None,
            };

            Some(ResolvedModelInfo {
                model: model_name,
                vendor,
                api_key,
                base_url,
                azure_deployment_id,
                azure_api_version,
                headers,
            })
        }
        Err(e) => {
            tracing::warn!(model = name, "could not resolve model from config.yml: {e}");
            None
        }
    }
}

#[cfg(test)]
mod read_error_shape_tests {
    use agentic_automation::WorkspaceContext;

    /// The three shapes must be told apart by what failed, not by where the
    /// call sat.
    ///
    /// `resolve_automation_yaml` briefly labelled every error from
    /// `ConfigManager::definition` `Unavailable`, on the stated grounds that
    /// `definition` only fails when there is no working copy. It does not: it
    /// also propagates `definition_from_disk`, which returns
    /// `ConfigurationError` for unparseable YAML. So a typo in an automation
    /// opened in the IDE answered 503 with `Retry-After` — a retryable status
    /// for a permanent condition, and the loss of the only status that could
    /// say the file is broken.
    #[tokio::test]
    async fn a_typo_is_invalid_and_an_absent_source_is_unavailable() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
            .await
            .expect("config");
        tokio::fs::create_dir_all(dir.path().join("automations"))
            .await
            .expect("mkdir");
        tokio::fs::write(
            dir.path().join("automations/broken.automation.yml"),
            "tasks: [\n  - unclosed\n",
        )
        .await
        .expect("automation");

        let manager = oxy::adapters::workspace::builder::WorkspaceBuilder::new(uuid::Uuid::nil())
            .with_working_copy(dir.path(), None, oxy::config::OnMissing::Empty)
            .await
            .expect("builder")
            .build()
            .await
            .expect("workspace manager");

        let ctx = super::OxyProjectContext::new(manager);

        let err = ctx
            .resolve_automation_yaml("automations/broken.automation.yml")
            .await
            .expect_err("a file that does not parse is not content");
        assert!(
            err.is_invalid(),
            "a YAML typo is the caller's to fix — 422, never a retry: {err:?}"
        );
        assert!(!err.is_unavailable());

        let missing = ctx
            .resolve_automation_yaml("automations/nope.automation.yml")
            .await
            .expect_err("a file that is not there is not content either");
        assert!(
            !missing.is_unavailable() && !missing.is_invalid(),
            "an absent file on a node holding the workspace is a plain 404: {missing:?}"
        );
    }
}

#[cfg(test)]
mod semantic_scan_root_tests {
    use agentic_pipeline::platform::ProjectContext;

    /// A manager whose working-copy slot names `root`, stat-ed or not:
    /// `effective_workspace_path` returns the database column without checking
    /// it, so the slot is full whether or not the directory exists. Pass a
    /// missing path for a stateless replica, a real one for a node holding the
    /// files.
    async fn manager_at(
        root: &std::path::Path,
    ) -> oxy::adapters::workspace::manager::WorkspaceManager<oxy::config::WorkingCopy> {
        oxy::adapters::workspace::builder::WorkspaceBuilder::new(uuid::Uuid::nil())
            .with_working_copy(root, None, oxy::config::OnMissing::Empty)
            .await
            .expect("a manager builds from the database column, unstat-ed")
            .build()
            .await
            .expect("workspace manager")
    }

    /// The workspace-health eval builds its context on the global-run fleet and
    /// took its semantic model from `working_copy().root()` — a directory that
    /// node does not have. Poke House Staging read
    /// `failed to load semantic model: failed to read /workspace/oxy_data/
    /// workspaces/<id>/oxy: No such file or directory` for 58 days while every
    /// product surface queried the same model happily, because those resolve
    /// through the compile boundary and this did not.
    #[tokio::test]
    async fn the_system_runner_reads_the_scan_root_the_context_was_handed() {
        let parent = tempfile::tempdir().expect("tempdir");
        let absent = parent.path().join("never-cloned");

        // What `resolve_query_scan_source` materialises out of the compiled
        // rows: the same shape, somewhere this node can actually read.
        let scan = parent.path().join("scan");
        std::fs::create_dir_all(scan.join("semantics/views")).expect("scan dirs");
        std::fs::write(
            scan.join("semantics/views/orders.view.yml"),
            "name: orders\ndata_source: warehouse\ntable: orders\n",
        )
        .expect("view");

        assert!(!absent.is_dir(), "the node holds no working copy");

        let stateless = super::OxyProjectContext::new(manager_at(&absent).await);
        let err = stateless
            .metric_tree_runner_system()
            .expect("system runner")
            .load_layer()
            .await
            .expect_err("there is no layer to read on this node");
        let err = err.to_string();
        assert!(
            !err.contains("No such file or directory"),
            "a node with no files must name the missing SCAN ROOT, not report the \
             workspace directory as an I/O fault — that reads as a broken workspace \
             and sent operators to audit YAML for two months: {err}"
        );

        let boundary = super::OxyProjectContext::new(manager_at(&absent).await)
            .with_semantic_scan_path(scan.clone());
        let layer = boundary
            .metric_tree_runner_system()
            .expect("system runner")
            .load_layer()
            .await
            .expect("the compile-boundary scan is readable on any node");
        assert_eq!(
            layer.views.len(),
            1,
            "the layer must come from the scan root, not the absent working copy"
        );
    }

    /// Workspace health JUDGES a workspace; it does not serve it. So it reads
    /// the promoted revision and nothing else.
    ///
    /// The eval can land on a node that holds a working copy — a single
    /// instance, or the factory doubling as the global-run node — and that
    /// working copy holds whatever someone is editing in the IDE right now: a
    /// half-written view, a branch mid-refactor. Falling back to it means an
    /// uncommitted edit pages the on-call about a workspace whose promoted
    /// revision, and therefore everything actually being served, is fine.
    #[tokio::test]
    async fn a_health_context_refuses_the_working_copy_it_could_have_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("semantics/views")).expect("dirs");
        // The IDE edit: on disk, on this node, and broken enough that a probe
        // against it would report the workspace unhealthy.
        std::fs::write(
            dir.path().join("semantics/views/orders.view.yml"),
            "name: orders\ndata_source: warehouse\ntable: orders\n",
        )
        .expect("view");

        let manager = manager_at(dir.path()).await;
        assert!(
            manager
                .config_manager
                .working_copy()
                .is_some_and(|wc| wc.root().is_dir()),
            "this node really does hold the files — that is the trap"
        );

        let health = super::OxyProjectContext::new(manager).requiring_compiled_semantic_scan();
        let err = health
            .metric_tree_runner_system()
            .expect("system runner")
            .load_layer()
            .await
            .expect_err("a health context must not read the working copy")
            .to_string();
        assert!(
            err.contains("not compiled"),
            "and it must say WHY it read nothing, so the alert names the promoted \
             revision rather than the model: {err}"
        );
    }
}
