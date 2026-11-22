//! `ProjectContext` implementation backed by Oxy's [`WorkspaceManager`].
//!
//! All `oxy::*` imports for connector/model resolution live in this file.
//! This is the concrete adapter — the agentic stack sees only the trait.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use agentic_analytics::config::{LlmVendor, ResolvedModelInfo};
use agentic_connector::DatabaseConnector;
use agentic_pipeline::platform::ProjectContext;
use entity::workspace_members::WorkspaceRole;
// Turns a `{ env_var: … }` header reference into a literal, the one rule every
// connector header resolves through.
use oxy::adapters::header_value::HeaderValueExt;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::WorkingCopy;
use oxy::config::model::Model;
use oxy_shared::errors::OxyError;

// The three big trait impls live in sibling child modules; as `impl` blocks
// they need no re-export — the trait impls are visible wherever the trait and
// `OxyProjectContext` are in scope.
mod agent_lookup;
mod compile_cooldown;
mod context_documents;
mod context_root;
mod function_context;
mod monitor_scan;
mod preview_hold;
mod project_context;
mod react_flag;
pub(crate) mod revision_rebuild;
mod workspace_context;

use oxy_workspace_runtime::connector::CredentialUse;
use oxy_workspace_runtime::connector::{
    airhouse_wire_params, build_unheld_connector, env_fallback, pg_wire_dsn,
    resolve_connector_impl, resolve_pre_built_airhouse,
};
// The connector half, `oxy-workspace-runtime`'s `connector` since 2026-11-22
// (W1b): re-exported at its old paths.
pub use oxy_workspace_runtime::connector::{
    build_connector_for_db, database_to_connector_config, try_database_to_connector_config,
};

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
    semantic_scan: Option<Arc<crate::agentic_wiring::scan_source::QueryScanSource>>,
    /// Whether this context may fall back to the node's working copy when no
    /// scan was resolved. `true` for a reader that JUDGES the workspace rather
    /// than serving it — workspace health — where the working copy is whatever
    /// someone is editing in the IDE right now and an uncommitted edit must
    /// never page the on-call about a promoted revision that is fine.
    semantic_scan_required: bool,
    /// Built for a held request (`oxy_app_core::write_hold`) — a workspace
    /// preview, or a custom app's staging ask: every write it would make is
    /// held. Captured at construction, with the hold's app and sink, so a run
    /// driven on a spawned task keeps the answer the request had. See
    /// [`preview_hold`].
    hold: Option<oxy_app_core::write_hold::HoldScope>,
    /// Build each DuckDB directory connector's own database instead of
    /// cloning the process's cached one. See [`Self::with_own_directory_databases`].
    own_directory_databases: bool,
    /// How to rebuild this context at the workspace's current promoted
    /// revision. Set only on the background driver's cached contexts (see
    /// [`revision_rebuild`]); `None` everywhere else, so an HTTP or preview
    /// context never re-resolves its revision mid-request.
    revision_rebuilder: Option<Arc<dyn revision_rebuild::RevisionRebuilder>>,
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
            hold: oxy_app_core::write_hold::current(),
            own_directory_databases: false,
            revision_rebuilder: None,
        }
    }

    /// Give every DuckDB directory connector this context builds a database
    /// of its own, as before the directory cache. For a caller whose first
    /// statement writes — a pre-aggregation cycle — a cached clone buys
    /// nothing: the write moves it onto a private copy built the same way,
    /// after a cold cache has made it wait for the shared build first.
    pub fn with_own_directory_databases(mut self) -> Self {
        self.own_directory_databases = true;
        self
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
    /// (`scan_source::resolve_query_scan_source`).
    ///
    /// The `QueryScanSource` is held for the life of the context because a
    /// materialised scan is a tempdir that vanishes when its guard drops.
    pub(crate) fn with_semantic_scan(
        mut self,
        scan: crate::agentic_wiring::scan_source::QueryScanSource,
    ) -> Self {
        self.semantic_scan = Some(Arc::new(scan));
        self
    }

    /// [`Self::with_semantic_scan`] for a root that is already on disk.
    pub(crate) fn with_semantic_scan_path(self, scan_path: PathBuf) -> Self {
        self.with_semantic_scan(crate::agentic_wiring::scan_source::QueryScanSource::at(
            scan_path,
        ))
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
            (Some(scan), _) => ScanRoot::Compiled(scan.scan.clone()),
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
    ///
    /// Held when this context was built for a workspace-preview request.
    pub async fn build_connector_for(
        &self,
        db_name: &str,
    ) -> Result<Arc<dyn DatabaseConnector>, OxyError> {
        let conn = build_unheld_connector(
            &self.workspace_manager,
            db_name,
            self.subject,
            self.role.clone(),
        )
        .await?;
        Ok(self.held(conn, db_name))
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
            if self.holds_writes() {
                // Held, and with the real reason on failure: under a preview
                // there is no bare config to fall back to.
                return self
                    .build_connector_for(name)
                    .await
                    .map_err(|e| format!("failed to build connector for '{name}': {e}"));
            }
            if let Some(conn) = self.resolve_pre_built_connector(name).await {
                return Ok(conn);
            }
            // The reason, not a `None`: a `*_var` that names no secret used to
            // reach the caller as "not found in workspace config", which sends
            // the reader to `config.yml` when the fix is a missing secret.
            let db = self
                .workspace_manager
                .config_manager
                .resolve_database(name)
                .map_err(|_| format!("database '{name}' not found in workspace config"))?;
            let cfg = try_database_to_connector_config(&db, &self.workspace_manager)
                .await
                .map_err(|e| format!("failed to build connector for '{name}': {e}"))?
                .ok_or_else(|| {
                    format!(
                        "could not build connector config for database '{name}' (type {})",
                        db.database_type_name()
                    )
                })?;
            let cfg = if self.own_directory_databases {
                cfg.without_cached_directory()
            } else {
                cfg
            };
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

/// Cheap syntactic reject for obviously-unsafe `workflow_ref`s. The
/// canonicalisation-based check in `validate_path_within_project` would
/// catch absolute paths and `..` traversal too, but its error messages
/// vary by OS and by whether the path happens to exist; failing here
/// keeps the diagnostics stable and avoids touching the filesystem at all
/// for the easy cases.
pub(super) fn validate_automation_ref_syntax(workflow_ref: &str) -> Result<(), String> {
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
    use super::validate_automation_ref_syntax;

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
                    .or_else(|| env_fallback(&workspace_manager.secrets_manager, kv))
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
