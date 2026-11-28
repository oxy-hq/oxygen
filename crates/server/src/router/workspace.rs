//! The per-workspace route tree. Mounted under `/{workspace_id}` in both
//! cloud and local modes (local always uses the nil UUID).
//!
//! This module owns the tree shape plus the per-resource sub-builders still
//! in-tree. The workspace's databases, secrets and integrations (the Looker
//! listing and the OAuth connects) are `oxy-api-workspace-config`'s, and its
//! threads, agents and run artifacts `oxy-api-threads`', merged in through
//! the workspace seam.

use std::sync::Arc;

use axum::routing::post;

use agentic_http::{AgenticState, airway_router, automation_router, router as agentic_router};

use super::local_setup;

use oxy_shared::fleet_role::RouteRole;

use super::AppState;
use super::role_router::RoleRouter;

pub(super) fn build_workspace_routes(
    app_state: AppState,
    agentic_state: Arc<AgenticState>,
    include_local_setup: bool,
) -> RoleRouter {
    let mut router = RoleRouter::new(app_state.clone())
        // `/meta` (the workspace's metadata, the Postgres half of the retired
        // `/details`) is `oxy-api-workspaces`' since 2026-11-16, merged inside
        // this nest by `oxy-server` through the workspace seam, still FleetOk.
        // Camera fleet operator endpoints (sites / cameras / edge-boxes /
        // UniFi integration). Mounted here so `workspace_middleware`
        // (cloud) / `local_context_middleware` (local) gate them, and
        // the URL's `workspace_id` flows into the service layer for
        // resource-ownership checks. The edge-facing `/control/*` tree
        // is mounted separately at the app root (bearer auth resolves
        // workspace_id from the device token).
        .merge_undeclared(
            oxy_cameras::routes::workspace_routes::<AppState>(agentic_state.db.clone()),
            "oxy-cameras owns these paths; every one reads Postgres or S3",
        )
        // Legacy `/workflows` and `/automations` routes have been retired.
        // Workflow execution + the single-file fetch live under
        // `/agentic-workflows` (IdeOnly), mounted below. The automation LIST
        // and single-automation YAML (`/procedures`, `/automations` and their
        // `/{path_b64}`), served from the compile boundary (FleetOk), are
        // `oxy-api-automations`' since 2026-11-15, merged inside this nest by
        // `oxy-server` through `SurfaceSeams::workspace`.
        // `/airway-pipelines` (the boundary-backed pipeline listing and the
        // sp_api marketplace list) moved to `oxy-api-airway`, merged inside this
        // nest through the workspace seam, FleetOk as before.
        // `/threads` and `/agents` (the thread routes and the agent listing)
        // are `oxy-api-threads`' since 2026-11-16, merged inside this nest by
        // `oxy-server` through the workspace seam, still FleetOk.
        // `/api-tokens` (the tokens that can reach this workspace) moved to
        // `oxy-api-identity`, merged inside this nest through the workspace
        // seam, FleetOk as before. `/api-keys` was beside it until legacy API
        // keys were removed.
        // The staging branch compile (`/compile/staging` and its status, for a
        // custom-app draft build to pin) is `oxy-api-previews`' since
        // 2026-11-16, merged inside this nest by `oxy-server` through the
        // workspace seam, still FleetOk.
        // `/databases` and `/secrets` (Workspace Settings' Databases and
        // Secrets, each handler `WorkspaceAdmin`-guarded where it writes) are
        // `oxy-api-workspace-config`'s since 2026-11-15, merged inside this
        // nest by `oxy-server` through `SurfaceSeams::workspace`, still FleetOk.
        // `/integrations/*` (the Looker listing since 2026-11-16, and the
        // OAuth connects `/integrations/quickbooks/authorize` and
        // `/integrations/oauth/{provider}/authorize` since 2026-11-21, each
        // `WorkspaceAdmin`-guarded in its signature) is
        // `oxy-api-workspace-config`'s, merged inside this nest by `oxy-server`
        // through `SurfaceSeams::workspace`, still FleetOk.
        // `/members`, `/members/{user_id}` (a workspace's members and their
        // role overrides) and `/oxy-access` (the Oxy-staff lockdown) are
        // `oxy-api-workspaces`' since 2026-11-16, merged inside this nest
        // through the workspace seam, still FleetOk.
        // `/org-subdomain` (the org subdomain's read-only status) and `/logo`
        // (the org's uploaded logo) are `oxy-api-tenancy`'s since 2026-11-16,
        // merged inside this nest by `oxy-server` through the workspace seam,
        // still FleetOk.
        // `/source-uploads` moved to the `oxy-api-airway` sibling crate,
        // merged inside this nest by `oxy-server` through the workspace seam.
        // `/app-integrations` (the app integrations `config.yml` declares) is
        // `oxy-api-workspaces`' since 2026-11-16, merged inside this nest
        // through the workspace seam, still FleetOk.
        // `/traces`, `/metrics` and `/execution-analytics` (the tenant-facing
        // observability console) moved to the `oxy-api-observability` sibling
        // crate, merged inside this nest by `oxy-server` through the workspace
        // seam, still FleetOk.
        // `/artifacts/{id}`, `/charts/{file_path}` and `/logs` are
        // `oxy-api-threads`' since 2026-11-16, merged inside this nest through
        // the workspace seam, still FleetOk. (`/builder-availability` was
        // deleted with the builder agent on 2026-11-19.)
        // `/onboarding-readiness`, `/onboarding/github-setup` and the
        // `/onboarding/*` subtree live in `oxy-api-tenancy`; `oxy-server`
        // merges them into this nest.
        // `/sql/{pathb64}` and `/sql/query` (the Dev Portal's SQL execute), the
        // semantic explorer (`/semantic`, `/semantic/compile`, a topic's and a
        // view's details), `/semantic/objects`, `/context-graph/sources`, the
        // compile boundary's revision reads (`/revision`, `/revision/files`,
        // `/revision/files/content`) and the pre-aggregation status and
        // rebuild are `oxy-api-semantic`'s since 2026-11-16, merged inside this
        // nest by `oxy-server` through the workspace seam, still FleetOk.
        // The metric tree (`/semantic/metric-tree*`, under its timing probe),
        // `/semantic/monitors` and the anomaly inbox (`/semantic/anomalies`) are
        // `oxy-api-metric-monitoring`'s, merged inside this nest by `oxy-server`
        // through `SurfaceSeams::workspace`, all FleetOk in its
        // `workspace_route_roles` (2026-11-15).
        // Declared worlds and their runs, and experiments, are
        // `oxy-api-experiment`'s: merged inside this nest by `oxy-server` through
        // `SurfaceSeams::workspace`; every route there is FleetOk.
        // The World Model Graph (`/semantic/world-model*`) and peer cohorts
        // (`/semantic/cohort`) are `oxy-api-world-model`'s, merged inside this
        // nest by `oxy-server` through `SurfaceSeams::workspace`, all FleetOk
        // in its `workspace_route_roles`.
        // `/world-model/events`, foot traffic, competitors, and (since
        // 2026-11-15) the camera list and the weather (`/world-model/cameras`,
        // `/world-model/weather/*`, out of `api/video.rs`) are
        // `oxy-api-world-model`'s, merged inside this nest by `oxy-server`
        // through `SurfaceSeams::workspace`, still FleetOk.
        // `/results/files/{file_id}` (a run's result parquet, S3-mirrored) is
        // `oxy-api-threads`' since 2026-11-16, merged inside this nest through
        // the workspace seam, still FleetOk.
        .nest_declared(
            "/analytics",
            agentic_router(agentic_state.clone()),
            agentic_http::router_roles(),
        )
        // New agentic-workflow surface — coexists with the legacy `/workflows`
        // routes during migration. Will subsume them in the cleanup task.
        .nest_declared(
            "/agentic-workflows",
            automation_router(agentic_state.clone()),
            agentic_http::automation_router_roles(),
        )
        // Canonical execution surface alias (Procedures/Workflows -> Automations).
        // Same handlers as `/agentic-workflows`, mirrored in `role_manifest.rs`.
        .nest_declared(
            "/agentic-automations",
            automation_router(agentic_state.clone()),
            agentic_http::automation_router_roles(),
        )
        .nest_declared(
            "/agentic-airway",
            airway_router(agentic_state),
            agentic_http::airway_router_roles(),
        );
    // `/agentic-schedules` (schedule CRUD, run-now, backfill; relocated from
    // agentic-http by §12 FU4b because the handlers need `WorkspaceAdmin`) is
    // `oxy-api-automations`' since 2026-11-15, merged inside this nest by
    // `oxy-server` through `SurfaceSeams::workspace`. The `AgenticState`
    // extension its handlers extract, which `build_schedule_routes` layered
    // here, now comes from `protected::with_agentic_state` over the seam.

    if include_local_setup {
        router = router
            .route_fleet("/setup/empty", post(local_setup::setup_empty))
            .route_fleet("/setup/demo", post(local_setup::setup_demo));
    }

    router
}

/// Curated subset of workspace routes for the EXTERNAL API surface
/// (`/external/api/{workspace_id}/...`): just the query + agent + world-model
/// endpoints a standalone app needs, WITHOUT the IDE / file / git / admin /
/// settings surface. Mounted with API-key-only auth + wide-open CORS by
/// [`super::protected::build_external_api_router`]. Reuses the exact same
/// handler functions as `build_workspace_routes`, so behavior is identical —
/// only the auth + CORS wrapper differs.
pub(super) fn build_external_workspace_routes(
    app_state: &AppState,
    agentic_state: Arc<AgenticState>,
) -> RoleRouter {
    RoleRouter::new(app_state.clone())
        // `/sql/query`, `/semantic` and `/semantic/compile` are
        // `oxy-api-semantic`'s since 2026-11-16, merged into this tree by
        // `oxy-server` through `SurfaceSeams::external`.
        // `/world-model/events`, foot traffic, competitors, the LLM proxy, the
        // camera list and the weather (`/world-model/cameras`,
        // `/world-model/weather/*`) are `oxy-api-world-model`'s, merged into
        // this tree by `oxy-server` through `SurfaceSeams::external`.
        // Camera live streaming for standalone apps: registry, WHEP
        // signaling, HLS proxy. Same handlers as the operator tree —
        // only the auth wrapper differs.
        .nest_all(
            "/world-model/camera-stream",
            RouteRole::FleetOk,
            oxy_cameras::routes::external_stream_routes::<AppState>(agentic_state.db.clone()),
            "camera streaming signals against Postgres + the media edge",
        )
        // Evidence/compliance clip playback for standalone apps — mints a
        // presigned S3 GET (workspace-prefix checked). Without this the
        // customer app's clip-URL call falls through to the SPA.
        //
        // Path is intentionally `/cameras/clips/...` at the tree root
        // (mirrors the operator route), NOT under `/world-model/camera-stream`
        // like the streaming nest — already-shipped customer apps call this
        // exact path. Don't "tidy" it under the stream prefix; that silently
        // re-breaks playback and would need a coordinated app+server deploy.
        .merge_undeclared(
            oxy_cameras::routes::external_clip_routes::<AppState>(),
            "oxy-cameras clip playback mints a presigned S3 GET",
        )
        // The anomaly inbox (list, scan, preview, status, explain) the SDK's
        // `client.anomalies.*` calls here is `oxy-api-metric-monitoring`'s,
        // merged into this tree by `oxy-server` through `SurfaceSeams::external`.
        // Agentic analytics: POST /analytics/runs, the SSE events stream,
        // /answer, /cancel, /threads/* — the chat surface external apps drive.
        .nest_declared(
            "/analytics",
            agentic_router(agentic_state),
            agentic_http::router_roles(),
        )
}

// `build_workflow_routes` and `build_automation_routes` were retired
// alongside `oxy-workflow`. The agentic-pipeline workflow surface mounted
// at `/agentic-workflows` replaces every endpoint they exposed.

// `build_integration_routes` left with its last two routes (the OAuth
// connects) for `oxy-api-workspace-config` on 2026-11-21; see the comment
// where it was nested above.

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_pipeline::platform::ThreadOwnerLookup;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use sea_orm::DatabaseConnection;
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    use oxy_app_core::serve_mode::ServeMode;

    fn test_app_state() -> AppState {
        AppState {
            enterprise: false,
            internal: false,
            mode: ServeMode::Local,
            observability: None,
            startup_cwd: std::path::PathBuf::new(),
            preagg_cache: None,
            preagg_renewal_threshold_secs: None,
            agentic_state: None,
            semantic_layer_cache: oxy_app_core::workspace_cache::new_semantic_layer_cache(),
            semantic_engine_cache: oxy_app_core::workspace_cache::new_semantic_engine_cache(),
            authz: oxy_server_authz::enforcer(),
        }
    }

    struct StubThreadOwner;

    #[async_trait]
    impl ThreadOwnerLookup for StubThreadOwner {
        async fn thread_owner(
            &self,
            _thread_id: uuid::Uuid,
        ) -> Result<Option<Option<uuid::Uuid>>, String> {
            Ok(None)
        }
    }

    fn test_agentic_state() -> Arc<AgenticState> {
        Arc::new(agentic_http::new_agentic_state(
            CancellationToken::new(),
            DatabaseConnection::default(),
            Arc::new(StubThreadOwner),
        ))
    }

    async fn status_for(router: axum::Router, method: &str, path: &str) -> StatusCode {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        router.oneshot(req).await.unwrap().status()
    }

    /// The IDE's git and file routes are gone: every one 404s. Workspaces are
    /// edited in their repository (`internal-docs/factory-retirement.md`,
    /// phase 5).
    #[tokio::test]
    async fn the_ide_git_and_file_routes_are_gone() {
        let state = test_app_state();
        let router = build_workspace_routes(state.clone(), test_agentic_state(), false)
            .into_router()
            .with_state(state);

        let cases: &[(&str, &str)] = &[
            ("GET", "/branches"),
            ("DELETE", "/branches/foo"),
            ("POST", "/switch-branch"),
            ("POST", "/pull-changes"),
            ("POST", "/fetch"),
            ("POST", "/push-changes"),
            ("POST", "/force-push"),
            ("POST", "/discard-all"),
            ("POST", "/abort-rebase"),
            ("POST", "/continue-rebase"),
            ("POST", "/resolve-conflict-file"),
            ("POST", "/unresolve-conflict-file"),
            ("POST", "/resolve-conflict-with-content"),
            ("GET", "/recent-commits"),
            ("GET", "/revision-info"),
            ("POST", "/reset-to-commit"),
            ("GET", "/files"),
            ("GET", "/files/Zm9vLnltbA=="),
            ("POST", "/files/Zm9vLnltbA=="),
            ("GET", "/files/Zm9vLnltbA==/from-git"),
            ("POST", "/files/Zm9vLnltbA==/revert"),
            ("GET", "/git-state"),
            ("GET", "/status"),
            ("GET", "/worktrees"),
            ("POST", "/compile"),
            ("GET", "/compile/status"),
            ("POST", "/databases/sync"),
            ("POST", "/databases/build"),
            ("POST", "/databases/clean"),
            ("DELETE", "/app-integrations/toast"),
        ];

        for (method, path) in cases {
            let router = router.clone();
            let status = status_for(router, method, path).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "{method} {path} must 404: the IDE route is removed (got {status})"
            );
        }
    }

    /// Setup endpoints are mounted when `include_local_setup: true` (local mode).
    #[tokio::test]
    async fn setup_routes_present_when_include_local_setup_true() {
        let state = test_app_state();
        let router = build_workspace_routes(state.clone(), test_agentic_state(), true)
            .into_router()
            .with_state(state);

        for path in ["/setup/empty", "/setup/demo"] {
            let status = status_for(router.clone(), "POST", path).await;
            assert_ne!(
                status,
                StatusCode::NOT_FOUND,
                "{} must be mounted when include_local_setup=true (got {})",
                path,
                status
            );
        }
    }

    /// Setup endpoints are absent when `include_local_setup: false` (cloud mode).
    #[tokio::test]
    async fn setup_routes_absent_when_include_local_setup_false() {
        let state = test_app_state();
        let router = build_workspace_routes(state.clone(), test_agentic_state(), false)
            .into_router()
            .with_state(state);

        for path in ["/setup/empty", "/setup/demo"] {
            let status = status_for(router.clone(), "POST", path).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "{} must 404 when include_local_setup=false (got {})",
                path,
                status
            );
        }
    }

    /// The camera fleet operator surface (sites / cameras / edge boxes /
    /// UniFi integration / preview proxies) MUST be merged into
    /// `build_workspace_routes`. That's the only thing keeping it
    /// behind `workspace_middleware`/`local_context_middleware` +
    /// `auth_middleware` — see the cross-workspace write incident
    /// closed by commit `21735f047`. If someone re-merges these routes
    /// at the app root (or under an un-authed mount), this test fails
    /// fast instead of waiting for a real cross-workspace breach.
    #[tokio::test]
    async fn camera_operator_routes_mounted_inside_workspace_tree() {
        let state = test_app_state();
        let router = build_workspace_routes(state.clone(), test_agentic_state(), false)
            .into_router()
            .with_state(state);

        // One representative from each operator sub-area. Wildcard
        // routes (HLS / recording) aren't worth poking here — the
        // mount-point check on the non-wildcard variants is enough.
        let cases: &[(&str, &str)] = &[
            ("GET", "/cameras/sites"),
            ("GET", "/cameras/edge-boxes"),
            ("GET", "/cameras"),
            ("POST", "/integrations/unifi/preview"),
        ];
        for (method, path) in cases {
            let status = status_for(router.clone(), method, path).await;
            assert_ne!(
                status,
                StatusCode::NOT_FOUND,
                "{method} {path} must be mounted inside build_workspace_routes (got {status}); \
                 mounting it elsewhere bypasses workspace_middleware + auth_middleware"
            );
        }
    }
}
