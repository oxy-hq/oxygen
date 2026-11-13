//! The measure breakdown: one instance's measure decomposed along the metric
//! tree, valued per node and streamed.

use agentic_semantic::config::SemanticQueryConfig;
use futures::stream::{FuturesUnordered, StreamExt as _};
use http::StatusCode;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::DiskSlot;

use super::GraphError;
use super::query::*;
use super::types::*;
use super::warehouse::GraphWarehouse;

/// Core of the world-model measure-breakdown (driver-tree) stream — shared by
/// the workspace `get_world_model_measure_breakdown` handler and the
/// customer-app gate handler (`oxy-api-world-model`'s `customer_app::world_model`).
///
/// Builds the breakdown structure, plans + compiles the per-view value
/// queries, then spawns a task that streams `Init → Value* → Done` on the
/// returned channel. `layer` is borrowed (cloned only where the blocking
/// compile needs an owned copy). The terminal `Done` is always emitted once
/// every value query resolves. Errors are the transport-agnostic
/// [`GraphError`] so each caller wraps the body its own way.
///
/// `engine` is the caller's cached engine when it has one. The two callers
/// differ here and cannot share an entry: the workspace handler reads the
/// working copy and holds a `SemanticEngineCacheCtx`, while the customer-app
/// handler enters through `enter_semantic_boundary`, which is headers-driven
/// and has no `AppState` to carry a cache. `None` falls back to building one
/// for this request, which is what both callers did before.
pub async fn measure_breakdown_core<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    warehouse: &dyn GraphWarehouse,
    layer: &oxy_airlayer_compat::SemanticLayer,
    engine: Option<std::sync::Arc<oxy_airlayer_compat::SemanticEngine>>,
    q: WmMeasureBreakdownQuery,
) -> Result<tokio::sync::mpsc::Receiver<WmMeasureBreakdownEvent>, GraphError> {
    let view = primary_view_of(layer, &q.entity).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("Entity '{}' not found", q.entity),
        )
    })?;
    let primary_view = view.name.clone();
    let pk_cols = entity_keys_in_view(view, &q.entity, true);
    let datasource = q
        .datasource
        .clone()
        .or_else(|| view.datasource.clone())
        .unwrap_or_default();
    let root_id = format!("{}.{}", primary_view, q.measure);

    let tree = oxy_semantic::build_metric_tree(layer);
    let (nodes, edges) = breakdown_structure(&tree, &root_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("Measure '{root_id}' not found in metric tree"),
        )
    })?;

    let key_values: Vec<String> =
        serde_json::from_str::<Vec<String>>(&q.key).unwrap_or_else(|_| vec![q.key.clone()]);

    let plan = breakdown_value_plan(
        layer,
        &nodes,
        &q.entity,
        &key_values,
        &pk_cols,
        &primary_view,
    );

    // Compile all view-group SQLs up front (pure, blocking).
    let layer_clone = layer.clone();
    let databases = database_configs(workspace_manager);
    let cfgs: Vec<SemanticQueryConfig> = plan.groups.iter().map(|(_, _, c)| c.clone()).collect();
    let compiled: Vec<Option<String>> = tokio::task::spawn_blocking(move || {
        // The caller's cached engine when it had one; otherwise build for this
        // request (the customer-app caller has no cache to reach — see the
        // doc comment on this function).
        let engine = match engine {
            Some(e) => e,
            None => match oxy_airlayer_compat::build_engine(layer_clone, &databases) {
                Ok(e) => std::sync::Arc::new(e),
                Err(e) => {
                    tracing::warn!(error = %e, "measure-breakdown: engine build failed");
                    return vec![None; cfgs.len()];
                }
            },
        };
        cfgs.iter()
            .map(|cfg| {
                agentic_semantic::compile_with_engine(&engine, cfg)
                    .map_err(|e| {
                        tracing::warn!(error = %e, "measure-breakdown: SQL compile failed");
                    })
                    .ok()
            })
            .collect()
    })
    .await
    .unwrap_or_else(|_| vec![None; plan.groups.len()]);

    let connection = warehouse
        .connect(&datasource)
        .await
        .map_err(|message| (StatusCode::INTERNAL_SERVER_ERROR, message))?;

    let (tx, rx) = tokio::sync::mpsc::channel::<WmMeasureBreakdownEvent>(64);
    let group_node_ids: Vec<Vec<String>> =
        plan.groups.iter().map(|(_, ids, _)| ids.clone()).collect();
    let unvalued = plan.unvalued.clone();

    tokio::spawn(async move {
        tx.send(WmMeasureBreakdownEvent::Init {
            root: root_id,
            nodes,
            edges,
        })
        .await
        .ok();

        for node_id in unvalued {
            tx.send(WmMeasureBreakdownEvent::Value {
                node_id,
                value: None,
                unvalued_reason: Some("no join path to instance".to_string()),
            })
            .await
            .ok();
        }

        // Run each view-group query concurrently; emit one Value per node.
        let mut futs: FuturesUnordered<_> = compiled
            .into_iter()
            .zip(group_node_ids)
            .map(|(sql, node_ids)| {
                let connection = connection.clone();
                async move {
                    let rows = match sql {
                        Some(ref s) => connection.rows(s).await,
                        None => vec![],
                    };
                    (node_ids, rows.into_iter().next().unwrap_or_default())
                }
            })
            .collect();

        while let Some((node_ids, row)) = futs.next().await {
            for (i, node_id) in node_ids.into_iter().enumerate() {
                let value = row.get(i).cloned();
                tx.send(WmMeasureBreakdownEvent::Value {
                    node_id,
                    value,
                    unvalued_reason: None,
                })
                .await
                .ok();
            }
        }

        tx.send(WmMeasureBreakdownEvent::Done).await.ok();
    });

    Ok(rx)
}
