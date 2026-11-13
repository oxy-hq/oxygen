//! `/api/projects/{project_id}/semantic/world-model*` — world-model graph
//! + instances for customer-app bundles.
//!
//! Customer-app-gated variants of the IDE's workspace-scoped
//! `/semantic/world-model*` handlers. They enter through the customer-app
//! gate and load the semantic model from the compile boundary (via
//! [`super::semantic_boundary`]) so they run on the stateless serve fleet.
//! The graph-assembly, instance-listing and breakdown cores are shared with
//! the workspace handlers in [`crate::server::api::world_model_graph`]
//! (both answer from `oxy_world_model::graph`) — only the layer/path
//! acquisition differs. What is this route's alone is the operating graph:
//! a listing scoped to the viewer's reach, and the places its instances map
//! to ([`GraphScope`]).

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
use crate::server::api::world_model_graph::WorkspaceWarehouse;
use agentic_semantic::config::{SemanticFilter, SemanticFilterType};
use oxy_world_model::graph::{
    WmInstanceItem, WmInstanceLocation, WmInstancesQuery, WmMeasureBreakdownQuery,
    build_world_model_response, instances_core, instances_view, measure_breakdown_core,
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
    let layer = match load_layer(boundary.scan.clone()).await {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let config_manager = &boundary.proj_ctx.workspace_manager().config_manager;
    let config = match display_config(config_manager, project_id).await {
        Ok(config) => config,
        Err(resp) => return resp,
    };
    match build_world_model_response(&layer, config.as_ref()) {
        Ok(resp) => cache_store(&boundary, "wm-graph", "", &resp),
        Err(message) => err_with_code(
            StatusCode::INTERNAL_SERVER_ERROR,
            message,
            "world_model_failed",
        ),
    }
}

/// What a bundle is told when the display config could not be read. The read
/// error stays in the log: it can carry a database error or a path on this
/// node, and this is the public custom-app router.
const CONFIG_UNAVAILABLE: &str =
    "the world-model display config could not be read just now; retry shortly";

/// The `.world-model.yml` display config, or the response that refuses.
///
/// The manager knows whether there is a working copy to fall back to. On a
/// replica the pinned revision is the only source: no row there is "no
/// display overrides", and a read that failed is a retryable 503 — never the
/// unfiltered graph.
async fn display_config<S: oxy::config::DiskSlot>(
    config_manager: &oxy::config::ConfigManager<S>,
    project_id: Uuid,
) -> Result<Option<oxy_world_model::WorldModelConfig>, Response> {
    use oxy_world_model::WorldModelConfigError::{Invalid, Unavailable};
    match oxy_world_model::WorldModelConfig::resolve(config_manager).await {
        Ok(config) => Ok(config),
        Err(Unavailable(e)) => {
            tracing::warn!(%project_id, error = %e, "world-model display config could not be read");
            Err(err_with_code(
                StatusCode::SERVICE_UNAVAILABLE,
                CONFIG_UNAVAILABLE,
                "world_model_unavailable",
            ))
        }
        // The workspace's own YAML, and the message this route always gave.
        Err(Invalid(e)) => Err(err_with_code(
            StatusCode::INTERNAL_SERVER_ERROR,
            e,
            "world_model_failed",
        )),
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
    let layer = match load_layer(boundary.scan.clone()).await {
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
    let graph = GraphScope {
        db: boundary.app.db.clone(),
        org_id: boundary.app.org_id,
        reach,
    };
    // The core's refusals first (unknown entity, no table, no key), then the
    // reach's: a scoped request on an entity that is not bound is refused.
    let scope_filters = match instances_view(&layer, &q.entity) {
        Ok(view) => match reach_filter(&graph, view, &q.entity).await {
            Ok(filters) => filters,
            Err((status, message)) => return err(status, message),
        },
        Err((status, message)) => return err(status, message),
    };
    let workspace_manager = boundary.proj_ctx.workspace_manager();
    let warehouse = WorkspaceWarehouse {
        workspace_manager: workspace_manager.clone(),
        user_id: boundary.app.user.id,
        role: WorkspaceRole::Viewer,
    };
    let mut resp = match instances_core(
        workspace_manager,
        &warehouse,
        &layer,
        boundary.scan.clone(),
        // No engine cache on this path: `enter_semantic_boundary` is
        // headers-driven and carries no `AppState`.
        None,
        &q,
        scope_filters,
    )
    .await
    {
        Ok(resp) => resp,
        Err((status, message)) => return err(status, message),
    };
    if let Ok(view) = instances_view(&layer, &q.entity) {
        attach_places(&graph, view, &mut resp.items).await;
    }
    Json(resp).into_response()
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
    let layer = match load_layer(boundary.scan.clone()).await {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    // The core opens its connection before it returns the channel, so
    // `layer` / `boundary` may drop once the synchronous setup returns.
    let workspace_manager = boundary.proj_ctx.workspace_manager();
    let warehouse = WorkspaceWarehouse {
        workspace_manager: workspace_manager.clone(),
        user_id: boundary.app.user.id,
        role: WorkspaceRole::Viewer,
    };
    // No engine cache on this path: `enter_semantic_boundary` is headers-driven
    // and carries no `AppState`. `None` keeps today's per-request build.
    match measure_breakdown_core(workspace_manager, &warehouse, &layer, None, q).await {
        Ok(rx) => Sse::new(create_sse_stream(rx))
            .keep_alive(KeepAlive::default())
            .into_response(),
        Err((status, message)) => err(status, message),
    }
}

/// The operating graph, as an instances request sees it: the org whose
/// registry a bound entity resolves against, a handle to read it, and the
/// viewer's reach when the listing is scoped. The customer-app route
/// supplies it from the gate context it already holds; the workspace route
/// has none, so the IDE's instance list is the warehouse's view of the world
/// and carries no places.
pub(crate) struct GraphScope {
    pub db: sea_orm::DatabaseConnection,
    pub org_id: uuid::Uuid,
    /// `Some` for `scope=reach`. An everywhere reach adds no filter.
    pub reach: Option<crate::server::api::operating_graph::reach::Reach>,
}

fn bound_primary(
    view: &oxy_airlayer_compat::View,
) -> Option<(String, oxy_airlayer_compat::EntityBinding)> {
    let e = view.entities.iter().find(|e| {
        matches!(
            e.entity_type,
            oxy_airlayer_compat::schema::models::EntityType::Primary
        )
    })?;
    let binding = oxy_airlayer_compat::entity_binding(e)?;
    let key = e.get_keys().into_iter().next()?;
    Some((key, binding))
}

/// The `in` filter that pins a scoped listing to the viewer's places: over
/// the bound key, with the keys their places carry in the bound system, or
/// one value no key equals. A scoped request on an entity that is not bound
/// is refused — the same rule `semantic.query` applies — rather than
/// answered whole.
async fn reach_filter(
    graph: &GraphScope,
    view: &oxy_airlayer_compat::View,
    entity: &str,
) -> Result<Vec<SemanticFilter>, (StatusCode, String)> {
    use crate::server::api::operating_graph::binding::{
        NO_REACH_SENTINEL, external_ids_for_locations,
    };
    let Some(reach) = graph.reach.as_ref() else {
        return Ok(vec![]);
    };
    let Some((key, binding)) = bound_primary(view) else {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("scope `reach` needs entity '{entity}' bound to the locations registry"),
        ));
    };
    if reach.everywhere {
        return Ok(vec![]);
    }
    let mut keys =
        external_ids_for_locations(&graph.db, graph.org_id, &binding.system, &reach.locations)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("reach could not be resolved: {e}"),
                )
            })?;
    if keys.is_empty() {
        keys.push(NO_REACH_SENTINEL.to_string());
    }
    Ok(vec![SemanticFilter {
        field: format!("{}.{}", view.name, key),
        filter_type: SemanticFilterType::In(agentic_semantic::config::ArrayFilter {
            values: keys.into_iter().map(serde_json::Value::String).collect(),
        }),
    }])
}

/// When the entity is bound to the locations registry, name each instance's
/// place. Best-effort: a lookup that fails leaves the items as they were,
/// because a list of stores is still a list of stores without the mapping.
async fn attach_places(
    graph: &GraphScope,
    view: &oxy_airlayer_compat::View,
    items: &mut [WmInstanceItem],
) {
    use crate::server::api::operating_graph::binding::locations_by_external_ids;
    let Some((_, binding)) = bound_primary(view) else {
        return;
    };
    if items.is_empty() {
        return;
    }
    let keys: Vec<String> = items.iter().map(|i| i.key.clone()).collect();
    match locations_by_external_ids(&graph.db, graph.org_id, &binding.system, &keys).await {
        Ok(places) => {
            for item in items.iter_mut() {
                item.location = places.get(&item.key).map(|p| WmInstanceLocation {
                    id: p.id,
                    name: p.name.clone(),
                    kind: p.kind.clone(),
                    parent_id: p.parent_id,
                });
            }
        }
        Err(e) => tracing::warn!(error = %e, "world model instances: places not resolved"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy::adapters::workspace::builder::WorkspaceBuilder;

    /// The refusal is on the public custom-app router. It carries the code a
    /// bundle matches on, its own words rather than the read error's (which
    /// names a path here and a database error on a replica), and no header
    /// that would let a cache keep it.
    #[tokio::test]
    async fn an_unreadable_config_is_a_503_in_the_routes_own_words_and_not_cacheable() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
            .expect("config");
        // A directory where the file should be: present, and unreadable as a
        // file whoever runs the test.
        std::fs::create_dir(dir.path().join(".world-model.yml")).expect("mkdir");
        let id = Uuid::new_v4();
        let manager = WorkspaceBuilder::new(id)
            .with_working_copy(dir.path(), None, oxy::config::OnMissing::Empty)
            .await
            .expect("builder")
            .build()
            .await
            .expect("manager");

        let refusal = display_config(&manager.config_manager, id)
            .await
            .expect_err("a read that failed is not `None`");

        assert_eq!(refusal.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            !refusal
                .headers()
                .contains_key(axum::http::header::CACHE_CONTROL)
        );
        let body = axum::body::to_bytes(refusal.into_body(), usize::MAX)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(body["code"], "world_model_unavailable", "{body}");
        assert_eq!(body["message"], CONFIG_UNAVAILABLE, "{body}");
    }
}
