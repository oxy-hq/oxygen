//! The per-workspace route tree. Mounted under `/{workspace_id}` in both
//! cloud and local modes (local always uses the nil UUID).
//!
//! This module owns the tree shape plus all per-resource sub-builders
//! (automations, threads, agents, files, etc.). Secrets live in their own
//! module because they ship with an admin-gated middleware.

use std::sync::Arc;

use axum::routing::{get, post, put};

use agentic_http::{AgenticState, airway_router, automation_router, router as agentic_router};

use crate::api::{
    agent, apps, artifacts, automation, chart, compile_staging, data, database,
    execution_analytics, integration, local_setup, metric_anomalies, metric_tree,
    metric_tree_probe, metric_tree_projection, metrics, org_subdomain, preagg, result_files,
    schedules, semantic, semantic_scan, thread, traces, video, workspace_logo, workspace_members,
    workspace_oxy_access, workspaces,
};

use oxy_shared::fleet_role::RouteRole;

use super::AppState;
use super::role_router::RoleRouter;
use super::secrets::build_secret_routes;

pub(super) fn build_workspace_routes(
    app_state: AppState,
    agentic_state: Arc<AgenticState>,
    include_local_setup: bool,
) -> RoleRouter {
    let mut router = RoleRouter::new(app_state.clone())
        // `/details` served both halves and was the last route needing the
        // ide-down degrade mechanism. The frontend fetches these two instead.
        .route_fleet("/meta", get(workspaces::get_workspace_meta))
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
        // `/agentic-workflows` (IdeOnly), mounted below. The automation LIST,
        // however, is served HERE from the compile boundary (FleetOk) so the
        // customer-nav sidebar renders on a stateless serve replica with no
        // working copy — `/agentic-workflows` is IdeOnly and lives in a crate
        // that can't reach `compiled_reader`.
        .route_fleet("/procedures", get(automation::list_automations))
        // Single-automation YAML, also from the boundary, so clicking an automation
        // renders its diagram on a serve node when the ide is down.
        .route_fleet("/procedures/{path_b64}", get(automation::get_automation))
        // Canonical "automations" aliases for the procedure list/get (the term
        // Procedure/Workflow was renamed to Automation). Same FleetOk handlers,
        // classified identically in `role_manifest.rs`. `/procedures` is kept
        // for backward compatibility.
        .route_fleet("/automations", get(automation::list_automations))
        .route_fleet("/automations/{path_b64}", get(automation::get_automation))
        // `/airway-pipelines` (the boundary-backed pipeline listing and the
        // sp_api marketplace list) moved to `oxy-api-airway`, merged inside this
        // nest through the workspace seam, FleetOk as before.
        .nest("/threads", build_thread_routes(&app_state))
        .nest("/agents", build_agent_routes(&app_state))
        // `/api-keys` (the legacy keys) and `/api-tokens` (the tokens that can
        // reach this workspace) moved to `oxy-api-identity`, merged inside this
        // nest through the workspace seam, FleetOk as before.
        // Staging branch compile for a custom-app draft build to pin
        // (`oxyc publish --semantic-branch`). The POST asks GitHub where the
        // branch is and queues a compile of that commit; the status reads
        // Postgres. Both FleetOk. A branch GitHub does not have is refused
        // with the reason.
        .route_fleet(
            "/compile/staging",
            post(compile_staging::enqueue_staging_compile),
        )
        .route_fleet(
            "/compile/staging/status",
            get(compile_staging::staging_compile_status),
        )
        .nest("/databases", build_database_routes(&app_state))
        .nest("/integrations", build_integration_routes(&app_state))
        .nest("/secrets", build_secret_routes(app_state.clone()))
        .route_fleet("/members", get(workspace_members::list_workspace_members))
        .route_fleet(
            "/members/{user_id}",
            put(workspace_members::set_workspace_role_override)
                .delete(workspace_members::remove_workspace_role_override),
        )
        // POST = lock Oxy staff OUT; DELETE = lift the lockdown (default is
        // access-granted). Only a REAL org officer may do either.
        .route_fleet(
            "/oxy-access",
            get(workspace_oxy_access::get_oxy_access)
                .post(workspace_oxy_access::lock_oxy_access)
                .delete(workspace_oxy_access::unlock_oxy_access),
        )
        .route_fleet("/org-subdomain", get(org_subdomain::get_org_subdomain))
        .route_fleet("/logo", get(workspace_logo::get_workspace_logo))
        // `/source-uploads` moved to the `oxy-api-airway` sibling crate,
        // merged inside this nest by `oxy-server` through the workspace seam.
        .nest(
            "/app-integrations",
            build_app_integration_routes(&app_state),
        )
        .nest_all(
            "/traces",
            RouteRole::FleetOk,
            traces::traces_routes(),
            "trace reads come from the runtime tables",
        )
        .nest_all(
            "/metrics",
            RouteRole::FleetOk,
            metrics::metrics_routes(),
            "metric reads come from the runtime tables",
        )
        .nest_all(
            "/execution-analytics",
            RouteRole::FleetOk,
            execution_analytics::execution_analytics_routes(),
            "run analytics aggregate the runtime tables",
        )
        .route_fleet("/artifacts/{id}", get(artifacts::get_artifact))
        .route_fleet("/charts/{file_path}", get(chart::get_chart))
        .route_fleet("/logs", get(thread::get_logs))
        .route_fleet(
            "/builder-availability",
            get(agent::check_builder_availability),
        )
        // `/onboarding-readiness`, `/onboarding/github-setup` and the
        // `/onboarding/*` subtree moved to the `oxy-api-onboarding` sibling
        // crate; they are merged into this nest by `oxy-server` and declared
        // at that seam, since `route_ide` cannot reach across the crate line.
        .route_fleet("/sql/{pathb64}", post(data::execute_sql))
        .route_fleet("/sql/query", post(data::execute_sql_query))
        // Semantic-model endpoints the IDE uses. The legacy `/semantic`
        // execute route was retired alongside `oxy-workflow`; execution
        // now flows through the agentic pipeline. Compile + the
        // read-only file handlers stay here so the IDE's topic / view
        // explorer + SQL preview panel keep working without re-introducing
        // a workflow dependency on the request path. Compile reaches
        // into airlayer via `agentic_automation::semantic_bridge`.
        // The views and topics the workspace serves, for the explorer's list
        // and the pickers: the revision (or single-process mode's directory).
        .route_fleet(
            "/semantic/objects",
            get(semantic_scan::list_semantic_objects),
        )
        // The Context Graph page's views, topics, automations, agents and
        // verified-query paths, from the served revision (it read `/files`).
        .route_fleet(
            "/context-graph/sources",
            get(semantic_scan::get_context_graph_sources),
        )
        // The Factory's read-only file browser: the served revision's files
        // and one file's content, from the compiled rows (it read `/files`).
        // The revision this request reads and its commit, for the top bar.
        .route_fleet("/revision", get(semantic_scan::get_served_revision))
        .route_fleet("/revision/files", get(semantic_scan::get_revision_files))
        .route_fleet(
            "/revision/files/content",
            get(semantic_scan::get_revision_file),
        )
        .route_fleet(
            "/semantic/topic/{file_path_b64}",
            get(semantic::get_topic_details),
        )
        .route_fleet(
            "/semantic/view/{file_path_b64}",
            get(semantic::get_view_details),
        )
        // Moved to `api::preagg` by #2989, which also added the rebuild verb.
        // Both stay FleetOk: the list comes from the DECLARATIONS (resolved
        // through the compile boundary first), and a rollup built on another
        // node is readable here when a blob bucket is configured.
        .route_fleet("/semantic/preagg-status", get(preagg::get_preagg_status))
        .route_fleet("/semantic/preagg-rebuild", post(preagg::rebuild_preagg))
        .route_fleet("/semantic/compile", post(semantic::compile_semantic_query))
        .route_fleet("/semantic", post(semantic::execute_semantic_query))
        // Metric tree — structure + pure analysis ops over the semantic model.
        // Merged as its own sub-router so the diagnostic probe layers over
        // these ten routes and nothing else.
        //
        // FleetOk on every route: the scan root resolves through the compile
        // boundary first (`semantic::resolve_query_scan_source`), working copy
        // second, and warehouse execution needs only config + secrets — so a
        // replica answers these. Pinned by
        // `workspace_metric_tree_routes_are_fleet_ok`; the outage behind it is
        // oxy-hq/oxygen#878 (every call 500'd on the serve fleet when these
        // read `semantics_scan_path()` directly).
        .merge(build_metric_tree_routes(&app_state))
        // Declared worlds and their runs, and experiments, are
        // `oxy-api-experiment`'s: merged inside this nest by `oxy-server` through
        // `SurfaceSeams::workspace`; every route there is FleetOk.
        // The World Model Graph (`/semantic/world-model*`) and peer cohorts
        // (`/semantic/cohort`) are `oxy-api-world-model`'s, merged inside this
        // nest by `oxy-server` through `SurfaceSeams::workspace`, all FleetOk
        // in its `workspace_route_roles`.
        // Anomaly inbox — backed by oxy-metric-monitoring. Nested so the
        // `Extension<Arc<AgenticState>>` layer scopes only to these routes
        // (same pattern as `build_schedule_routes` below).
        //
        // `/semantic/monitors` is merged rather than plainly routed because it
        // also extracts `Arc<AgenticState>` (for the per-segment scan coverage
        // it returns alongside the config) and so needs the same Extension
        // layer. Merging a one-route sub-router keeps the layer scoped to it
        // and preserves the exact path — nesting would also answer on
        // `/semantic/monitors/`, which nothing calls.
        .merge({
            use axum::Extension;
            RoleRouter::new(app_state.clone())
                .route_fleet("/semantic/monitors", get(metric_anomalies::list_monitors))
                .map_router(|r| r.layer(Extension(agentic_state.clone())))
        })
        .nest(
            "/semantic/anomalies",
            build_metric_anomaly_routes(&app_state, agentic_state.clone()),
        )
        // `/world-model/events`, foot traffic and competitors are
        // `oxy-api-world-model`'s, merged inside this nest by `oxy-server`
        // through `SurfaceSeams::workspace`.
        .route_fleet("/world-model/cameras", get(video::list_cameras))
        .route_fleet(
            "/world-model/weather/{layer}/{z}/{x}/{y}",
            get(video::weather_tile),
        )
        .route_fleet(
            "/world-model/weather/current",
            post(video::weather_current_batch),
        )
        .route_fleet(
            "/results/files/{file_id}",
            get(result_files::get_result_file).delete(result_files::delete_result_file),
        )
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
            airway_router(agentic_state.clone()),
            agentic_http::airway_router_roles(),
        )
        // Relocated from agentic-http (§12 FU4b): the schedule handlers
        // need WorkspaceAdmin from app/role_guards which agentic-http
        // cannot depend on.
        .nest(
            "/agentic-schedules",
            build_schedule_routes(&app_state, agentic_state),
        );

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
        .route_fleet("/sql/query", post(data::execute_sql_query))
        .route_fleet("/semantic", post(semantic::execute_semantic_query))
        .route_fleet("/semantic/compile", post(semantic::compile_semantic_query))
        // `/world-model/events`, foot traffic, competitors and the LLM proxy are
        // `oxy-api-world-model`'s, merged into this tree by `oxy-server` through
        // `SurfaceSeams::external`.
        .route_fleet("/world-model/cameras", get(video::list_cameras))
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
        .route_fleet(
            "/world-model/weather/{layer}/{z}/{x}/{y}",
            get(video::weather_tile),
        )
        .route_fleet(
            "/world-model/weather/current",
            post(video::weather_current_batch),
        )
        // Anomaly monitoring: list, scan, update status, explain — used by
        // standalone apps (e.g. world-model-app) that can't reach the internal
        // /api surface. Mirrors `build_metric_anomaly_routes` exactly, so the
        // TypeScript SDK's `client.anomalies.*` (list/scan/updateStatus/explain)
        // works against `/external/api` as well as `/api`.
        //
        // Nested with an explicit Extension layer because every handler extracts
        // Arc<AgenticState> for db access (same pattern as build_metric_anomaly_routes).
        //
        // `/scan` and `/{id}/explain` are long-running (a scan waits up to 55 s
        // before returning `pending: true`; an uncached explain runs a 20-30 s
        // recursive search) but bounded by the same `timeout_middleware` the rest
        // of this surface carries — no separate budget needed.
        // The same builder the internal surface nests, rather than a copy of
        // it. The two differed only in a path-parameter NAME (`{id}` vs
        // `{anomaly_id}`), which no handler reads — both take
        // `Path<(Uuid, Uuid)>`, positionally. Keeping the copy also hid these
        // routes from `route_role_derivation`, whose parser resolves a nest
        // prefix through the `build_*` call and cannot see into an inline
        // block: `/scan` reached that guard with no prefix and classified as
        // FleetOk by default.
        .nest(
            "/semantic/anomalies",
            build_metric_anomaly_routes(&app_state, agentic_state.clone()),
        )
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

fn build_thread_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone())
        .route_fleet(
            "/",
            get(thread::get_threads)
                .post(thread::create_thread)
                .delete(thread::delete_all_threads),
        )
        .route_fleet("/bulk-delete", post(thread::bulk_delete_threads))
        .route_fleet(
            "/{id}",
            get(thread::get_thread).delete(thread::delete_thread),
        )
        .route_fleet("/{id}/stop", post(thread::stop_thread))
}

fn build_agent_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone()).route_fleet("/", get(agent::get_agents))
}

/// Metric-tree structure + analysis ops, carrying the diagnostic probe.
///
/// Kept as a sub-router purely so [`metric_tree_probe::probe`] covers exactly
/// these routes: layering it on the workspace router would time every
/// workspace endpoint, and the question it answers is about these ten.
/// Paths stay absolute (`.merge`, not `.nest`) so the role manifest's
/// classification of each one is unchanged.
fn build_metric_tree_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone())
        .route_fleet("/semantic/metric-tree", get(metric_tree::get_metric_tree))
        .route_fleet(
            "/semantic/metric-tree/{measure_id}/sensitivity",
            get(metric_tree::get_sensitivity),
        )
        .route_fleet(
            "/semantic/metric-tree/predict",
            post(metric_tree::post_predict),
        )
        .route_fleet(
            "/semantic/metric-tree/explain",
            post(metric_tree::post_explain),
        )
        .route_fleet(
            "/semantic/metric-tree/opportunity",
            post(metric_tree::post_opportunity),
        )
        .route_fleet(
            "/semantic/metric-tree/drill",
            post(metric_tree::post_opportunity_drill),
        )
        .route_fleet(
            "/semantic/metric-tree/time-dimensions",
            get(metric_tree::get_time_dimensions),
        )
        .route_fleet(
            "/semantic/metric-tree/distribution",
            post(metric_tree::post_distribution),
        )
        .route_fleet(
            "/semantic/metric-tree/baseline",
            post(metric_tree::post_baseline),
        )
        .route_fleet(
            "/semantic/metric-tree/projection",
            post(metric_tree_projection::post_projection),
        )
        .map_router(|r| r.layer(axum::middleware::from_fn(metric_tree_probe::probe)))
}

/// Schedule CRUD + run-now (§12 FU4b). Lives in the app crate so the
/// handlers can use `WorkspaceAdmin` from `role_guards` (agentic-http is
/// a lower layer and must not depend on `app`). AgenticState is attached
/// as an Extension here so the handlers can extract it.
fn build_metric_anomaly_routes(
    app_state: &AppState,
    agentic_state: Arc<AgenticState>,
) -> RoleRouter {
    use axum::Extension;
    RoleRouter::new(app_state.clone())
        .route_fleet("/", get(metric_anomalies::list_anomalies))
        // Mounted on the external surface too (`build_external_workspace_routes`).
        // The scan runner reads the semantic model off the workspace root, so
        // FleetOk was a promise a replica could not keep.
        .route_ide("/scan", post(metric_anomalies::run_scan))
        // The dry run of one monitor. Unlike `/scan` it asks for no working
        // copy: the file comes from the compile boundary and the semantic
        // model through `resolve_scan`, so any replica can answer.
        .route_fleet("/preview", post(metric_anomalies::preview_monitor))
        // Static `/status` (bulk) and `/{anomaly_id}/status` (single) differ in
        // segment count, so they don't compete for a match. Bulk triage writes
        // Postgres rows only — any replica can take it.
        .route_fleet("/status", post(metric_anomalies::update_status_bulk))
        .route_fleet(
            "/{anomaly_id}/status",
            post(metric_anomalies::update_status),
        )
        // Like `/preview`: the semantic model through `resolve_scan`, the
        // anomaly a Postgres row, the queries to the warehouse.
        .route_fleet(
            "/{anomaly_id}/explain",
            post(metric_anomalies::explain_anomaly),
        )
        .map_router(|r| r.layer(Extension(agentic_state)))
}

fn build_schedule_routes(app_state: &AppState, agentic_state: Arc<AgenticState>) -> RoleRouter {
    use axum::Extension;
    RoleRouter::new(app_state.clone())
        .route_fleet("/", get(schedules::list).post(schedules::create))
        .route_fleet(
            "/{id}",
            get(schedules::get)
                .patch(schedules::update)
                .delete(schedules::delete),
        )
        .route_fleet("/{id}/run-now", post(schedules::run_now))
        .route_fleet("/{id}/backfill", post(schedules::backfill))
        .map_router(|r| r.layer(Extension(agentic_state)))
}

fn build_database_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone())
        // The list degrades to `datasets: null` without a working copy and the
        // launcher readiness check calls it on every page load.
        .route_fleet("/", get(database::list_databases))
        .route_ide("/test-connection", post(database::test_database_connection))
        .route_fleet("/inspect", post(database::inspect_database_handler))
        .route_fleet("/inspect-schemas", post(database::inspect_schemas_handler))
        .route_fleet(
            "/inspect-schema-tables",
            post(database::inspect_schema_tables_handler),
        )
        .route_fleet(
            "/{database_name}/schema",
            get(database::get_database_schema),
        )
}

fn build_integration_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone())
        .route_ide("/looker", get(integration::list_looker_integrations))
        .route_fleet(
            "/quickbooks/authorize",
            post(crate::integrations::quickbooks::oauth::authorize::authorize),
        )
        // Uniform OAuth connect for any provider in `integrations::oauth_provider`.
        // FleetOk like its QuickBooks predecessor: Postgres state row + secret
        // manager, no filesystem and no local state dir.
        .route_fleet(
            "/oauth/{provider}/authorize",
            post(crate::integrations::quickbooks::oauth::authorize::authorize_by_slug),
        )
}

/// World-model "Apps" configuration routes — Toast / OpenWeatherMap /
/// BestTime integration entries surfaced in the Workspace Settings →
/// Apps tab.
fn build_app_integration_routes(app_state: &AppState) -> RoleRouter {
    RoleRouter::new(app_state.clone()).route_fleet("/", get(apps::list_apps))
}

/// Workspace previews, mounted at `/{workspace_id}/previews` BESIDE the
/// workspace tree rather than inside it, behind the access check alone
/// (`workspace_access_middleware`) instead of `workspace_middleware`.
///
/// Two reasons. The full middleware resolves a revision and builds a workspace
/// manager for `?branch=`, and here `?branch=` names the preview being deleted
/// or refreshed, not a working copy to read. And a request carrying the
/// preview header must still be able to manage previews, which the read-only
/// guard inside `workspace_middleware` would refuse.
///
/// Every route here is on the fleet. Creating and refreshing ask GitHub where
/// the branch is and queue a compile of that commit; the rest touch only
/// Postgres. A branch GitHub does not have is refused with the reason.
pub(super) fn build_workspace_preview_routes(app_state: &AppState) -> RoleRouter {
    use crate::api::workspace_previews as previews;
    RoleRouter::new(app_state.clone())
        .route_fleet(
            "/",
            post(previews::create_preview)
                .get(previews::list_previews)
                .delete(previews::delete_preview),
        )
        .route_fleet("/refresh", post(previews::refresh_preview))
        // Postgres only (the analyze row and its run outcome): any replica.
        .route_fleet("/checks", get(previews::get_checks))
        // Held procedure dry runs: the handlers write and read rows; the run
        // itself executes on the worker fleet. Any replica.
        .route_fleet("/runs", post(previews::start_run).get(previews::list_runs))
        .route_fleet("/runs/{run_id}", get(previews::get_run))
        // Airway samples' sandbox companies: rows only. Any replica.
        .route_fleet(
            "/sources",
            get(previews::list_sources).put(previews::put_source),
        )
}

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
            semantic_layer_cache: crate::server::router::workspace_cache::new_semantic_layer_cache(
            ),
            semantic_engine_cache:
                crate::server::router::workspace_cache::new_semantic_engine_cache(),
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
        Arc::new(AgenticState::new(
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

    /// The external API surface must expose the FULL anomaly inbox — list,
    /// scan, status, explain — not just the read half. Standalone custom apps
    /// can't reach `/api`, so a missing verb here means the TypeScript SDK's
    /// `client.anomalies.scan()` / `.explain()` 404 against `/external/api`
    /// while the same calls work in the IDE.
    #[tokio::test]
    async fn external_surface_exposes_full_anomaly_inbox() {
        let state = test_app_state();
        let router = build_external_workspace_routes(&state, test_agentic_state())
            .into_router()
            .with_state(state);

        let anomaly_id = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";

        // `POST /{id}/status` was already mounted externally BEFORE this change
        // and is known to work in production, so whatever a bare test router
        // returns for it ("routed fine, failed later on absent workspace
        // context") is the reference for a correctly-resolving route.
        let baseline = status_for(
            router.clone(),
            "POST",
            &format!("/semantic/anomalies/{anomaly_id}/status"),
        )
        .await;
        assert!(
            !baseline.is_success() && baseline != StatusCode::NOT_FOUND,
            "baseline POST /semantic/anomalies/{{id}}/status returned {baseline}; this test \
             assumes a bare router routes the request and then fails on missing context"
        );

        let cases: &[(&str, String)] = &[
            ("GET", "/semantic/anomalies".to_string()),
            ("POST", "/semantic/anomalies/scan".to_string()),
            // Bulk status — `client.anomalies.updateStatusBulk()`. One segment,
            // so it must not be swallowed by the two-segment `/{id}/status`.
            ("POST", "/semantic/anomalies/status".to_string()),
            ("POST", format!("/semantic/anomalies/{anomaly_id}/status")),
            ("POST", format!("/semantic/anomalies/{anomaly_id}/explain")),
        ];
        for (method, path) in cases {
            let status = status_for(router.clone(), method, path).await;
            assert_ne!(
                status,
                StatusCode::NOT_FOUND,
                "{method} {path} must be mounted on the external API surface (got {status}); \
                 custom apps reach anomalies only through /external/api"
            );
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path} is mounted on the external API surface under a DIFFERENT \
                 verb (got {status}); the SDK calls it with {method}"
            );
            // Equality with the baseline is what rules out a *silently broken*
            // mount: a route that resolves but whose role extractor stopped
            // resolving would diverge here (401/403) while still passing the
            // 404/405 checks above.
            assert_eq!(
                status, baseline,
                "{method} {path} returned {status} but the already-proven \
                 POST /semantic/anomalies/{{id}}/status returned {baseline}. These handlers all \
                 take the same EffectiveWorkspaceRole extractor, so a divergence means this \
                 route resolves differently — check the extractor stack, not just the mount."
            );
        }
    }
}
