//! The graph itself: every primary entity of the semantic model, its own and
//! induced measures, and the promotion and foreign-key edges between them.

use oxy_airlayer_compat::engine::promotions::Promotions;
use oxy_airlayer_compat::schema::models::{EntityType, MeasureType};

use super::query::*;
use super::types::*;

/// The set of fully-qualified measure ids (`view.measure`) that decompose
/// into a metric-tree driver breakdown — a measure has one when it is the
/// target (`to`) of a component edge. Drives each measure's `has_breakdown`.
fn breakdownable_measures(
    layer: &oxy_airlayer_compat::SemanticLayer,
) -> std::collections::HashSet<String> {
    use oxy_airlayer_compat::engine::metric_tree::EdgeKind;
    let tree = oxy_semantic::build_metric_tree(layer);
    tree.edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Component)
        .map(|e| e.to.clone())
        .collect()
}

/// Build one entity node (and push its promotion / FK edges) for `view`'s
/// primary entity. Returns `None` for a view with no primary entity.
fn build_entity_node(
    view: &oxy_airlayer_compat::schema::models::View,
    layer: &oxy_airlayer_compat::SemanticLayer,
    promotions: &Promotions,
    breakdownable: &std::collections::HashSet<String>,
    edges: &mut Vec<WmEdge>,
) -> Option<WmEntity> {
    let primary = view
        .entities
        .iter()
        .find(|e| e.entity_type == EntityType::Primary)?;
    let entity_name = &primary.name;
    let depth = promotions.ancestry(entity_name).len();

    let dimensions: Vec<WmDimension> = view
        .dimensions
        .iter()
        .map(|d| WmDimension {
            name: d.name.clone(),
            dim_type: format!("{:?}", d.dimension_type).to_lowercase(),
            label: None,
            description: d.description.clone(),
        })
        .collect();

    let own_measures: Vec<WmMeasure> = view
        .measures
        .as_ref()
        .map(|ms| {
            ms.iter()
                .filter(|m| !m.name.starts_with('_'))
                .map(|m| WmMeasure {
                    name: m.name.clone(),
                    measure_type: m.measure_type.clone(),
                    additivity: m.measure_type.additivity_class(),
                    label: None,
                    description: m.description.clone(),
                    expr: m.expr.clone(),
                    has_breakdown: breakdownable.contains(&format!("{}.{}", view.name, m.name)),
                    default_cohort: m.default_cohort.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    let induced_measures: Vec<WmInducedMeasure> = promotions
        .induced_for_view(&view.name)
        .into_iter()
        .filter(|im| !im.source_measure.starts_with('_'))
        .map(|im| {
            let source = layer
                .views
                .iter()
                .find(|v| v.name == im.source_view)
                .and_then(|v| v.measures.as_ref())
                .and_then(|ms| ms.iter().find(|m| m.name == im.source_measure));
            WmInducedMeasure {
                name: im.source_measure.clone(),
                measure_type: source
                    .map(|m| m.measure_type.clone())
                    .unwrap_or(MeasureType::Custom),
                additivity: im.additivity,
                label: None,
                description: source.and_then(|m| m.description.clone()),
                expr: source.and_then(|m| m.expr.clone()),
                promoted_from: im.source_view.clone(),
                path: im.path.clone(),
                default_cohort: source.and_then(|m| m.default_cohort.clone()),
            }
        })
        .collect();

    if let Some(parent) = promotions.parent_of(entity_name) {
        edges.push(WmEdge {
            from: entity_name.clone(),
            to: parent.to_string(),
            functional: true,
        });
    }

    // FK cross-reference edges: Foreign entity declarations signal a join
    // relationship without a promotion hierarchy. Emit dashed edges so the
    // graph shows these structural cross-links alongside solid parent edges.
    for foreign in view
        .entities
        .iter()
        .filter(|e| e.entity_type == EntityType::Foreign)
    {
        let fk_target = &foreign.name;
        // Skip if already covered by the parent edge (avoids parallel edges).
        if promotions.parent_of(entity_name) == Some(fk_target.as_str()) {
            continue;
        }
        // Only draw an edge if the target is also a primary entity somewhere
        // in the layer (otherwise there's no node to connect to).
        let target_exists = layer.views.iter().any(|v| {
            v.entities
                .iter()
                .any(|e| e.entity_type == EntityType::Primary && e.name == *fk_target)
        });
        if target_exists {
            edges.push(WmEdge {
                from: entity_name.clone(),
                to: fk_target.clone(),
                functional: false,
            });
        }
    }

    Some(WmEntity {
        id: entity_name.clone(),
        label: entity_name.clone(),
        view: view.name.clone(),
        description: view.description.clone(),
        depth,
        display_field: None,
        dimensions,
        own_measures,
        induced_measures,
    })
}

/// Assemble the world-model graph (entity nodes + promotion/FK edges) from a
/// parsed semantic model, apply the `.world-model.yml` display config, and
/// sort by depth.
///
/// Shared by the workspace-scoped `get_world_model` handler and the
/// customer-app gate handler
/// (`oxy-api-world-model`'s `customer_app::world_model`) — they differ only in the
/// gate they enter through; both read the layer from the compile boundary.
pub fn build_world_model_response(
    layer: &oxy_airlayer_compat::SemanticLayer,
    config: Option<&crate::WorldModelConfig>,
) -> Result<WorldModelResponse, String> {
    let promotions = Promotions::build(&layer.views)
        .map_err(|e| format!("Failed to build promotion closure: {e}"))?;
    let breakdownable = breakdownable_measures(layer);

    let mut entities: Vec<WmEntity> = Vec::new();
    let mut edges: Vec<WmEdge> = Vec::new();
    for view in &layer.views {
        if let Some(entity) =
            build_entity_node(view, layer, &promotions, &breakdownable, &mut edges)
        {
            entities.push(entity);
        }
    }

    // Apply the .world-model.yml display config when the workspace has one
    // (filter + label overrides). The caller resolved it, because a read that
    // failed is its to answer — see `WorldModelConfig::resolve`.
    if let Some(cfg) = config {
        apply_world_model_config(&mut entities, &mut edges, cfg);
    }

    entities.sort_by_key(|e| e.depth);
    Ok(WorldModelResponse { entities, edges })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-view layer whose single measure may or may not declare a cohort.
    /// Built by deserializing rather than by hand: `View`/`Measure` carry tens
    /// of optional fields and a literal would say nothing about which matter.
    fn layer_with(default_cohort: Option<&str>) -> oxy_airlayer_compat::SemanticLayer {
        let mut measure = serde_json::json!({ "name": "wage_pct", "type": "sum" });
        if let Some(c) = default_cohort {
            measure["default_cohort"] = serde_json::json!(c);
        }
        serde_json::from_value(serde_json::json!({
            "views": [{
                "name": "sales",
                "entities": [{ "name": "store_id", "type": "primary", "key": "store_id" }],
                "measures": [measure],
            }],
        }))
        .expect("fixture layer should deserialize")
    }

    fn own_measures_of(layer: &oxy_airlayer_compat::SemanticLayer) -> Vec<WmMeasure> {
        let promotions = Promotions::build(&layer.views).expect("fixture promotions should build");
        let mut edges = Vec::new();
        build_entity_node(
            &layer.views[0],
            layer,
            &promotions,
            &std::collections::HashSet::new(),
            &mut edges,
        )
        .expect("fixture view has a primary entity")
        .own_measures
    }

    /// The World Model panel decides whether to offer "Compared to peers" from
    /// the graph alone — it has no other way to know a cohort exists without
    /// spending two entity-grain warehouse pulls to be told there is none.
    #[test]
    fn a_measures_declared_cohort_reaches_the_graph() {
        let layer = layer_with(Some("store_id.size_matched"));
        let measures = own_measures_of(&layer);
        assert_eq!(measures.len(), 1);
        assert_eq!(
            measures[0].default_cohort.as_deref(),
            Some("store_id.size_matched")
        );
    }

    /// The common case today, and the one the toggle must not render for.
    #[test]
    fn a_measure_without_a_cohort_carries_none() {
        let layer = layer_with(None);
        assert_eq!(own_measures_of(&layer)[0].default_cohort, None);
    }
}
