use agentic_semantic::refresh_key_cache::RefreshKeyCache;
use axum::extract::{FromRequestParts, Path};
use axum::extract::{Query, State};
use axum::http::request::Parts;
use axum::{
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use entity::workspace_members::WorkspaceRole;
use oxy::adapters::secrets::SecretsManager;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::adapters::workspace::effective_workspace_path;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::{ReadOnly, WorkingCopy};
use oxy::database::client::establish_connection;
use oxy::service::secret_manager::SecretManagerService;
use oxy_app_core::AppState;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::EntityTrait;
use std::future::Future;
use uuid::Uuid;

#[derive(Clone)]
pub struct WorkspaceManagerWorkingCopy(pub WorkspaceManager<WorkingCopy>);

/// The same workspace manager with the filesystem capability dropped.
///
/// A handler that only reads data the compile boundary carries takes this
/// instead, and the compiler then refuses `workspace_path()`, `resolve_state_dir()`
/// and the workspace-file walks. The point is not that the value differs — it is
/// the same manager — but that the handler's signature now states what it needs,
/// and a later edit that reaches for the disk fails to build instead of failing
/// on three pods out of four.
///
/// It reads its own extension rather than downgrading the `WorkingCopy` one, so that a
/// pod which stops publishing `WorkspaceManager<WorkingCopy>` keeps serving every handler
/// that never needed a disk.
pub struct WorkspaceManagerReadOnly(pub WorkspaceManager<ReadOnly>);

impl<S> FromRequestParts<S> for WorkspaceManagerReadOnly
where
    S: Send + Sync,
{
    type Rejection = WorkspaceManagerMissing;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let result = parts
            .extensions
            .get::<WorkspaceManager<ReadOnly>>()
            .cloned()
            .map(WorkspaceManagerReadOnly)
            .ok_or_else(|| WorkspaceManagerMissing {
                needs_recompile: parts
                    .extensions
                    .get::<NeedsRecompileMarker>()
                    .map(|m| m.workspace_id),
            });

        async move { result }
    }
}

/// Why the workspace manager wasn't attached to this request. The two
/// flavors carry different operator + UX semantics:
///
/// - `NotAvailable`: the workspace path / config.yml is unreachable. The
///   user needs to fix their setup or the operator needs to debug a real
///   FS problem.
/// - `NeedsRecompile`: a serve replica refused to fall through to the
///   workspace FS (which it doesn't have) because the compile boundary
///   couldn't produce a usable config. The middleware has already
///   lazy-enqueued a Compile TaskSpec; the FE should retry shortly.
///   Triggered by either an absent / stale revision, or a compile-blob
///   deserialise failure (e.g. workspace `5ce5c011` with a schema-
///   drifted `DuckDBOptions`). Response carries the
///   `X-Oxy-Needs-Recompile` header so the FE can distinguish this from
///   a generic 503.
pub struct WorkspaceManagerMissing {
    pub needs_recompile: Option<Uuid>,
}

impl IntoResponse for WorkspaceManagerMissing {
    fn into_response(self) -> Response {
        match self.needs_recompile {
            Some(workspace_id) => {
                let mut response = (
                    StatusCode::SERVICE_UNAVAILABLE,
                    axum::Json(serde_json::json!({
                        "error": "Workspace has no current compiled revision on this replica. A compile has been enqueued; please retry shortly.",
                        "workspace_id": workspace_id,
                        "needs_recompile": true,
                    })),
                )
                    .into_response();
                // Correlation only — the workspace_id, for an interceptor and
                // for the logs. Nothing in the frontend branches on it.
                if let Ok(value) = axum::http::HeaderValue::from_str(&workspace_id.to_string()) {
                    response
                        .headers_mut()
                        .insert("x-oxy-needs-recompile", value);
                }
                // What the frontend actually branches on. `shouldRetryWorkspaceQuery`
                // gives a materialising workspace 24 retries at `Retry-After`
                // (~2 minutes) behind a spinner, and everything else three.
                // Without these two headers this 503 took the three-retry path
                // and surfaced as a generic load failure — for a workspace whose
                // compile had already been enqueued one line above and would have
                // landed well inside the long leash.
                //
                // Same state as `workspaces::handlers`' producer from the caller's
                // side: the workspace's data is not ready on this instance yet,
                // and waiting is the correct response. That the ide is missing a
                // working copy and a replica is missing a revision is a difference
                // the frontend has no use for.
                response.headers_mut().insert(
                    oxy_app_core::pods::ide_proxy::HEADER_UNAVAILABLE,
                    axum::http::HeaderValue::from_static("workspace-materializing"),
                );
                response.headers_mut().insert(
                    axum::http::header::RETRY_AFTER,
                    axum::http::HeaderValue::from_static("5"),
                );
                response
            }
            None => (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(serde_json::json!({
                    "error": "Workspace configuration is not available. Check that the workspace path is accessible and config.yml is valid."
                })),
            )
                .into_response(),
        }
    }
}

/// Why a workspace request was refused, in a shape the caller can act on.
///
/// `Status` is the ordinary path — an opaque code, exactly as before.
/// `AssumeRequired` is the one denial that is a **policy boundary, not a fault**:
/// Oxy staff get the ability to *assume* a tenant role, never ambient read access
/// to tenant data (`api::admin::assume`; this is the silent override closed in
/// #2710). A bare 403 there reads as a bug and sends operators hunting through
/// releases, so it explains itself instead — the reason, the org to assume, and an
/// `x-oxy-assume-required` header the frontend keys off to offer the assume dialog
/// in place of a dead error.
pub enum WorkspaceAccessError {
    Status(StatusCode),
    AssumeRequired {
        workspace_id: Uuid,
        org_id: Uuid,
        /// Best-effort: the FE opens the assume dialog pre-scoped with it, and
        /// the message names the tenant instead of a UUID. `None` when the
        /// lookup failed — never a reason to turn a 403 into a 500.
        org_name: Option<String>,
    },
}

impl From<StatusCode> for WorkspaceAccessError {
    fn from(status: StatusCode) -> Self {
        Self::Status(status)
    }
}

impl IntoResponse for WorkspaceAccessError {
    fn into_response(self) -> Response {
        match self {
            Self::Status(status) => status.into_response(),
            Self::AssumeRequired {
                workspace_id,
                org_id,
                org_name,
            } => {
                let tenant = org_name
                    .clone()
                    .unwrap_or_else(|| "the owning org".to_string());
                let mut response = (
                    StatusCode::FORBIDDEN,
                    axum::Json(serde_json::json!({
                        "error": "workspace_access_denied",
                        "reason": "assume_role_required",
                        "message": format!(
                            "Oxy staff do not get ambient access to tenant data. Viewing this \
                             workspace requires an explicit assume-role session for {tenant} — \
                             time-boxed and audited."
                        ),
                        "workspace_id": workspace_id,
                        "org_id": org_id,
                        "org_name": org_name,
                    })),
                )
                    .into_response();
                // FE interceptor keys off this to swap the dead 403 for the
                // assume-role dialog, scoped to the org that must be assumed.
                if let Ok(value) = axum::http::HeaderValue::from_str(&org_id.to_string()) {
                    response
                        .headers_mut()
                        .insert("x-oxy-assume-required", value);
                }
                response
            }
        }
    }
}

/// Marker inserted by the workspace middleware when a serve replica
/// refuses to fall through to FS. The `WorkspaceManagerWorkingCopy`
/// promotes it to a structured rejection so handlers don't have to
/// know about role-aware short-circuiting.
#[derive(Clone)]
struct NeedsRecompileMarker {
    workspace_id: Uuid,
}

impl FromRequestParts<oxy_app_core::pods::IdeState> for WorkspaceManagerWorkingCopy {
    type Rejection = WorkspaceManagerMissing;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &oxy_app_core::pods::IdeState,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let result = parts
            .extensions
            .get::<WorkspaceManager<WorkingCopy>>()
            .cloned()
            .map(WorkspaceManagerWorkingCopy)
            .ok_or_else(|| {
                // A serve replica that short-circuited the FS fallback left
                // this marker behind; promote it to a structured rejection.
                let needs_recompile = parts
                    .extensions
                    .get::<NeedsRecompileMarker>()
                    .map(|m| m.workspace_id);
                WorkspaceManagerMissing { needs_recompile }
            });

        async move { result }
    }
}

/// `true` when the caller's workspace role came from the **global-operator
/// override** (a Global Owner / Global Admin who is not a real member of the org
/// gets a synthesized org-Owner membership) rather than a real membership.
///
/// Guards for tenant-sovereign decisions MUST reject this — otherwise an Oxy
/// operator can act as the customer. The Oxy-access lockdown switch is exactly
/// such a decision: staff must not be able to unlock themselves.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceGlobalOverride(pub bool);

impl<S> FromRequestParts<S> for WorkspaceGlobalOverride
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        // Absent (e.g. local mode) => not an override.
        let v = parts
            .extensions
            .get::<WorkspaceGlobalOverride>()
            .copied()
            .unwrap_or(WorkspaceGlobalOverride(false));
        async move { Ok(v) }
    }
}

// `EffectiveWorkspaceRole` and its extension-reading extractors moved to
// `oxy-server-authz` (state-agnostic authz context, consumed by the workspace
// role guards that also moved). The workspace middleware below still resolves
// and inserts it; re-exported here so the original
// `middlewares::workspace_context::EffectiveWorkspaceRole` path keeps resolving
// (oxy-server's tests import `oxy_server_authz::workspace_role` since decompose
// RPT, 2026-11-24, when the `oxy_app::api` alias went).
pub use oxy_server_authz::workspace_role::EffectiveWorkspaceRole;

/// Moved to `oxy-app-core` so the custom-app serve path, which sits above this
/// crate, can name it; the old path stays so no call site changes.
pub use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;

/// Per-workspace airlayer SemanticLayer cache. Attached by workspace_middleware
/// so handlers avoid re-parsing all `.view.yml`/`.topic.yml` files on every
/// request. Call `get_or_load` for a cached layer, `invalidate` after writes.
#[derive(Clone)]
pub struct SemanticLayerCacheCtx {
    pub cache: std::sync::Arc<oxy_app_core::workspace_cache::SemanticLayerCache>,
    pub workspace_id: Uuid,
    /// The engine cache, so a layer reload can drop the engines built from the
    /// layer it replaced.
    ///
    /// The two are not independent: the world-model handlers PLAN from the
    /// layer and COMPILE against the engine, so an engine that outlives its
    /// layer compiles a plan referencing members it has never seen — and those
    /// handlers map compile failures to `None`, which renders an empty panel
    /// rather than an error. Holding the handle here is what lets one reload
    /// speak for both.
    pub engine_cache: std::sync::Arc<oxy_app_core::workspace_cache::SemanticEngineCache>,
}

impl SemanticLayerCacheCtx {
    /// Returns a cached `Arc<oxy_airlayer_compat::SemanticLayer>`, loading from
    /// disk on the first call per (workspace, source) (or after `invalidate`).
    /// The load is offloaded to a blocking thread so it does not stall the Tokio
    /// worker pool.
    ///
    /// `source_revision` is the revision the scan actually READ — `None` for the
    /// working copy — the same `Option<Uuid>` the engine cache takes, so one
    /// value describes the layer and the engine built from it. It is NOT
    /// `config_manager.revision_id()`, which reports the pin and is `Some` even
    /// on a node serving its own working copy; get it from
    /// `QueryScanSource::source_revision()`, or pass `None` at a handler that
    /// scans `semantics_scan_path()` unconditionally.
    pub async fn get_or_load(
        &self,
        source_revision: Option<Uuid>,
        scan: oxy::config::ScanDir,
    ) -> Result<
        std::sync::Arc<oxy_airlayer_compat::SemanticLayer>,
        oxy_airlayer_compat::SemanticError,
    > {
        let key = oxy_airlayer_compat::LayerKey::for_source(self.workspace_id, source_revision);
        if let Some(layer) = self.cache.lookup(&key) {
            tracing::debug!(workspace_id = %self.workspace_id, source = ?key.source, "semantic model cache hit");
            return Ok(layer);
        }
        tracing::info!(workspace_id = %self.workspace_id, source = ?key.source, directory = ?scan.directory(), "semantic model cache miss — loading");
        let t0 = std::time::Instant::now();
        let layer = tokio::task::spawn_blocking(move || scan.load_layer())
            .await
            .map_err(|e| {
                oxy_airlayer_compat::SemanticError::Engine(format!("blocking task failed: {e}"))
            })??;
        tracing::info!(workspace_id = %self.workspace_id, source = ?key.source, elapsed_ms = t0.elapsed().as_millis(), "semantic model loaded");
        let arc_layer = std::sync::Arc::new(layer);
        self.cache.insert(key, arc_layer.clone());
        // This layer is new to the process, so every cached engine for THIS
        // SOURCE was built from the one it replaces. Dropping them here is what
        // keeps "plan from the layer, compile against the engine" honest: an
        // edit that lands out of band (the Builder Agent writes files without
        // going through `POST /files`) is otherwise invisible to the engine
        // until its own TTL lapses, and the world-model handlers report the
        // resulting compile failure as an empty panel.
        //
        // Scoped to `key.source`, not the workspace. Since the layer cache
        // gained the source in its own key, a working-copy reload no longer
        // replaces the layer a `Revision(R)` engine was built from — so a
        // workspace-wide flush here would evict live engines for no reason,
        // and would move the promote-window ping-pong `engine_cache` avoids on
        // insert onto the layer/engine edge instead. The out-of-band edit this
        // guards against is a working-copy edit, so a working-copy reload
        // retiring working-copy engines is the whole requirement.
        self.engine_cache
            .invalidate_source(self.workspace_id, key.source);
        Ok(arc_layer)
    }

    /// Evicts every source's layer for this workspace so the next `get_or_load`
    /// reloads from disk. The callers — a semantic file write, a branch switch,
    /// a pull — mutate the working copy, but they know only that the workspace
    /// changed, not which of its keys that invalidates.
    pub fn invalidate(&self) {
        self.cache.invalidate_workspace(self.workspace_id);
    }
}

impl<S> FromRequestParts<S> for SemanticLayerCacheCtx
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let result = parts
            .extensions
            .get::<SemanticLayerCacheCtx>()
            .cloned()
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR);
        async move { result }
    }
}

/// Cached compiled `SemanticEngine` for a workspace, injected by workspace middleware.
///
/// This is the request-scoped handle on the process-wide cache in
/// `oxy_airlayer_compat::engine_cache`.
///
/// It deliberately does NOT carry the request's pinned revision. An engine is
/// identified by the layer source the caller actually read, and the pin cannot
/// tell you that: a node holding a working copy is `Origin::Compiled` too, so
/// `ConfigManager::revision_id()` is `Some` for a working-copy reader as much
/// as for a revision reader. Callers pick [`Self::working_copy_key`] or
/// [`Self::scan_key`] according to what they read.
#[derive(Clone)]
pub struct SemanticEngineCacheCtx {
    pub cache: std::sync::Arc<oxy_app_core::workspace_cache::SemanticEngineCache>,
    pub workspace_id: Uuid,
}

impl SemanticEngineCacheCtx {
    /// Key for an engine built from this node's WORKING COPY.
    ///
    /// For handlers reading `config_manager.semantics_scan_path()`.
    ///
    /// The pinned revision is deliberately not consulted: it is `Some` on a
    /// node serving its own working copy, so keying by it would let a
    /// working-copy read and a revision read share one engine.
    pub fn working_copy_key(
        &self,
        databases: &[oxy_airlayer_compat::DatabaseConfig],
    ) -> oxy_airlayer_compat::EngineKey {
        oxy_airlayer_compat::EngineKey::working_copy(self.workspace_id, databases)
    }

    /// Key for a caller that resolved its scan through `oxy::config::scan`.
    ///
    /// `source_revision` is what that scan actually read — `Some` only when it
    /// materialised the compiled revision. Get it from
    /// [`crate::scan_source::QueryScanSource::source_revision`],
    /// never from `ConfigManager::revision_id()`.
    pub fn scan_key(
        &self,
        source_revision: Option<Uuid>,
        databases: &[oxy_airlayer_compat::DatabaseConfig],
    ) -> oxy_airlayer_compat::EngineKey {
        oxy_airlayer_compat::EngineKey::for_source(self.workspace_id, source_revision, databases)
    }

    /// Returns a cached engine, building it from `layer` + `databases` on the
    /// first call.
    ///
    /// The build runs on the blocking pool — it revalidates the layer and
    /// rebuilds the join graph. The engine is `Send + Sync`, so what comes back
    /// is a bare `Arc` that callers may compile against concurrently and hold
    /// across awaits; there is no guard to drop.
    pub async fn get_or_build(
        &self,
        key: oxy_airlayer_compat::EngineKey,
        layer: std::sync::Arc<oxy_airlayer_compat::SemanticLayer>,
        databases: Vec<oxy_airlayer_compat::DatabaseConfig>,
    ) -> Result<
        std::sync::Arc<oxy_airlayer_compat::SemanticEngine>,
        oxy_airlayer_compat::SemanticError,
    > {
        let workspace_id = self.workspace_id;
        let source = key.source;
        // Fast path: a hit costs one lock and no blocking-pool hop.
        if let Some(engine) = self.cache.lookup(&key) {
            return Ok(engine);
        }
        let cache = self.cache.clone();
        tracing::info!(%workspace_id, ?source, "semantic engine cache miss — building engine");
        let t0 = std::time::Instant::now();
        let result = tokio::task::spawn_blocking(move || {
            cache.get_or_build(key, || {
                oxy_airlayer_compat::build_engine((*layer).clone(), &databases)
            })
        })
        .await;
        // The error is RETURNED, not just logged. It is the airlayer validation
        // message for the workspace's own `.view.yml` — the one thing that tells
        // the person looking at an empty IDE panel what to fix — and a
        // handler-level "engine unavailable" throws it away.
        match result {
            Ok(Ok(engine)) => {
                tracing::info!(%workspace_id, ?source, elapsed_ms = t0.elapsed().as_millis(), "semantic engine built and cached");
                Ok(engine)
            }
            Ok(Err(e)) => {
                tracing::warn!(%workspace_id, ?source, error = %e, elapsed_ms = t0.elapsed().as_millis(), "semantic engine build failed");
                Err(e)
            }
            Err(e) => {
                tracing::warn!(%workspace_id, ?source, error = %e, "semantic engine build task failed");
                Err(oxy_airlayer_compat::SemanticError::Engine(format!(
                    "engine build task failed: {e}"
                )))
            }
        }
    }

    /// Evicts every cached engine for this workspace — all revisions, all
    /// dialect maps (call alongside `SemanticLayerCacheCtx::invalidate`).
    pub fn invalidate(&self) {
        self.cache.invalidate_workspace(self.workspace_id);
    }
}

impl<S> FromRequestParts<S> for SemanticEngineCacheCtx
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let result = parts
            .extensions
            .get::<SemanticEngineCacheCtx>()
            .cloned()
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR);
        async move { result }
    }
}

/// The caller's org membership, inserted by workspace_middleware when the workspace belongs to an org.
#[derive(Clone)]
pub struct OrgMembershipExtractor(pub entity::org_members::Model);

impl<S> FromRequestParts<S> for OrgMembershipExtractor
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let result = parts
            .extensions
            .get::<entity::org_members::Model>()
            .cloned()
            .map(OrgMembershipExtractor)
            .ok_or(StatusCode::FORBIDDEN);

        async move { result }
    }
}

#[derive(serde::Deserialize)]
pub struct WorkspacePath {
    pub workspace_id: Uuid,
}

#[derive(serde::Deserialize)]
pub struct BranchQuery {
    pub branch: Option<String>,
}

pub async fn workspace_middleware(
    State(app_state): State<AppState>,
    Path(WorkspacePath { workspace_id }): Path<WorkspacePath>,
    Query(query): Query<BranchQuery>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, WorkspaceAccessError> {
    if workspace_id == Uuid::nil() {
        tracing::warn!("Nil-UUID workspace path is not allowed");
        return Err(StatusCode::NOT_FOUND.into());
    }

    let agentic_db = app_state
        .agentic_state
        .as_ref()
        .map(|s| std::sync::Arc::new(s.db.clone()));

    // Authorize before choosing the revision: a preview pin depends on who is
    // asking. Reads no compiled data, so it needs no pin of its own.
    let workspace_row = authorize_workspace(workspace_id, &user, &mut request).await?;

    // The Factory's routes are for Oxy staff, the org's owners and admins, and the
    // members an operator granted this workspace to. A no-op for every route a customer
    // reaches — see `server::factory_access`.
    request = match super::factory_access::gate(request).await {
        Ok(request) => request,
        Err(refusal) => return Ok(refusal),
    };

    // A preview request (`x-oxy-preview-revision`) from staff, naming a ready
    // staging revision of this workspace, reads that revision. Every other
    // request — the IDE on the same branch included, which sends no header — is
    // untouched. See `server::previews::pin`.
    let (mut request, preview) =
        super::preview_pin::for_request(workspace_id, query.branch.as_deref(), request).await;
    // There is no working copy to read: the request is served from the
    // compiled revision whatever branch it names.
    let working_copy_branch: Option<String> = None;

    // Resolve the one revision this request reads — ONCE — and pin it for the
    // whole downstream (config resolution below + every compiled reader the
    // handler calls). Without this, each reader re-resolves `current_revision_id`
    // independently and a promotion landing mid-request yields a torn read.
    // A preview is pinned by id, so there is nothing to resolve.
    let preview_revision = preview.as_ref().map(|p| p.revision_id);
    let pinned_revision = match preview_revision {
        Some(_) => None,
        None => {
            oxy_app_core::compiled_reader::resolve_request_revision(
                workspace_id,
                working_copy_branch.as_deref(),
            )
            .await
        }
    };
    // Rollups are built from the promoted model, so a preview never gets the
    // cache that would let a spawned task answer from one either.
    let preagg_cache = match preview {
        Some(_) => None,
        None => app_state.preagg_cache,
    };

    // Boxed: the whole downstream request runs inside `serve`, and each pin
    // wrapper below holds its future by value — a debug build's poll frames
    // copy it at every layer, which overflowed a 2 MiB tokio worker serving
    // `/sql/query` in the custom-app canary.
    let serve = Box::pin(async move {
        // Read-only in a preview: refused before any handler work.
        if let Some(pin) = &preview {
            request = match super::preview_read_only::check(request, pin).await {
                Ok(request) => request,
                Err(refusal) => return Ok(refusal),
            };
        }
        match workspace_row {
            Some(workspace_row) => {
                try_attach_workspace_manager(
                    &workspace_row,
                    working_copy_branch.as_deref(),
                    workspace_id,
                    user.id,
                    preagg_cache,
                    app_state.preagg_renewal_threshold_secs,
                    agentic_db,
                    app_state.semantic_layer_cache,
                    app_state.semantic_engine_cache,
                    &mut request,
                )
                .await?;
            }
            None => {
                tracing::warn!(
                    "No workspace path available for workspace {}, continuing without workspace manager",
                    workspace_id
                );
            }
        }

        // ── Fail-safe fallback (oxygen-internal#2528 follow-up) ────────────
        // If this serve replica has no compiled config for the workspace,
        // `try_attach_workspace_manager` set `NeedsRecompileMarker` (and
        // lazy-enqueued a compile). Rather than 503 the request, forward it to
        // the ide node — it owns the working copy and serves the workspace from
        // disk — so the workspace stays AVAILABLE (one in-cluster hop, degraded)
        // until the compile lands and the fleet can serve it from Postgres. This
        // turns "uncompiled / mid-compile = outage" into "= slower", which is the
        // whole point of an HA fleet. Loop-guarded; a no-op without
        // OXY_IDE_UPSTREAM (local / single-node), where the marker stays a 503.
        if request.extensions().get::<NeedsRecompileMarker>().is_some()
            && let Some(upstream) = oxy_app_core::pods::ide_proxy::ide_upstream()
            && !oxy_app_core::pods::ide_proxy::already_forwarded(&request)
        {
            tracing::info!(
                workspace_id = %workspace_id,
                "serve replica: no compiled config — forwarding to ide node (fail-safe) \
                 while the lazy compile lands"
            );
            return Ok(forward_with_original_uri(upstream, request).await);
        }

        let mut response = next.run(request).await;
        if let Some(pin) = &preview {
            pin.stamp(&mut response);
        }
        Ok(response)
    });
    match preview_revision {
        // The staging pin, not the bare revision pin: inside it the staging
        // rules hold for everything the handler calls (no rollups, partitioned
        // result caches) — the same scope a custom-app draft build reads in.
        // Inside it, the preview-request scope: whatever the handler runs holds
        // its writes (`oxy_app_core::write_hold`), whatever route it came in on.
        Some(revision_id) => {
            oxy_app_core::staging_pin_scope::with_staging_pin(
                Some(revision_id),
                oxy_app_core::write_hold::scope(serve),
            )
            .await
        }
        None => oxy_app_core::compiled_reader::with_pinned_revision(pinned_revision, serve).await,
    }
}

/// Proxy `request` to the ide node from inside the `/api/{workspace_id}` nest.
///
/// axum has rewritten the URI here to the stripped remainder (`/agents`, not
/// `/api/{ws}/agents`), and `forward_to_ide` proxies `req.uri()` verbatim, so the
/// original full URI goes back first — else the ide receives the bare `/agents`
/// and serves the SPA fallback. (`enforce_role` has the same problem on the
/// `/external/api` surface, which layers it inside its nest; that one is
/// `enforce_role_external`.)
async fn forward_with_original_uri(
    upstream: &str,
    mut request: Request<axum::body::Body>,
) -> Response {
    if let Some(original) = request
        .extensions()
        .get::<axum::extract::OriginalUri>()
        .map(|o| o.0.clone())
    {
        *request.uri_mut() = original;
    }
    oxy_app_core::pods::ide_proxy::forward_to_ide(
        upstream,
        request,
        oxy_app_core::pods::ide_proxy::ForwardReason::FailSafe,
    )
    .await
}

/// The access half of [`workspace_middleware`] and nothing else: authorize the
/// caller for `{workspace_id}` and attach the workspace row, effective role and
/// org membership, so the role guards work. No revision is resolved, no
/// workspace manager is built, and `?branch=` means nothing here — for surfaces
/// (the previews API) that decide who may act on a workspace but read neither
/// its config nor its files.
pub async fn workspace_access_middleware(
    Path(WorkspacePath { workspace_id }): Path<WorkspacePath>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, WorkspaceAccessError> {
    if workspace_id == Uuid::nil() {
        return Err(StatusCode::NOT_FOUND.into());
    }
    authorize_workspace(workspace_id, &user, &mut request).await?;
    Ok(next.run(request).await)
}

/// Looks up the workspace, authorizes the caller, and inserts request extensions
/// (workspace row, effective role, org membership). Returns the workspace row
/// only when it has a configured path — i.e. when builder construction should follow.
///
/// `Ok(None)`: workspace has no configured path (builder construction skipped).
/// Fatal: DB unreachable (SERVICE_UNAVAILABLE), workspace not found (NOT_FOUND),
/// workspace with no `org_id` (FORBIDDEN), caller not in org (FORBIDDEN),
/// query errors (INTERNAL_SERVER_ERROR).
async fn authorize_workspace(
    workspace_id: Uuid,
    user: &oxy_auth::types::AuthenticatedUser,
    request: &mut Request<axum::body::Body>,
) -> Result<Option<entity::workspaces::Model>, WorkspaceAccessError> {
    use entity::prelude::Workspaces;

    // The user AND the credential the request arrived with: an API token's
    // grants decide below whether this workspace is reachable at all, and at
    // what ceiling. `/api` and `/external/api` both come through here.
    let caller = oxy_server_authz::Caller::of(
        user,
        request
            .extensions()
            .get::<oxy_auth::token::CredentialContext>(),
    );

    let db = establish_connection().await.map_err(|e| {
        tracing::error!(
            "Could not connect to DB to resolve workspace {}: {}",
            workspace_id,
            e
        );
        StatusCode::SERVICE_UNAVAILABLE
    })?;

    let workspace_row = Workspaces::find_by_id(workspace_id)
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query workspace {}: {}", workspace_id, e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or_else(|| {
            tracing::warn!("Workspace {} not found in DB", workspace_id);
            StatusCode::NOT_FOUND
        })?;

    // Every workspace must belong to an org.
    let org_id = workspace_row.org_id.ok_or_else(|| {
        tracing::warn!("Workspace {} has no org_id — access denied", workspace_id);
        StatusCode::FORBIDDEN
    })?;

    // Resolved BEFORE the row is attached: a token with no grant here leaves
    // nothing behind for a later layer to read.
    let (org_membership, effective_role, is_global_override) =
        resolve_effective_role(&db, workspace_id, org_id, &caller).await?;

    request.extensions_mut().insert(workspace_row.clone());

    request
        .extensions_mut()
        .insert(EffectiveWorkspaceRole(effective_role));
    // Whether the Owner role above is REAL or a synthesized operator override.
    // Surfaces that distinction to guards that must never accept an Oxy operator
    // acting as the tenant (e.g. the Oxy-access lockdown switch).
    request
        .extensions_mut()
        .insert(WorkspaceGlobalOverride(is_global_override));
    request.extensions_mut().insert(org_membership);

    if workspace_row.path.is_none() {
        tracing::warn!(
            "Workspace {} has no path configured — continuing without workspace manager",
            workspace_id
        );
        return Ok(None);
    }

    Ok(Some(workspace_row))
}

/// Resolve a caller's effective workspace role.
///
/// `pub(crate)` because the custom-app function path needs it too: that route
/// does not run behind this middleware, so it cannot read the role out of
/// request extensions and must ask directly.
///
/// **This is where an API token's grant is applied to a workspace** (design
/// §4.4, "cap the facts at the source"):
///
/// - a token with no grant covering this workspace answers **404**, exactly as
///   an unknown workspace does, so it cannot probe which workspaces exist;
/// - otherwise the role returned is `min(live role, the grant's ceiling)`, and
///   the org membership beside it is capped the same way. Every workspace guard
///   and the ~15 handlers that read `EffectiveWorkspaceRole` see the capped role.
///
/// A browser session and a legacy key have no ceiling: they resolve exactly as
/// before.
pub async fn resolve_effective_role(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    org_id: Uuid,
    caller: &oxy_server_authz::Caller,
) -> Result<(entity::org_members::Model, WorkspaceRole, bool), WorkspaceAccessError> {
    use entity::org_members::Column as OrgMemberCol;
    use entity::prelude::{OrgMembers, WorkspaceMembers};
    use entity::workspace_members::Column as WsMemberCol;
    use sea_orm::{ColumnTrait, QueryFilter};

    // A sandbox agent token holds no role in any workspace (sandbox agent
    // credential design §3.2). Its workspace grant exists so the custom-app
    // paths can find its apps, not to make it an admin here: without this it
    // would resolve as one wherever its minter is a member, on every workspace
    // route and in every handler that reads the role by hand.
    if caller.is_sandbox_agent() {
        return Err(StatusCode::NOT_FOUND.into());
    }
    let ceiling = caller
        .workspace_ceiling(org_id, workspace_id)
        .ok_or(StatusCode::NOT_FOUND)?;
    let user_id = caller.user_id;
    let user_email = caller.email();

    // A service account's standing is its own row, never `org_members` (design
    // §3.3): a Member or an Admin of its one org, which then derives the
    // workspace role below exactly as a person's org role does. Outside its org
    // it answers 404, like any workspace a token does not cover.
    let real_membership = if caller.is_service_account() {
        Some(
            caller
                .account_membership(org_id)
                .ok_or(StatusCode::NOT_FOUND)?,
        )
    } else {
        OrgMembers::find()
            .filter(OrgMemberCol::OrgId.eq(org_id))
            .filter(OrgMemberCol::UserId.eq(user_id))
            .one(db)
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to query org membership (org={}, user={}, workspace={}): {}",
                    org_id,
                    user_id,
                    workspace_id,
                    e
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?
    };

    // Mirror `org_middleware`'s global-operator override: a Global Owner
    // (OXY_OWNER) or Global Admin (`app_admins`) who is not a real member of
    // the org gets a synthesized Owner membership so they can open a tenant's
    // workspace for support/triage (e.g. the admin "Open /home" on a workspace
    // that granted Oxy access). Without this, `/{workspace_id}/details` 403s
    // even though the parallel `/orgs/{id}/*` routes already allow operators
    // through — the inconsistency that bounced operators out of granted
    // workspaces.
    let mut is_global_override = false;
    let org_membership = match real_membership {
        Some(m) => m,
        None => {
            // Staff alone is not enough — an explicit, live assume-role session for
            // THIS org is required (see `api::admin::assume`). Without it the
            // operator is a plain non-member. This closes the silent override that
            // (among other things) let staff self-grant the old Oxy-access toggle.
            //
            // Same two populations as `org_context`: staff, or a partner acting as
            // an assigned client with `develop_apps`. Building a client's app means
            // opening their workspace, so this is exactly where the data-plane
            // capability has to be honoured.
            use oxy_app_core::assume;
            // Capability is independent of an active session — `may_act_as` reads
            // platform standing / partner scope only. Compute it FIRST so the
            // refusal can be shaped for the population that can actually act on
            // it: a plain non-member must stay opaque (naming the org to anyone
            // who guesses a workspace id is disclosure), while staff and partners
            // get the explanation and the way through.
            //
            // Both reads take the CALLER: standing is what the credential carries,
            // and a session belongs to the credential that opened it — a
            // new-format token does not inherit the browser's.
            let authority = assume::may_act_as(db, caller, org_id).await;
            let live = authority.is_some() && assume::is_session_live(db, caller, org_id).await;

            if let (Some(authority), true) = (authority, live) {
                let now = Utc::now().into();
                let role = authority.org_role();
                let role_label = role.as_str();
                tracing::info!(
                    actor_email = %user_email,
                    org_id = %org_id,
                    workspace_id = %workspace_id,
                    ?authority,
                    role = %role_label,
                    "workspace_context: assume-role session active"
                );
                is_global_override = true;
                entity::org_members::Model {
                    id: Uuid::nil(),
                    org_id,
                    user_id,
                    role,
                    created_at: now,
                    updated_at: now,
                }
            } else if authority.is_none() {
                // A plain non-member. Nothing here is actionable for them and the
                // org's existence/name is not theirs to learn, so this stays the
                // opaque 403 it has always been.
                tracing::warn!(
                    actor = %user_id,
                    workspace_id = %workspace_id,
                    org_id = %org_id,
                    "workspace_context: denied — not a member of this org"
                );
                return Err(StatusCode::FORBIDDEN.into());
            } else {
                // Staff or an assigned partner, without a live session for this
                // org. They CAN act here, so the wall explains itself rather than
                // reading as a bug — see `WorkspaceAccessError::AssumeRequired`.
                tracing::warn!(
                    actor = %user_id,
                    workspace_id = %workspace_id,
                    org_id = %org_id,
                    "workspace_context: denied — has authority for this org but no \
                     live assume-role session"
                );
                // Name the tenant so the message and the assume dialog read
                // "Pokehouse", not a UUID. Denial-only path, so the extra read
                // costs nothing on the hot path — and a failed lookup just
                // degrades the wording.
                let org_name = entity::prelude::Organizations::find_by_id(org_id)
                    .one(db)
                    .await
                    .ok()
                    .flatten()
                    .map(|org| org.name);
                return Err(WorkspaceAccessError::AssumeRequired {
                    workspace_id,
                    org_id,
                    org_name,
                });
            }
        }
    };

    // A service account holds no per-workspace elevation: its org role is the
    // ceiling of anything it can be given.
    let ws_override = if caller.is_service_account() {
        None
    } else {
        WorkspaceMembers::find()
            .filter(WsMemberCol::WorkspaceId.eq(workspace_id))
            .filter(WsMemberCol::UserId.eq(user_id))
            .one(db)
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to query workspace member override (workspace={}, user={}): {}",
                    workspace_id,
                    user_id,
                    e
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?
    };

    let org_derived_role = match org_membership.role {
        entity::org_members::OrgRole::Owner => WorkspaceRole::Owner,
        entity::org_members::OrgRole::Admin => WorkspaceRole::Admin,
        entity::org_members::OrgRole::Member => WorkspaceRole::Member,
    };

    // Workspace-member override can only elevate, never downgrade below org-derived role.
    let effective_role = match ws_override {
        Some(ws_member) => std::cmp::max(org_derived_role, ws_member.role),
        None => org_derived_role,
    };

    // The token's ceiling, applied last so it caps whatever the role came from:
    // the org role, a workspace elevation, or an assumed one.
    let effective_role = oxy_server_authz::cap_workspace_role(effective_role, ceiling);
    let org_membership = entity::org_members::Model {
        role: oxy_server_authz::cap_org_role(org_membership.role, ceiling),
        ..org_membership
    };

    Ok((org_membership, effective_role, is_global_override))
}

/// The lazy and import compile enqueue moved to `compile_reconcile::enqueue`
/// (2026-11-21); the old path stays so no caller changes.
pub use oxy_compile_runtime::reconcile::enqueue::{enqueue_import_compile, enqueue_lazy_compile};

/// The status for a `?branch=` value the workspace middleware could not
/// resolve. A well-formed detached label that no longer matches the working
/// copy is a conflict with the workspace's state (the checkout moved since the
/// caller read it), not a malformed request; everything else stays a `400`.
/// (In `api::workspaces::detached` until 2026-11-16; its one caller is here.)
fn unresolved_branch_status(branch: Option<&str>) -> StatusCode {
    match branch.map(str::trim) {
        Some(b) if oxy_git::is_detached_label(b) => StatusCode::CONFLICT,
        _ => StatusCode::BAD_REQUEST,
    }
}

/// Best-effort: builds the `WorkspaceManager` (with secrets, runs, intent classifier)
/// and inserts it into request extensions. The only fatal outcome is an invalid
/// branch query parameter, which yields BAD_REQUEST.
async fn try_attach_workspace_manager(
    workspace_row: &entity::workspaces::Model,
    branch_name: Option<&str>,
    workspace_id: Uuid,
    user_id: Uuid,
    preagg_cache: Option<std::sync::Arc<std::sync::RwLock<RefreshKeyCache>>>,
    preagg_renewal_threshold_secs: Option<u64>,
    db: Option<std::sync::Arc<sea_orm::DatabaseConnection>>,
    semantic_layer_cache: std::sync::Arc<oxy_app_core::workspace_cache::SemanticLayerCache>,
    semantic_engine_cache: std::sync::Arc<oxy_app_core::workspace_cache::SemanticEngineCache>,
    request: &mut Request<axum::body::Body>,
) -> Result<(), StatusCode> {
    // Branch name is validated inside `effective_workspace_path`. The helper
    // rejects ".." / leading "-" / non-allowed chars via OxyError::RuntimeError —
    // we map that to 400 before the string reaches any shell-out downstream.
    // The one value that is not a branch name, the detached-HEAD label, is
    // resolved there too (to the root working copy); a label the working copy
    // no longer matches answers 409 rather than 400.
    let effective_path = effective_workspace_path(workspace_row, branch_name)
        .await
        .map_err(|e| {
            tracing::warn!(
                "Invalid branch or missing path for workspace {}: {}",
                workspace_id,
                e
            );
            unresolved_branch_status(branch_name)
        })?;

    // Compile-boundary fast path: when the workspace has a promoted revision,
    // hydrate the workspace `Config` from `workspace_compiled_configs` instead
    // of re-parsing `config.yml` from disk on every request. This is the
    // largest single FS hit on the customer hot path — every chat / thread /
    // data app / automation request reads config.yml today.
    //
    // Thread the active branch through so the IDE on a feature branch sees its
    // working-copy `config.yml` edits via FS, matching the branch-aware
    // contract every other compiled reader honours. On any miss (no promoted
    // revision, non-default branch, deserialise fails) we fall through to FS.
    let pinned_revision: Option<uuid::Uuid> =
        oxy_app_core::compiled_reader::resolve_request_revision(workspace_id, branch_name).await;

    // ── Stateless-fleet short-circuit ──────────────────────────────────────
    // A serve replica has no workspace working copy (emptyDir OXY_STATE_DIR,
    // no PVC). If the compile boundary couldn't produce a config (no
    // current_revision_id, schema-drift deserialise failure, DB hiccup),
    // we must NOT fall through to the FS path below — the FS read would
    // either return stale data from a different workspace or, more likely,
    // 500 with "workspace data directory not found". Either is worse than
    // a structured 503 that tells the FE to retry after the compile lands.
    //
    // We still lazy-enqueue a Compile TaskSpec on the way out so the next
    // request can succeed without operator action. The
    // `WorkspaceManagerWorkingCopy` promotes `NeedsRecompileMarker` to a 503
    // with `X-Oxy-Needs-Recompile: <workspace_id>` so the FE can render a
    // proper "retrying compile" toast instead of a generic platform error —
    // UNLESS `OXY_IDE_UPSTREAM` is set (the serve fleet), in which case
    // `workspace_middleware` intercepts this marker and forwards the request to
    // the ide node (which serves from disk) instead of 503ing. The 503 is the
    // local / single-node fallback when there's no ide upstream to forward to.
    //
    // Regression context: oxy-hq/oxygen-internal#1619.
    if pinned_revision.is_none()
        && oxy_app_core::pods::role_manifest::current_process_role()
            == oxy_app_core::pods::role_manifest::Role::Serve
    {
        // Lazy self-heal: enqueue a (deduped) compile whenever the boundary
        // couldn't produce a usable config for a compilable workspace —
        // NOT only when `current_revision_id` is null. A workspace that HAS
        // a revision whose compiled config won't deserialise (a schema
        // drift, e.g. a stale `DuckDBOptions` shape) also lands here, and it
        // needs a recompile just as much as a never-compiled one. Gating on
        // `current_revision_id.is_none()` left drifted workspaces 503ing
        // forever with no recovery. `path.is_some()` is the real
        // precondition (a pathless workspace can't be compiled).
        if workspace_row.path.is_some()
            && let Some(agentic_db) = db.as_ref()
        {
            enqueue_lazy_compile(agentic_db, workspace_id).await;
        }
        request
            .extensions_mut()
            .insert(NeedsRecompileMarker { workspace_id });
        tracing::warn!(
            workspace_id = %workspace_id,
            "serve replica: compile boundary missed; refusing FS fallback (NeedsRecompile)"
        );
        return Ok(());
    }

    let builder_init = WorkspaceBuilder::new(workspace_id)
        .with_working_copy(
            &effective_path,
            pinned_revision,
            oxy::config::OnMissing::Empty,
        )
        .await;
    let mut builder = match builder_init {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                "Failed to set workspace path in workspace builder for workspace {}: {}, continuing without workspace manager",
                workspace_id,
                e
            );
            // Lazy self-heal for a workspace that has never been compiled and
            // whose FS config build also failed. NOTE: this path is only
            // reached by Ide/All/Worker — a serve replica already
            // short-circuited above (the `Role::Serve` block) with the WIDER
            // `path.is_some()` gate that also covers schema drift. This
            // narrower `current_revision_id.is_none()` gate is correct HERE
            // (these roles have a working copy, so a drifted-but-promoted
            // workspace still serves from the FS) but is deliberately
            // different from the serve-mode gate above — don't "align" one
            // without re-checking the other.
            if workspace_row.current_revision_id.is_none()
                && workspace_row.path.is_some()
                && let Some(agentic_db) = db.as_ref()
            {
                enqueue_lazy_compile(agentic_db, workspace_id).await;
            }
            return Ok(());
        }
    };

    // DB-first with env fallback so DB secrets hot-reload and override env vars
    // without a server restart.
    match SecretsManager::from_database_with_env_fallback(SecretManagerService::new(workspace_id)) {
        Ok(secrets_manager) => builder = builder.with_secrets_manager(secrets_manager),
        Err(_) => tracing::warn!(
            "Failed to create secrets manager for workspace {}, continuing without it",
            workspace_id
        ),
    }

    let workspace_manager = match builder.build().await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(
                "Failed to build workspace manager for workspace {}: {}, continuing without it",
                workspace_id,
                e
            );
            return Ok(());
        }
    };

    let mut ctx =
        crate::project_ctx::OxyProjectContext::new(workspace_manager.clone()).with_subject(user_id);
    // The effective role was inserted into extensions by `authorize_workspace`
    // a few lines up; thread it through so the airhouse_managed builder can
    // mint with the user's mapped airhouse role.
    if let Some(EffectiveWorkspaceRole(role)) = request.extensions().get::<EffectiveWorkspaceRole>()
    {
        ctx = ctx.with_role(role.clone());
    }
    // Share the same cache Arc with the background preagg worker so Layer 1
    // (per-query freshness) and Layer 2 (background rebuild) use the same state.
    if let Some(cache) = preagg_cache.clone() {
        ctx = ctx.with_preagg_cache(cache);
    }
    // Resolve the renewal threshold from THIS workspace's own
    // `pre_aggregations.refresh_worker.renewal_threshold`, falling back to the
    // process-wide value and then to the shared default. The process-wide
    // value has been `None` since the startup-bound worker was removed (see
    // `router::entry`), so before this a workspace configuring `10m` still got
    // 120s on every query while the rebuild cycle — which resolves the same
    // key from the same config — honoured it. The read side and the build side
    // now read one setting.
    let preagg_renewal_threshold_secs = Some(preagg_renewal_threshold_secs.unwrap_or_else(|| {
        oxy::config::preagg_check::resolve_renewal_threshold(
            workspace_manager
                .config_manager
                .get_config()
                .pre_aggregations
                .as_ref(),
        )
        .as_secs()
    }));
    if let Some(secs) = preagg_renewal_threshold_secs {
        ctx = ctx.with_preagg_renewal_threshold_secs(secs);
    }
    if let Some(db) = db {
        ctx = ctx.with_db(db);
    }
    // Same cache the handlers get, so an automation step and a `/semantic`
    // request reading the same source share one compiled engine.
    ctx = ctx.with_semantic_engine_cache(semantic_engine_cache.clone());
    // Also expose the cache + threshold directly to handlers via a typed
    // extension so endpoints like POST /semantic can resolve preagg without
    // routing through OxyProjectContext.
    request.extensions_mut().insert(PreaggCacheCtx {
        cache: preagg_cache,
        renewal_threshold_secs: preagg_renewal_threshold_secs,
    });
    request.extensions_mut().insert(SemanticLayerCacheCtx {
        cache: semantic_layer_cache,
        workspace_id,
        engine_cache: semantic_engine_cache.clone(),
    });
    request.extensions_mut().insert(SemanticEngineCacheCtx {
        cache: semantic_engine_cache,
        workspace_id,
    });
    let project_ctx = std::sync::Arc::new(ctx);
    let platform: std::sync::Arc<dyn agentic_pipeline::platform::PlatformContext> =
        project_ctx.clone();
    request
        .extensions_mut()
        .insert(workspace_manager.clone().into_read_only());
    // The `WorkingCopy` extension only exists on a process that owns the files.
    // Without this the extension was published everywhere, so a MISCLASSIFIED
    // route — one declared FleetOk whose handler requires a disk — received a
    // manager rooted at a directory that is not on this node, and failed deep
    // in the call stack with an IO error naming a path the caller never wrote.
    // Absent, `WorkspaceManagerWorkingCopy` rejects at the door instead.
    //
    // This is a backstop, not a fix: `role_middleware` classifies and proxies
    // an `IdeOnly` route to the ide BEFORE the handler runs, so a correctly
    // classified handler never reaches here on a replica. Three test guards
    // already catch the misclassification
    // (`a_handler_that_asks_for_a_working_copy_is_on_an_ide_only_route`,
    // `the_optional_working_copy_door_stays_shut`,
    // `undeclared_mounts_stay_diskless`). This is what happens when one slips
    // past all three.
    //
    // The read-only extension is NOT gated the same way. The flag is a
    // declaration made at startup; the filesystem is the fact, and `disk()`
    // checks `is_dir()` on the way through. A process that declares no files
    // but has them keeps serving its fallbacks, which is the safer direction
    // for a flag that could be set wrong.
    if oxy::workspace_fs_probe::process_owns_workspace_files() {
        request.extensions_mut().insert(workspace_manager);
    }
    request.extensions_mut().insert(platform);
    request.extensions_mut().insert(project_ctx);
    Ok(())
}

#[cfg(test)]
#[path = "workspace_context_tests.rs"]
mod tests;
