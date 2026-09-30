//! `/api/projects/{project_id}/semantic/world-model*` — world-model graph
//! + instances for customer-app bundles.
//!
//! Customer-app-gated variants of the IDE's workspace-scoped
//! `/semantic/world-model*` handlers. They enter through the customer-app
//! gate and load the semantic model from the compile boundary (via
//! [`super::semantic_boundary`]) so they run on the stateless serve fleet.
//! The graph-assembly and instance-listing cores are shared with the
//! workspace handlers in [`crate::server::api::world_model_graph`] — only
//! the layer/path acquisition differs.

use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use entity::workspace_members::WorkspaceRole;
use oxy::utils::create_sse_stream;
use uuid::Uuid;

use crate::server::api::projects::semantic_boundary::{
    cache_lookup, cache_store, enter_semantic_boundary, err, err_with_code, load_layer,
    wants_refresh,
};
use crate::server::api::world_model_graph::{
    WmInstancesQuery, WmMeasureBreakdownQuery, build_world_model_response, instances_core,
    measure_breakdown_core,
};

/// `GET .../world-model` — the entity/measure graph (nodes + promotion/FK
/// edges), with the `.world-model.yml` display config applied.
pub async fn get_world_model(
    Path(project_id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    // Gate before cache read so a cached hit can't bypass authorization.
    let boundary = match enter_semantic_boundary(&headers, project_id).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    if let Some(hit) = cache_lookup(&boundary, "wm-graph", "", wants_refresh(uri.query())) {
        return hit;
    }
    let layer = match load_layer(boundary.scan.path_buf()).await {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    // The manager knows whether there is a working copy to fall back to. On a
    // replica the compiled row is the only source, and it says `NoSource`
    // rather than `None` — which is what keeps "not compiled" from reading as
    // "no display overrides" on the public custom-app router.
    let config_manager = &boundary.proj_ctx.workspace_manager().config_manager;
    match build_world_model_response(&layer, config_manager).await {
        Ok(resp) => cache_store(&boundary, "wm-graph", "", &resp),
        Err(message) => err_with_code(
            StatusCode::INTERNAL_SERVER_ERROR,
            message,
            "world_model_failed",
        ),
    }
}

/// `GET .../world-model/instances` — bounded, searchable listing of an
/// entity's instances (primary key + display label).
pub async fn get_world_model_instances(
    Path(project_id): Path<Uuid>,
    Query(q): Query<WmInstancesQuery>,
    headers: HeaderMap,
) -> Response {
    let boundary = match enter_semantic_boundary(&headers, project_id).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let layer = match load_layer(boundary.scan.path_buf()).await {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    // The graph, from the gate context this route already holds: no second
    // connection, no second workspace read. Reach only when asked, and it
    // goes into the scan rather than over the page.
    let reach = if q.scope.as_deref() == Some("reach") {
        Some(
            crate::server::api::operating_graph::reach::reach_for_viewer(
                &boundary.app.db,
                boundary.app.org_id,
                &boundary.app.user,
                project_id,
                q.app_id(),
            )
            .await,
        )
    } else {
        None
    };
    let graph = crate::server::api::world_model_graph::GraphScope {
        db: boundary.app.db.clone(),
        org_id: boundary.app.org_id,
        reach,
    };
    match instances_core(
        boundary.proj_ctx.workspace_manager(),
        boundary.app.user.id,
        WorkspaceRole::Viewer,
        &layer,
        boundary.scan.path_buf(),
        // No engine cache on this path: `enter_semantic_boundary` is
        // headers-driven and carries no `AppState`.
        None,
        &q,
        Some(&graph),
    )
    .await
    {
        Ok(resp) => Json(resp).into_response(),
        Err((status, body)) => (status, body).into_response(),
    }
}

/// `GET .../world-model/measure-breakdown` — SSE driver-tree decomposition of
/// one instance's measure (the per-instance RCA view). Streams
/// `init → value* → done`.
pub async fn get_measure_breakdown(
    Path(project_id): Path<Uuid>,
    Query(q): Query<WmMeasureBreakdownQuery>,
    headers: HeaderMap,
) -> Response {
    let boundary = match enter_semantic_boundary(&headers, project_id).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };
    let layer = match load_layer(boundary.scan.path_buf()).await {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    // `measure_breakdown_core` moves the WorkspaceManager into the streaming
    // task, so hand it an owned clone; `layer` / `boundary` may drop once the
    // synchronous setup returns the channel.
    let wm = boundary.proj_ctx.workspace_manager().clone();
    // No engine cache on this path: `enter_semantic_boundary` is headers-driven
    // and carries no `AppState`. `None` keeps today's per-request build.
    match measure_breakdown_core(
        wm,
        boundary.app.user.id,
        WorkspaceRole::Viewer,
        &layer,
        None,
        q,
    )
    .await
    {
        Ok(rx) => Sse::new(create_sse_stream(rx))
            .keep_alive(KeepAlive::default())
            .into_response(),
        Err((status, body)) => err(status, body.message),
    }
}
