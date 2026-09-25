use oxy_airlayer_compat::engine::promotions::Promotions;
use oxy_airlayer_compat::schema::models::{EntityType, MeasureType};
use std::collections::HashMap;

use agentic_semantic::compile::{CompiledQuery, resolve_and_compile_cached};
use agentic_semantic::config::{
    ArrayFilter, ScalarFilter, SemanticFilter, SemanticFilterType, SemanticQueryConfig,
};
use entity::workspace_members::WorkspaceRole;
use oxy::adapters::workspace::manager::WorkspaceManager;
use uuid::Uuid;

use crate::server::api::data::{build_connector, run_with_connector};

use super::types::*;
use oxy::config::WorkingCopy;

/// Load the semantic model and build its promotion closure — the preamble the
/// world-model handlers otherwise repeat verbatim (`semantics_scan_path →
/// get_or_load → Promotions::build`). Returns the transport error tuple ready to
/// `?`-propagate from a handler so the load path lives in one place.
pub(super) async fn load_layer_and_promotions(
    workspace_manager: &WorkspaceManager<WorkingCopy>,
    layer_cache: &crate::server::api::middlewares::workspace_context::SemanticLayerCacheCtx,
) -> Result<
    (
        std::sync::Arc<oxy_airlayer_compat::SemanticLayer>,
        Promotions,
    ),
    (
        axum::http::StatusCode,
        axum::extract::Json<crate::server::api::semantic::ErrorResponse>,
    ),
> {
    let semantics_path = workspace_manager.config_manager.semantics_scan_path();
    // `semantics_path` is `semantics_scan_path()` — this family scans the
    // working copy unconditionally, so it names that source rather than the
    // revision the request happens to be pinned to.
    let layer = layer_cache
        .get_or_load(None, semantics_path)
        .await
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                axum::extract::Json(crate::server::api::semantic::ErrorResponse {
                    message: e.to_string(),
                }),
            )
        })?;
    let promotions = Promotions::build(&layer.views).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::extract::Json(crate::server::api::semantic::ErrorResponse {
                message: e.to_string(),
            }),
        )
    })?;
    Ok((layer, promotions))
}

/// Resolve the `.world-model.yml` display config, tolerating a missing or
/// unreadable config (`None`). Compile-boundary first (serve replicas have no
/// working copy), FS fallback — see [`WorldModelConfig::resolve`]. The
/// world-model handlers otherwise repeat this `resolve(..).ok().flatten()`
/// incantation verbatim, so it lives here in one place.
pub(super) async fn resolve_world_model_config(
    workspace_manager: &WorkspaceManager<WorkingCopy>,
) -> Option<oxy_world_model::world_model_config::WorldModelConfig> {
    oxy_world_model::world_model_config::WorldModelConfig::resolve(
        &workspace_manager.config_manager,
    )
    .await
    .ok()
    .flatten()
}

// ── World Model — SQL helpers ─────────────────────────────────────────────────

/// A cached engine handle and the exact `databases` its key was fingerprinted
/// from.
///
/// The two travel together because deriving them separately is a silent bug:
/// `EngineKey`'s fingerprint DESCRIBES a database list, so building an engine
/// from a different list caches it under a fingerprint that does not match it.
/// The compiler cannot catch that, and the symptom is a query compiled in the
/// wrong dialect — so the type makes them one value instead.
#[derive(Clone)]
pub(crate) struct CachedEngine {
    pub(crate) cache: std::sync::Arc<crate::server::router::workspace_cache::SemanticEngineCache>,
    pub(crate) key: oxy_airlayer_compat::EngineKey,
    pub(crate) databases: Vec<oxy_airlayer_compat::DatabaseConfig>,
}

impl CachedEngine {
    /// For a caller reading this node's working copy.
    pub(crate) fn working_copy<S: oxy::config::DiskSlot>(
        engine_cache: &crate::server::api::middlewares::workspace_context::SemanticEngineCacheCtx,
        workspace_manager: &WorkspaceManager<S>,
    ) -> Self {
        let databases = database_configs(workspace_manager);
        Self {
            cache: engine_cache.cache.clone(),
            key: engine_cache.working_copy_key(&databases),
            databases,
        }
    }
}

/// The whole workspace's databases, as the dialect map wants them.
///
/// The per-entry rule — pass `dialect()`, never the raw `type:` string — lives
/// on `oxy_airlayer_compat::database_config`, which this defers to. What this
/// adds is the LIST: it was transcribed at six call sites in this module, each
/// its own chance to forget one.
pub(crate) fn database_configs<S: oxy::config::DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
    workspace_manager
        .config_manager
        .list_databases()
        .iter()
        .map(|db| oxy_airlayer_compat::database_config(db.name.clone(), db.dialect()))
        .collect()
}

/// Find the view where `entity_name` is declared as Primary.
pub(super) fn primary_view_of<'a>(
    layer: &'a oxy_airlayer_compat::SemanticLayer,
    entity_name: &str,
) -> Option<&'a oxy_airlayer_compat::View> {
    layer.views.iter().find(|v| {
        v.entities
            .iter()
            .any(|e| e.entity_type == EntityType::Primary && e.name == entity_name)
    })
}

/// Get key columns for `entity_name` in `view`.
/// `is_primary`: true = Primary declaration, false = Foreign declaration.
/// Returns logical dimension names (use `entity_key_exprs_in_view` for SQL).
pub(super) fn entity_keys_in_view(
    view: &oxy_airlayer_compat::View,
    entity_name: &str,
    is_primary: bool,
) -> Vec<String> {
    view.entities
        .iter()
        .find(|e| {
            e.name == entity_name
                && if is_primary {
                    e.entity_type == EntityType::Primary
                } else {
                    e.entity_type == EntityType::Foreign
                }
        })
        .map(|e| e.get_keys())
        .unwrap_or_default()
}

/// Build per-column `IN` filters for a child entity's foreign key from the
/// matched PK rows of its parent.
///
/// `fk_dim_refs[col]` is matched positionally against column `col` of every
/// row in `parent_pk_rows`. A column is emitted only when at least one parent
/// row actually supplies a value for it.
///
/// The skip matters when the parent identifies its instances by fewer columns
/// than the child's composite FK — e.g. an `order_item` seed coming from the
/// instance picker carries only its first PK column (`order_id`), while the
/// `shipment` child references `order_item` by the composite (`order_id`,
/// `line_item_id`). Emitting a filter for the missing `line_item_id` column
/// would produce an empty `IN ()` that matches nothing, zeroing the reachable
/// count. Skipping it filters on what the seed actually constrains, matching
/// the instance-detail child count (which filters on the first FK column only).
pub(super) fn child_fk_filters(
    fk_dim_refs: &[String],
    parent_pk_rows: &[Vec<serde_json::Value>],
) -> Vec<agentic_semantic::config::SemanticFilter> {
    fk_dim_refs
        .iter()
        .enumerate()
        .filter_map(|(col, fk_ref)| {
            let values: Vec<serde_json::Value> = parent_pk_rows
                .iter()
                .filter_map(|row| row.get(col).cloned())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            if values.is_empty() {
                return None;
            }
            Some(agentic_semantic::config::SemanticFilter {
                field: fk_ref.clone(),
                filter_type: agentic_semantic::config::SemanticFilterType::In(
                    agentic_semantic::config::ArrayFilter { values },
                ),
            })
        })
        .collect()
}

/// One navigable relationship an entity participates in, used by the instance
/// drill-down traversal. Direction is implicit: the entity that owns this link
/// is the *finer* (child / fan-out) side and references `target_entity`'s PK via
/// `fk_dim_refs`, so `target_entity` is the coarser side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EntityLink {
    /// The entity on the other end of the relationship.
    pub(super) target_entity: String,
    /// "{view}.{fk_col}" refs on the child (self) side pointing at `target`'s PK.
    pub(super) fk_dim_refs: Vec<String>,
    /// Whether this is the solid `parent:` spine or a dashed foreign cross-link.
    pub(super) kind: LinkKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LinkKind {
    /// The `parent:` declaration — the single solid hierarchy edge.
    Parent,
    /// A Foreign entity that resolves to a Primary elsewhere — a dashed edge.
    CrossLink,
}

/// Build the navigable link set for the Primary entity of `view`.
///
/// This is the union of the two relationship sources that already feed the
/// *drawn* edges (`world_model_graph` edge builder): the `parent:` spine and
/// every Foreign entity whose name resolves to a Primary entity elsewhere in
/// the layer. The parent is itself always declared as a Foreign entity (it must
/// be, to carry the FK column), so iterating Foreign declarations captures both;
/// the parent one is tagged `LinkKind::Parent`, the rest `CrossLink`.
///
/// The resulting navigable graph is exactly the drawn graph — no edge appears in
/// the traversal that the user can't already see.
pub(super) fn build_entity_links(
    views: &[oxy_airlayer_compat::View],
    view: &oxy_airlayer_compat::View,
    parent_entity: Option<&str>,
) -> Vec<EntityLink> {
    view.entities
        .iter()
        .filter(|e| e.entity_type == EntityType::Foreign)
        .filter_map(|foreign| {
            // Only navigable if the target is a Primary entity somewhere.
            let target_exists = views.iter().any(|v| {
                v.entities
                    .iter()
                    .any(|e| e.entity_type == EntityType::Primary && e.name == foreign.name)
            });
            if !target_exists {
                return None;
            }
            let fk_dim_refs: Vec<String> = foreign
                .get_keys()
                .into_iter()
                .map(|k| format!("{}.{}", view.name, k))
                .collect();
            if fk_dim_refs.is_empty() {
                return None;
            }
            let kind = if parent_entity == Some(foreign.name.as_str()) {
                LinkKind::Parent
            } else {
                LinkKind::CrossLink
            };
            Some(EntityLink {
                target_entity: foreign.name.clone(),
                fk_dim_refs,
                kind,
            })
        })
        .collect()
}

/// Per-entity metadata needed to build the instance drill-down semantic
/// queries. Hoisted out of the filter-counts handler so the expansion-plan
/// helpers below can be unit-tested against it.
pub(super) struct EntityMeta {
    pub(super) entity_name: String,
    pub(super) view_name: String,
    pub(super) datasource: String,
    /// "{view}.{pk_dim}" refs — used in SemanticFilter fields and dimension selects.
    pub(super) pk_dim_refs: Vec<String>,
    /// Navigable relationships this entity participates in (parent spine +
    /// foreign cross-links). Drives the undirected instance-drill-down BFS.
    pub(super) links: Vec<EntityLink>,
    /// Dims to SELECT for a sample row: PK dims first, then label dim if any.
    pub(super) sample_dims: Vec<String>,
    pub(super) pk_count: usize,
    pub(super) has_label_dim: bool,
}

/// Reverse adjacency over the navigable link graph: target entity → its
/// *finer* neighbours, i.e. (child index, the child's FK refs pointing at
/// this target). This is the inbound direction the `parent:` spine alone
/// never provides for cross-links (e.g. `store ← order`). Built once and
/// shared by every traversal (schema reachability, the direct-join fast
/// path, and the cross-datasource legacy BFS fallback) instead of being
/// recomputed inline at each call site.
pub(super) fn build_inbound_index(
    entity_metas: &[EntityMeta],
) -> HashMap<&str, Vec<(usize, &[String])>> {
    let mut inbound: HashMap<&str, Vec<(usize, &[String])>> = HashMap::new();
    for (i, meta) in entity_metas.iter().enumerate() {
        for link in &meta.links {
            inbound
                .entry(link.target_entity.as_str())
                .or_default()
                .push((i, link.fk_dim_refs.as_slice()));
        }
    }
    inbound
}

/// Every entity index reachable from `seed_idx` via the undirected link graph
/// (parent spine + FK cross-links, both directions) — the same edges drawn in
/// the graph UI. This is a static property of the `.view.yml` relationships
/// alone, so unlike matched-row reachability it needs no IO and can be
/// computed once up front instead of discovered hop-by-hop at query time.
pub(super) fn schema_reachable_entities(
    entity_metas: &[EntityMeta],
    inbound: &HashMap<&str, Vec<(usize, &[String])>>,
    meta_idx: &HashMap<&str, usize>,
    seed_idx: usize,
) -> Vec<usize> {
    let mut visited: std::collections::HashSet<usize> = std::collections::HashSet::from([seed_idx]);
    let mut stack = vec![seed_idx];
    let mut result = Vec::new();
    while let Some(idx) = stack.pop() {
        for link in &entity_metas[idx].links {
            if let Some(&t) = meta_idx.get(link.target_entity.as_str())
                && visited.insert(t)
            {
                result.push(t);
                stack.push(t);
            }
        }
        if let Some(children) = inbound.get(entity_metas[idx].entity_name.as_str()) {
            for &(child_idx, _) in children {
                if visited.insert(child_idx) {
                    result.push(child_idx);
                    stack.push(child_idx);
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod schema_reachability_tests {
    use super::*;

    fn meta(name: &str, datasource: &str, links: Vec<EntityLink>) -> EntityMeta {
        EntityMeta {
            entity_name: name.to_string(),
            view_name: name.to_string(),
            datasource: datasource.to_string(),
            pk_dim_refs: vec![format!("{name}.id")],
            links,
            sample_dims: vec![format!("{name}.id")],
            pk_count: 1,
            has_label_dim: false,
        }
    }

    fn link(target: &str, kind: LinkKind) -> EntityLink {
        EntityLink {
            target_entity: target.to_string(),
            fk_dim_refs: vec![format!("self.{target}_id")],
            kind,
        }
    }

    fn idx(metas: &[EntityMeta], name: &str) -> usize {
        metas.iter().position(|m| m.entity_name == name).unwrap()
    }

    // seed -> parent (outbound), child -> seed (inbound cross-link): both
    // neighbours must be reachable regardless of edge direction.
    #[test]
    fn reaches_both_outbound_and_inbound_neighbours() {
        let metas = vec![
            meta("seed", "db1", vec![link("parent", LinkKind::Parent)]),
            meta("parent", "db1", vec![]),
            meta("child", "db1", vec![link("seed", LinkKind::CrossLink)]),
        ];
        let meta_idx: HashMap<&str, usize> = metas
            .iter()
            .enumerate()
            .map(|(i, m)| (m.entity_name.as_str(), i))
            .collect();
        let inbound = build_inbound_index(&metas);
        let seed_idx = idx(&metas, "seed");
        let mut reachable = schema_reachable_entities(&metas, &inbound, &meta_idx, seed_idx);
        reachable.sort();
        let mut expected = vec![idx(&metas, "parent"), idx(&metas, "child")];
        expected.sort();
        assert_eq!(reachable, expected);
    }

    // A chain seed -> a -> b -> c must all be reachable, however deep —
    // schema reachability doesn't stop at direct neighbours.
    #[test]
    fn reaches_transitively_through_a_chain() {
        let metas = vec![
            meta("seed", "db1", vec![link("a", LinkKind::Parent)]),
            meta("a", "db1", vec![link("b", LinkKind::Parent)]),
            meta("b", "db1", vec![link("c", LinkKind::Parent)]),
            meta("c", "db1", vec![]),
        ];
        let meta_idx: HashMap<&str, usize> = metas
            .iter()
            .enumerate()
            .map(|(i, m)| (m.entity_name.as_str(), i))
            .collect();
        let inbound = build_inbound_index(&metas);
        let seed_idx = idx(&metas, "seed");
        let mut reachable = schema_reachable_entities(&metas, &inbound, &meta_idx, seed_idx);
        reachable.sort();
        let mut expected = vec![idx(&metas, "a"), idx(&metas, "b"), idx(&metas, "c")];
        expected.sort();
        assert_eq!(reachable, expected);
    }

    // An entity with no path to the seed at all is not reachable.
    #[test]
    fn excludes_disconnected_entities() {
        let metas = vec![
            meta("seed", "db1", vec![link("a", LinkKind::Parent)]),
            meta("a", "db1", vec![]),
            meta("island", "db1", vec![]),
        ];
        let meta_idx: HashMap<&str, usize> = metas
            .iter()
            .enumerate()
            .map(|(i, m)| (m.entity_name.as_str(), i))
            .collect();
        let inbound = build_inbound_index(&metas);
        let seed_idx = idx(&metas, "seed");
        let reachable = schema_reachable_entities(&metas, &inbound, &meta_idx, seed_idx);
        assert_eq!(reachable, vec![idx(&metas, "a")]);
    }

    // The seed's own index is never included in its reachable set.
    #[test]
    fn excludes_seed_itself() {
        let metas = vec![
            meta("seed", "db1", vec![link("a", LinkKind::Parent)]),
            meta("a", "db1", vec![]),
        ];
        let meta_idx: HashMap<&str, usize> = metas
            .iter()
            .enumerate()
            .map(|(i, m)| (m.entity_name.as_str(), i))
            .collect();
        let inbound = build_inbound_index(&metas);
        let seed_idx = idx(&metas, "seed");
        let reachable = schema_reachable_entities(&metas, &inbound, &meta_idx, seed_idx);
        assert!(!reachable.contains(&seed_idx));
    }
}

/// Column layout of an entity's single "expansion" query, which selects its PK
/// columns, each outbound link's FK column, and its label column **in one shot**
/// (all functionally determined by the PK, so grouping by them doesn't change
/// row cardinality). One query then yields everything a BFS hop needs: the
/// matched count, the PK rows for inbound children, the FK values for outbound
/// targets, and the display sample — replacing the old separate count + pk +
/// fk-select + sample queries.
pub(super) struct WmExpansionPlan {
    /// Dimensions to SELECT, in column order.
    pub(super) dims: Vec<String>,
    /// Column index of each PK dimension (in `pk_dim_refs` order).
    pub(super) pk_cols: Vec<usize>,
    /// Column index of the label dimension, if the entity has one.
    pub(super) label_col: Option<usize>,
    /// (target entity, column index of that link's first FK column).
    pub(super) link_cols: Vec<(String, usize)>,
}

/// Build the expansion column layout for `meta`. De-duplicates dimensions that
/// coincide (e.g. a PK column reused as an FK) via a first-seen index map.
pub(super) fn wm_expansion_plan(meta: &EntityMeta) -> WmExpansionPlan {
    let mut dims: Vec<String> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    fn col_of(r: &str, dims: &mut Vec<String>, index: &mut HashMap<String, usize>) -> usize {
        if let Some(&i) = index.get(r) {
            return i;
        }
        let i = dims.len();
        dims.push(r.to_string());
        index.insert(r.to_string(), i);
        i
    }
    let pk_cols: Vec<usize> = meta
        .pk_dim_refs
        .iter()
        .map(|r| col_of(r, &mut dims, &mut index))
        .collect();
    let link_cols: Vec<(String, usize)> = meta
        .links
        .iter()
        .filter_map(|l| {
            l.fk_dim_refs
                .first()
                .map(|r| (l.target_entity.clone(), col_of(r, &mut dims, &mut index)))
        })
        .collect();
    // The label dim (if any) is the last entry of `sample_dims` (PK dims first).
    let label_col = if meta.has_label_dim {
        meta.sample_dims
            .last()
            .map(|r| col_of(r, &mut dims, &mut index))
    } else {
        None
    };
    WmExpansionPlan {
        dims,
        pk_cols,
        label_col,
        link_cols,
    }
}

/// Result of executing one entity's expansion query.
pub(super) struct WmExpansionResult {
    /// Distinct-PK count — the node's `matched` value.
    pub(super) matched: u64,
    /// Distinct PK rows (all columns), for building inbound-child FK filters.
    pub(super) pk_rows: Vec<Vec<serde_json::Value>>,
    /// Per outbound link: (target entity, that link's distinct FK values).
    pub(super) fk_values: Vec<(String, Vec<serde_json::Value>)>,
    /// Up to 3 display strings and their nav keys.
    pub(super) sample: Vec<String>,
    pub(super) sample_keys: Vec<String>,
}

/// Parse the rows of an expansion query into a [`WmExpansionResult`] using the
/// column layout from [`wm_expansion_plan`]. Pure — no IO — so it is unit-tested
/// directly. `matched` counts **distinct** PK tuples (an FK fanning out to
/// several rows per instance never inflates the count).
pub(super) fn parse_expansion_rows(
    rows: &[Vec<String>],
    plan: &WmExpansionPlan,
) -> WmExpansionResult {
    let project = |row: &[String], cols: &[usize]| -> Vec<String> {
        cols.iter()
            .map(|&c| row.get(c).cloned().unwrap_or_default())
            .collect()
    };

    let mut seen: std::collections::HashSet<Vec<String>> = std::collections::HashSet::new();
    let mut pk_rows: Vec<Vec<serde_json::Value>> = Vec::new();
    for row in rows {
        let key = project(row, &plan.pk_cols);
        if seen.insert(key.clone()) {
            pk_rows.push(key.into_iter().map(serde_json::Value::String).collect());
        }
    }
    let matched = pk_rows.len() as u64;

    let fk_values: Vec<(String, Vec<serde_json::Value>)> = plan
        .link_cols
        .iter()
        .map(|(target, col)| {
            let vals: Vec<serde_json::Value> = rows
                .iter()
                .filter_map(|r| r.get(*col).cloned())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .map(serde_json::Value::String)
                .collect();
            (target.clone(), vals)
        })
        .collect();

    let pk_count = plan.pk_cols.len();
    let has_label = plan.label_col.is_some();
    // Dedup by PK tuple before taking the first 3 so a single instance fanning
    // out to several FK rows never wastes a preview slot (mirrors `seen`/`pk_rows`).
    let mut sample_seen: std::collections::HashSet<Vec<String>> = std::collections::HashSet::new();
    let (sample, sample_keys): (Vec<String>, Vec<String>) = rows
        .iter()
        .filter(|&row| sample_seen.insert(project(row, &plan.pk_cols)))
        .take(3)
        .map(|row| {
            let mut proj = project(row, &plan.pk_cols);
            if let Some(lc) = plan.label_col {
                proj.push(row.get(lc).cloned().unwrap_or_default());
            }
            sample_row_to_display_key(&proj, pk_count, has_label)
        })
        .unzip();

    WmExpansionResult {
        matched,
        pk_rows,
        fk_values,
        sample,
        sample_keys,
    }
}

/// Centralises the "what to SELECT and how to render a display string" logic for an entity.
///
/// Rule: if `label: <dim>` is declared, use that dimension; otherwise join all PK
/// columns with " · ".  Used by the instance picker, instance detail (init, parent,
/// child samples) so the behaviour is identical everywhere.
pub(super) struct EntityDisplaySpec {
    /// View-qualified dimension refs to SELECT (all PKs first, then label dim if any).
    pub(super) dims: Vec<String>,
    /// Number of leading PK columns in `dims`.
    pub(super) pk_count: usize,
    /// Whether a label dim was appended after the PK columns.
    pub(super) has_label_dim: bool,
    /// Unqualified label dim name (for attr-map lookup).
    pub(super) label_name: Option<String>,
    /// Unqualified PK col names (for attr-map and search expressions).
    pub(super) pk_names: Vec<String>,
}

impl EntityDisplaySpec {
    pub(super) fn for_entity(
        view: &oxy_airlayer_compat::View,
        entity_name: &str,
        display_field: Option<&str>,
    ) -> Self {
        let entity = view
            .entities
            .iter()
            .find(|e| e.name == entity_name && e.entity_type == EntityType::Primary);
        let pk_names = entity.map(|e| e.get_keys()).unwrap_or_default();
        let label_name: Option<String> = display_field.map(|s| s.to_string());
        let mut dims: Vec<String> = pk_names
            .iter()
            .map(|k| format!("{}.{}", view.name, k))
            .collect();
        let pk_count = dims.len();
        let has_label_dim = if let Some(ref lbl) = label_name {
            let lbl_ref = format!("{}.{}", view.name, lbl);
            if !dims.contains(&lbl_ref) {
                dims.push(lbl_ref);
                true
            } else {
                false
            }
        } else {
            false
        };
        Self {
            dims,
            pk_count,
            has_label_dim,
            label_name,
            pk_names,
        }
    }

    /// Build a display string from a SELECT result row (columns ordered as `self.dims`).
    pub(super) fn display_from_row(&self, row: &[String]) -> String {
        if self.has_label_dim {
            let v = row.get(self.pk_count).cloned().unwrap_or_default();
            if !v.is_empty() {
                return v;
            }
        }
        self.join_pks_from_row(row)
    }

    /// Build a display string from attribute name→value pairs (e.g. the attrs query).
    pub(super) fn display_from_attrs(&self, attrs: &[(String, String)]) -> String {
        if let Some(ref lbl) = self.label_name
            && let Some((_, v)) = attrs.iter().find(|(n, _)| n == lbl)
            && !v.is_empty()
        {
            return v.clone();
        }
        let parts: Vec<&str> = self
            .pk_names
            .iter()
            .filter_map(|pk| attrs.iter().find(|(n, _)| n == pk).map(|(_, v)| v.as_str()))
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            String::new()
        } else {
            parts.join(" · ")
        }
    }

    pub(super) fn join_pks_from_row(&self, row: &[String]) -> String {
        let parts: Vec<&str> = row[..self.pk_count.min(row.len())]
            .iter()
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            row.first().cloned().unwrap_or_default()
        } else {
            parts.join(" · ")
        }
    }
}

/// Turn a sample SELECT row into `(display, nav_key)`.
///
/// Columns are ordered `[pk_0, .., pk_{pk_count-1}, (label?)]` per
/// `EntityDisplaySpec::dims`. `nav_key` is the canonical key the instance
/// endpoints accept: the plain first PK value for single-PK entities, or a
/// JSON array string of the PK columns for composite PKs. `display` prefers the
/// label column, falling back to PK columns joined with " · ".
pub(super) fn sample_row_to_display_key(
    row: &[String],
    pk_count: usize,
    has_label_dim: bool,
) -> (String, String) {
    let pk_vals = &row[..pk_count.min(row.len())];
    let nav_key = if pk_count <= 1 {
        row.first().cloned().unwrap_or_default()
    } else {
        serde_json::to_string(&pk_vals.to_vec())
            .unwrap_or_else(|_| row.first().cloned().unwrap_or_default())
    };
    let display = if has_label_dim {
        row.get(pk_count)
            .cloned()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| join_pk_parts(pk_vals, row))
    } else {
        join_pk_parts(pk_vals, row)
    };
    (display, nav_key)
}

/// Join non-empty PK column values with " · "; fall back to the first column.
fn join_pk_parts(pk_vals: &[String], row: &[String]) -> String {
    let parts: Vec<&str> = pk_vals
        .iter()
        .map(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        row.first().cloned().unwrap_or_default()
    } else {
        parts.join(" · ")
    }
}

fn sql_quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Return the injected `_row_count` measure reference for a view.
pub(super) fn count_measure_ref(view: &oxy_airlayer_compat::View) -> String {
    format!("{}.{}", view.name, "__oxy_row_count")
}

fn measure_agg(measure_type: MeasureType, expr: &str) -> Option<String> {
    match measure_type {
        MeasureType::Sum => Some(format!("SUM({expr})")),
        MeasureType::Count => Some("COUNT(*)".to_string()),
        MeasureType::Average => Some(format!("AVG({expr})")),
        MeasureType::Min => Some(format!("MIN({expr})")),
        MeasureType::Max => Some(format!("MAX({expr})")),
        MeasureType::CountDistinct => Some(format!("COUNT(DISTINCT {expr})")),
        _ => None,
    }
}

pub(super) fn apply_world_model_config(
    entities: &mut Vec<WmEntity>,
    edges: &mut Vec<WmEdge>,
    cfg: &oxy_world_model::world_model_config::WorldModelConfig,
) {
    use oxy_world_model::world_model_config::{WmEntityConfig, WmFieldConfig};
    use std::collections::{HashMap, HashSet};

    let entity_map: HashMap<&str, &WmEntityConfig> =
        cfg.entities.iter().map(|e| (e.id.as_str(), e)).collect();

    // Filter entities to only those listed in config
    entities.retain(|e| entity_map.contains_key(e.id.as_str()));

    // Filter edges — keep only edges where both endpoints survived the entity filter
    let kept: HashSet<&str> = entities.iter().map(|e| e.id.as_str()).collect();
    edges.retain(|e| kept.contains(e.from.as_str()) && kept.contains(e.to.as_str()));

    for entity in entities.iter_mut() {
        let Some(ec) = entity_map.get(entity.id.as_str()) else {
            continue;
        };

        if let Some(lbl) = &ec.label {
            entity.label = lbl.clone();
        }
        if let Some(desc) = &ec.description {
            entity.description = Some(desc.clone());
        }
        entity.display_field = ec.display_field.clone();

        // Filter and relabel dimensions when the config lists them explicitly
        if let Some(dim_cfgs) = &ec.dimensions {
            let dim_map: HashMap<&str, &WmFieldConfig> =
                dim_cfgs.iter().map(|d| (d.name.as_str(), d)).collect();
            let order: HashMap<&str, usize> = dim_cfgs
                .iter()
                .enumerate()
                .map(|(i, d)| (d.name.as_str(), i))
                .collect();
            entity
                .dimensions
                .retain(|d| dim_map.contains_key(d.name.as_str()));
            entity
                .dimensions
                .sort_by_key(|d| order.get(d.name.as_str()).copied().unwrap_or(usize::MAX));
            for dim in entity.dimensions.iter_mut() {
                if let Some(dc) = dim_map.get(dim.name.as_str()) {
                    dim.label = dc.label.clone();
                    if let Some(desc) = &dc.description {
                        dim.description = Some(desc.clone());
                    }
                }
            }
        }

        // Filter and relabel measures (own + induced) when listed explicitly
        if let Some(meas_cfgs) = &ec.measures {
            let meas_map: HashMap<&str, &WmFieldConfig> =
                meas_cfgs.iter().map(|m| (m.name.as_str(), m)).collect();
            entity
                .own_measures
                .retain(|m| meas_map.contains_key(m.name.as_str()));
            entity
                .induced_measures
                .retain(|m| meas_map.contains_key(m.name.as_str()));
            for m in entity.own_measures.iter_mut() {
                if let Some(mc) = meas_map.get(m.name.as_str()) {
                    m.label = mc.label.clone();
                    if let Some(desc) = &mc.description {
                        m.description = Some(desc.clone());
                    }
                }
            }
            for m in entity.induced_measures.iter_mut() {
                if let Some(mc) = meas_map.get(m.name.as_str()) {
                    m.label = mc.label.clone();
                    if let Some(desc) = &mc.description {
                        m.description = Some(desc.clone());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod wm_config_tests {
    use oxy_airlayer_compat::schema::models::AdditivityClass;

    use super::*;
    use oxy_world_model::world_model_config::{WmEntityConfig, WmFieldConfig, WorldModelConfig};

    fn make_entity(id: &str) -> WmEntity {
        WmEntity {
            id: id.to_string(),
            label: id.to_string(),
            view: id.to_string(),
            description: None,
            depth: 0,
            display_field: None,
            dimensions: vec![
                WmDimension {
                    name: "dim_a".into(),
                    dim_type: "string".into(),
                    label: None,
                    description: None,
                },
                WmDimension {
                    name: "dim_b".into(),
                    dim_type: "number".into(),
                    label: None,
                    description: None,
                },
            ],
            own_measures: vec![WmMeasure {
                name: "revenue".into(),
                measure_type: MeasureType::Sum,
                additivity: AdditivityClass::Additive,
                label: None,
                description: None,
                expr: None,
                has_breakdown: false,
            }],
            induced_measures: vec![],
        }
    }

    fn s(v: &str) -> serde_json::Value {
        serde_json::Value::String(v.to_string())
    }

    fn in_values(f: &agentic_semantic::config::SemanticFilter) -> Vec<String> {
        match &f.filter_type {
            agentic_semantic::config::SemanticFilterType::In(a) => {
                let mut vs: Vec<String> = a
                    .values
                    .iter()
                    .map(|v| v.as_str().unwrap_or_default().to_string())
                    .collect();
                vs.sort();
                vs
            }
            _ => panic!("expected IN filter"),
        }
    }

    // A seed carrying only its first PK column (the instance-picker shape) must
    // not emit an empty `IN ()` for the child's remaining composite-FK columns —
    // that empty filter is what zeroed the Shipment/Return node counts.
    #[test]
    fn child_fk_filters_skips_columns_absent_from_parent() {
        let fk = vec![
            "order_shipments.order_id".to_string(),
            "order_shipments.line_item_id".to_string(),
        ];
        // Seed only constrains the first column (order_id = 2).
        let parent_pk_rows = vec![vec![s("2")]];
        let filters = child_fk_filters(&fk, &parent_pk_rows);
        assert_eq!(filters.len(), 1, "only the supplied column is filtered");
        assert_eq!(filters[0].field, "order_shipments.order_id");
        assert_eq!(in_values(&filters[0]), vec!["2".to_string()]);
    }

    // When the parent supplies all composite columns, every FK column is filtered
    // and duplicate values are de-duplicated.
    #[test]
    fn child_fk_filters_uses_all_columns_when_present() {
        let fk = vec![
            "order_shipments.order_id".to_string(),
            "order_shipments.line_item_id".to_string(),
        ];
        let parent_pk_rows = vec![vec![s("2"), s("4")], vec![s("2"), s("7")]];
        let filters = child_fk_filters(&fk, &parent_pk_rows);
        assert_eq!(filters.len(), 2);
        assert_eq!(in_values(&filters[0]), vec!["2".to_string()]);
        assert_eq!(
            in_values(&filters[1]),
            vec!["4".to_string(), "7".to_string()]
        );
    }

    // No usable values anywhere → no filters (caller skips the entity instead of
    // counting every row).
    #[test]
    fn child_fk_filters_empty_when_no_values() {
        let fk = vec!["v.fk".to_string()];
        let filters = child_fk_filters(&fk, &[]);
        assert!(filters.is_empty());
    }

    fn wm_view(name: &str, entities: serde_json::Value) -> oxy_airlayer_compat::View {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "table": name,
            "entities": entities,
            "dimensions": [],
        }))
        .expect("valid view")
    }

    /// The `examples/` star-schema slice: `order` rolls up to `customer` (its
    /// declared parent) and also foreign-references `retail_store` and
    /// `shipping_address`. The link set must union the parent spine with the
    /// foreign cross-links, tagging each correctly.
    #[test]
    fn build_entity_links_unions_parent_and_cross_links() {
        let orders = wm_view(
            "orders",
            serde_json::json!([
                {"name": "order", "type": "primary", "key": "order_id", "parent": "customer"},
                {"name": "customer", "type": "foreign", "key": "customer_id"},
                {"name": "shipping_address", "type": "foreign", "key": "shipping_address_id"},
                {"name": "retail_store", "type": "foreign", "key": "store_id"},
            ]),
        );
        let customers = wm_view(
            "customers",
            serde_json::json!([{"name": "customer", "type": "primary", "key": "customer_id"}]),
        );
        let stores = wm_view(
            "stores",
            serde_json::json!([{"name": "retail_store", "type": "primary", "key": "store_id"}]),
        );
        let shipping = wm_view(
            "shipping_addresses",
            serde_json::json!([
                {"name": "shipping_address", "type": "primary", "key": "shipping_address_id"},
            ]),
        );
        let views = vec![orders.clone(), customers, stores, shipping];

        let links = build_entity_links(&views, &orders, Some("customer"));
        assert_eq!(
            links,
            vec![
                EntityLink {
                    target_entity: "customer".into(),
                    fk_dim_refs: vec!["orders.customer_id".into()],
                    kind: LinkKind::Parent,
                },
                EntityLink {
                    target_entity: "shipping_address".into(),
                    fk_dim_refs: vec!["orders.shipping_address_id".into()],
                    kind: LinkKind::CrossLink,
                },
                EntityLink {
                    target_entity: "retail_store".into(),
                    fk_dim_refs: vec!["orders.store_id".into()],
                    kind: LinkKind::CrossLink,
                },
            ],
        );
    }

    fn link(target: &str, fk: &str, kind: LinkKind) -> EntityLink {
        EntityLink {
            target_entity: target.into(),
            fk_dim_refs: vec![fk.into()],
            kind,
        }
    }

    fn meta_for(
        name: &str,
        view: &str,
        pk_dim_refs: Vec<String>,
        links: Vec<EntityLink>,
        sample_dims: Vec<String>,
        has_label_dim: bool,
    ) -> EntityMeta {
        EntityMeta {
            entity_name: name.into(),
            view_name: view.into(),
            datasource: "local".into(),
            pk_count: pk_dim_refs.len(),
            pk_dim_refs,
            links,
            sample_dims,
            has_label_dim,
        }
    }

    /// The expansion query selects PK + each outbound link's FK + the label in
    /// one shot; the plan records where each lands so one scan yields matched,
    /// PK rows, FK values, and the sample.
    #[test]
    fn wm_expansion_plan_lays_out_pk_fk_columns() {
        let meta = meta_for(
            "order",
            "orders",
            vec!["orders.order_id".into()],
            vec![
                link("customer", "orders.customer_id", LinkKind::Parent),
                link("retail_store", "orders.store_id", LinkKind::CrossLink),
            ],
            vec!["orders.order_id".into()],
            false,
        );
        let plan = wm_expansion_plan(&meta);
        assert_eq!(
            plan.dims,
            vec!["orders.order_id", "orders.customer_id", "orders.store_id"]
        );
        assert_eq!(plan.pk_cols, vec![0]);
        assert_eq!(plan.label_col, None);
        assert_eq!(
            plan.link_cols,
            vec![("customer".to_string(), 1), ("retail_store".to_string(), 2)]
        );
    }

    /// When the label dimension coincides with the PK column (cities: `city` is
    /// both key and label), the layout de-duplicates to a single column.
    #[test]
    fn wm_expansion_plan_dedups_label_matching_pk() {
        let meta = meta_for(
            "city",
            "cities",
            vec!["cities.city".into()],
            vec![link("region", "cities.region", LinkKind::Parent)],
            vec!["cities.city".into(), "cities.city".into()],
            true,
        );
        let plan = wm_expansion_plan(&meta);
        assert_eq!(plan.dims, vec!["cities.city", "cities.region"]);
        assert_eq!(plan.pk_cols, vec![0]);
        assert_eq!(plan.label_col, Some(0));
        assert_eq!(plan.link_cols, vec![("region".to_string(), 1)]);
    }

    /// `matched` counts DISTINCT PK tuples (an FK fanning out to duplicate rows
    /// never inflates it); FK values are de-duplicated; the sample is the first
    /// three rows projected to [PK.., label].
    #[test]
    fn parse_expansion_rows_distinct_pk_and_fk() {
        let plan = WmExpansionPlan {
            dims: vec!["v.pk".into(), "v.name".into(), "v.customer_id".into()],
            pk_cols: vec![0],
            label_col: Some(1),
            link_cols: vec![("customer".into(), 2)],
        };
        let rows = vec![
            vec!["1".into(), "Alice".into(), "100".into()],
            vec!["1".into(), "Alice".into(), "100".into()],
            vec!["2".into(), "Bob".into(), "100".into()],
        ];
        let res = parse_expansion_rows(&rows, &plan);
        assert_eq!(res.matched, 2, "distinct PK count, not raw row count");
        assert_eq!(res.pk_rows, vec![vec![s("1")], vec![s("2")]]);
        let (target, mut vals) = res.fk_values.into_iter().next().unwrap();
        assert_eq!(target, "customer");
        vals.sort_by_key(|v| v.as_str().unwrap_or_default().to_string());
        assert_eq!(vals, vec![s("100")]);
        // Sample is deduped by PK tuple: the two identical Alice rows collapse
        // to one preview so a fanned-out FK never wastes a preview slot.
        assert_eq!(res.sample, vec!["Alice", "Bob"]);
        assert_eq!(res.sample_keys, vec!["1", "2"]);
    }

    /// The PK dedup must happen *before* the `take(3)` window, so a leading run
    /// of duplicate rows can't crowd distinct instances out of the 3 preview
    /// slots (the bug: `[1,1,1]` previews instead of `[1,2,3]`).
    #[test]
    fn parse_expansion_rows_sample_dedups_before_take() {
        let plan = WmExpansionPlan {
            dims: vec!["v.pk".into(), "v.name".into()],
            pk_cols: vec![0],
            label_col: Some(1),
            link_cols: vec![],
        };
        let rows = vec![
            vec!["1".into(), "Alice".into()],
            vec!["1".into(), "Alice".into()],
            vec!["1".into(), "Alice".into()],
            vec!["2".into(), "Bob".into()],
            vec!["3".into(), "Carol".into()],
            vec!["4".into(), "Dave".into()],
        ];
        let res = parse_expansion_rows(&rows, &plan);
        assert_eq!(res.sample, vec!["Alice", "Bob", "Carol"]);
        assert_eq!(res.sample_keys, vec!["1", "2", "3"]);
    }

    /// A Foreign entity whose name resolves to no Primary anywhere is not
    /// navigable — it has no node to point at, so it is dropped (mirrors the
    /// `target_exists` guard the drawn-edge builder uses).
    #[test]
    fn build_entity_links_skips_unresolvable_foreign() {
        let orders = wm_view(
            "orders",
            serde_json::json!([
                {"name": "order", "type": "primary", "key": "order_id"},
                {"name": "ghost", "type": "foreign", "key": "ghost_id"},
            ]),
        );
        let views = vec![orders.clone()];
        assert!(build_entity_links(&views, &orders, None).is_empty());
    }

    #[test]
    fn sample_row_single_pk_no_label() {
        let row = vec!["42".to_string()];
        let (display, key) = sample_row_to_display_key(&row, 1, false);
        assert_eq!(display, "42");
        assert_eq!(key, "42");
    }

    #[test]
    fn sample_row_with_label_dim() {
        let row = vec!["42".to_string(), "Acme Corp".to_string()];
        let (display, key) = sample_row_to_display_key(&row, 1, true);
        assert_eq!(display, "Acme Corp");
        assert_eq!(key, "42");
    }

    #[test]
    fn sample_row_composite_pk_json_key() {
        let row = vec!["70978".to_string(), "177411".to_string()];
        let (display, key) = sample_row_to_display_key(&row, 2, false);
        assert_eq!(display, "70978 · 177411");
        assert_eq!(key, r#"["70978","177411"]"#);
    }

    #[test]
    fn sample_row_label_empty_falls_back_to_pks() {
        let row = vec!["42".to_string(), "".to_string()];
        let (display, key) = sample_row_to_display_key(&row, 1, true);
        assert_eq!(display, "42");
        assert_eq!(key, "42");
    }

    #[test]
    fn entity_not_in_config_is_filtered() {
        let mut entities = vec![make_entity("orders"), make_entity("customers")];
        let mut edges = vec![];
        let cfg = WorldModelConfig {
            entities: vec![WmEntityConfig {
                id: "orders".into(),
                label: None,
                description: None,
                display_field: None,
                dimensions: None,
                measures: None,
            }],
        };
        apply_world_model_config(&mut entities, &mut edges, &cfg);
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].id, "orders");
    }

    #[test]
    fn label_and_display_field_applied() {
        let mut entities = vec![make_entity("orders")];
        let mut edges = vec![];
        let cfg = WorldModelConfig {
            entities: vec![WmEntityConfig {
                id: "orders".into(),
                label: Some("My Orders".into()),
                description: None,
                display_field: Some("customer_name".into()),
                dimensions: None,
                measures: None,
            }],
        };
        apply_world_model_config(&mut entities, &mut edges, &cfg);
        assert_eq!(entities[0].label, "My Orders");
        assert_eq!(entities[0].display_field.as_deref(), Some("customer_name"));
    }

    #[test]
    fn dimension_allowlist_filters_and_labels() {
        let mut entities = vec![make_entity("orders")];
        let mut edges = vec![];
        let cfg = WorldModelConfig {
            entities: vec![WmEntityConfig {
                id: "orders".into(),
                label: None,
                description: None,
                display_field: None,
                dimensions: Some(vec![WmFieldConfig {
                    name: "dim_b".into(),
                    label: Some("B Label".into()),
                    description: None,
                }]),
                measures: None,
            }],
        };
        apply_world_model_config(&mut entities, &mut edges, &cfg);
        assert_eq!(entities[0].dimensions.len(), 1);
        assert_eq!(entities[0].dimensions[0].name, "dim_b");
        assert_eq!(entities[0].dimensions[0].label.as_deref(), Some("B Label"));
    }

    #[test]
    fn absent_dimensions_shows_all() {
        let mut entities = vec![make_entity("orders")];
        let mut edges = vec![];
        let cfg = WorldModelConfig {
            entities: vec![WmEntityConfig {
                id: "orders".into(),
                label: None,
                description: None,
                display_field: None,
                dimensions: None,
                measures: None,
            }],
        };
        apply_world_model_config(&mut entities, &mut edges, &cfg);
        assert_eq!(entities[0].dimensions.len(), 2);
    }

    #[test]
    fn edges_filtered_when_endpoint_entity_hidden() {
        let mut entities = vec![make_entity("orders"), make_entity("customers")];
        let mut edges = vec![WmEdge {
            from: "orders".into(),
            to: "customers".into(),
            functional: true,
        }];
        let cfg = WorldModelConfig {
            entities: vec![WmEntityConfig {
                id: "orders".into(),
                label: None,
                description: None,
                display_field: None,
                dimensions: None,
                measures: None,
            }],
        };
        apply_world_model_config(&mut entities, &mut edges, &cfg);
        assert!(
            edges.is_empty(),
            "edge to hidden 'customers' must be removed"
        );
    }
}

/// Build per-entity drill-down metadata for every Primary entity in the layer.
/// Shared by the filter-counts BFS and the scoped sample-browser endpoint so
/// both traverse the exact same navigable link graph.
pub(super) fn build_entity_metas(
    layer: &oxy_airlayer_compat::SemanticLayer,
    promotions: &Promotions,
    wm_cfg: Option<&oxy_world_model::world_model_config::WorldModelConfig>,
) -> Vec<EntityMeta> {
    let get_display_field = |entity_id: &str| -> Option<String> {
        wm_cfg
            .and_then(|cfg| cfg.entities.iter().find(|e| e.id == entity_id))
            .and_then(|ec| ec.display_field.clone())
    };
    layer
        .views
        .iter()
        .filter_map(|view| {
            let primary = view
                .entities
                .iter()
                .find(|e| e.entity_type == EntityType::Primary)?;
            let pk_names = entity_keys_in_view(view, &primary.name, true);
            if pk_names.is_empty() {
                return None;
            }
            let pk_dim_refs = pk_names
                .iter()
                .map(|k| format!("{}.{}", view.name, k))
                .collect();
            let parent_entity = promotions.parent_of(&primary.name);
            let links = build_entity_links(&layer.views, view, parent_entity);
            let disp = EntityDisplaySpec::for_entity(
                view,
                &primary.name,
                get_display_field(&primary.name).as_deref(),
            );
            Some(EntityMeta {
                entity_name: primary.name.clone(),
                view_name: view.name.clone(),
                datasource: view.datasource.clone().unwrap_or_default(),
                pk_dim_refs,
                links,
                sample_dims: disp.dims,
                pk_count: disp.pk_count,
                has_label_dim: disp.has_label_dim,
            })
        })
        .collect()
}

/// Everything needed to compile + execute one drill-down query outside the
/// streaming filter-counts handler.
///
/// Compilation goes through the shared engine cache like every other semantic
/// surface. The sample browser is low-QPS, so it is not the reason the cache
/// exists — but it reads the same workspace as the handlers around it, and a
/// second door here is how the cache stopped meaning anything the first time.
pub(super) struct WmExecCtx {
    pub(super) workspace_manager: WorkspaceManager<WorkingCopy>,
    pub(super) user_id: Uuid,
    pub(super) role: WorkspaceRole,
    pub(super) scan_path: std::path::PathBuf,
    pub(super) databases: Vec<oxy_airlayer_compat::DatabaseConfig>,
    pub(super) layer: oxy_airlayer_compat::SemanticLayer,
    pub(super) engine_cache:
        std::sync::Arc<crate::server::router::workspace_cache::SemanticEngineCache>,
    pub(super) engine_key: oxy_airlayer_compat::EngineKey,
}

impl WmExecCtx {
    /// Compile a config to `(sql, database_name)`; `None` on compile failure.
    pub(super) async fn compile_full(&self, cfg: SemanticQueryConfig) -> Option<(String, String)> {
        let sp = self.scan_path.clone();
        let dbs = self.databases.clone();
        let layer = self.layer.clone();
        let cache = self.engine_cache.clone();
        let key = self.engine_key;
        tokio::task::spawn_blocking(move || {
            resolve_and_compile_cached(&cache, key, &sp, &dbs, &cfg, None, Some(layer)).ok()
        })
        .await
        .ok()
        .flatten()
        .map(|compiled| match compiled {
            CompiledQuery::Warehouse { sql, database_name } => (sql, database_name),
            // No `PreaggContext` is passed above, so this variant is
            // unreachable today. Take the warehouse SQL rather than the
            // rollup's if that ever changes: the rollup SQL reads Parquet
            // through DuckDB and `run_expansion` runs whatever comes back
            // through a warehouse connector.
            CompiledQuery::Preaggregation {
                warehouse_sql,
                warehouse_database,
                ..
            } => (warehouse_sql, warehouse_database),
        })
    }

    /// Run one expansion query and parse it into matched PK rows + outbound FK
    /// values (the fuel the BFS needs to reach the next hop). Empty on any
    /// failure so an unreachable node just contributes nothing.
    pub(super) async fn run_expansion(
        &self,
        datasource: &str,
        cfg: SemanticQueryConfig,
        plan: &WmExpansionPlan,
    ) -> WmExpansionResult {
        let empty = || WmExpansionResult {
            matched: 0,
            pk_rows: vec![],
            fk_values: vec![],
            sample: vec![],
            sample_keys: vec![],
        };
        let Some((sql, _db)) = self.compile_full(cfg).await else {
            return empty();
        };
        let Ok(connector) = build_connector(
            &self.workspace_manager,
            self.user_id,
            self.role.clone(),
            datasource,
        )
        .await
        else {
            return empty();
        };
        let rows = run_with_connector(&connector, &sql).await;
        parse_expansion_rows(&rows, plan)
    }
}

/// Build the single-column `Eq` PK filter that selects one entity's own instance.
/// Parse a request key into its PK component values. The picker encodes a
/// composite-PK instance as a JSON array of strings (`["2","4"]`); a single-PK
/// instance may arrive either as a bare scalar or a one-element array. Mirrors
/// the instance-detail / measure-breakdown handlers so every consumer agrees on
/// the same decoding.
fn parse_key_values(key: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(key).unwrap_or_else(|_| vec![key.to_string()])
}

/// The seed's PK as a single composite row (`Vec<Value>`, one entry per PK
/// column). Used to seed BFS `pk_rows` when the pre-fetch query returns nothing.
pub(super) fn seed_pk_row(key: &str) -> Vec<serde_json::Value> {
    parse_key_values(key)
        .into_iter()
        .map(serde_json::Value::String)
        .collect()
}

/// Equality filters selecting the seed instance by its PK. A single value maps
/// to the first PK dim (the picker's single-PK flow, unchanged); multiple values
/// are zipped positionally against the composite PK dims — so an `order_item`
/// keyed by `["2","4"]` filters `order_id=2 AND line=4` rather than comparing the
/// first PK column against the literal JSON string `["2","4"]` (which matches
/// nothing and zeroed every reachable count). Empty only when the entity has no
/// PK dims.
pub(super) fn seed_self_filters(meta: &EntityMeta, key: &str) -> Vec<SemanticFilter> {
    let values = parse_key_values(key);
    let mk = |field: String, value: String| SemanticFilter {
        field,
        filter_type: SemanticFilterType::Eq(ScalarFilter {
            value: serde_json::Value::String(value),
        }),
    };
    if values.len() == 1 {
        return meta
            .pk_dim_refs
            .first()
            .cloned()
            .map(|field| vec![mk(field, values[0].clone())])
            .unwrap_or_default();
    }
    meta.pk_dim_refs
        .iter()
        .zip(values.iter())
        .map(|(field, val)| mk(field.clone(), val.clone()))
        .collect()
}

#[cfg(test)]
mod seed_key_tests {
    use super::*;

    fn meta(view: &str, pk_dims: &[&str]) -> EntityMeta {
        EntityMeta {
            entity_name: view.to_string(),
            view_name: view.to_string(),
            datasource: "db".to_string(),
            pk_dim_refs: pk_dims.iter().map(|d| format!("{view}.{d}")).collect(),
            links: vec![],
            sample_dims: vec![],
            pk_count: pk_dims.len(),
            has_label_dim: false,
        }
    }

    fn eq_value(f: &SemanticFilter) -> &str {
        match &f.filter_type {
            SemanticFilterType::Eq(ScalarFilter {
                value: serde_json::Value::String(s),
            }) => s,
            _ => panic!("expected string Eq filter"),
        }
    }

    #[test]
    fn scalar_key_filters_first_pk_dim() {
        let m = meta("order_item", &["id"]);
        let filters = seed_self_filters(&m, "42");
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].field, "order_item.id");
        assert_eq!(eq_value(&filters[0]), "42");
    }

    // The regression: a composite-PK seed encoded as a JSON array must filter
    // every PK column, not compare the first column to the literal `["2","4"]`.
    #[test]
    fn composite_json_key_filters_each_pk_dim() {
        let m = meta("order_item", &["order_id", "line_number"]);
        let filters = seed_self_filters(&m, "[\"2\",\"4\"]");
        assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].field, "order_item.order_id");
        assert_eq!(eq_value(&filters[0]), "2");
        assert_eq!(filters[1].field, "order_item.line_number");
        assert_eq!(eq_value(&filters[1]), "4");
    }

    // A single-element JSON array is still the single-PK flow (first dim only).
    #[test]
    fn single_element_array_maps_to_first_pk_dim() {
        let m = meta("order_item", &["id"]);
        let filters = seed_self_filters(&m, "[\"7\"]");
        assert_eq!(filters.len(), 1);
        assert_eq!(eq_value(&filters[0]), "7");
    }

    #[test]
    fn no_pk_dims_yields_no_filters() {
        let m = meta("order_item", &[]);
        assert!(seed_self_filters(&m, "42").is_empty());
    }

    #[test]
    fn seed_pk_row_expands_composite_key() {
        assert_eq!(
            seed_pk_row("[\"2\",\"4\"]"),
            vec![
                serde_json::Value::String("2".into()),
                serde_json::Value::String("4".into()),
            ]
        );
        assert_eq!(
            seed_pk_row("42"),
            vec![serde_json::Value::String("42".into())]
        );
    }
}

/// Local BFS state fuel — an entity's matched PK rows and its distinct outbound
/// FK values per target (identical role to the filter-counts `NeighborData`).
struct BfsNeighbor {
    pk_rows: Vec<Vec<serde_json::Value>>,
    fk_values: HashMap<String, Vec<serde_json::Value>>,
}

/// Reconstruct the single-view `SemanticFilter` set that selects the rows of
/// `target` reachable from the seed instance, replaying the same undirected
/// link-graph BFS `filter-counts` uses — but stopping the moment `target` is
/// discovered and returning the filters that reach it. `None` when `target` is
/// unreachable from the seed.
///
/// The returned filters constrain only `target`'s own view (its PK for an
/// outbound/coarser target, its FK for an inbound/finer one), so the caller can
/// drop them straight into a paginated `SELECT` over that entity.
pub(super) async fn resolve_target_filters(
    exec: &WmExecCtx,
    entity_metas: &[EntityMeta],
    seed_entity: &str,
    seed_key: &str,
    target: &str,
) -> Option<Vec<SemanticFilter>> {
    let meta_idx: HashMap<&str, usize> = entity_metas
        .iter()
        .enumerate()
        .map(|(i, m)| (m.entity_name.as_str(), i))
        .collect();
    let seed_idx = *meta_idx.get(seed_entity)?;
    let target_idx = *meta_idx.get(target)?;

    // The seed itself: match its own PK (no traversal needed).
    if seed_entity == target {
        let filters = seed_self_filters(&entity_metas[seed_idx], seed_key);
        return (!filters.is_empty()).then_some(filters);
    }

    // Reverse adjacency: coarser entity → its finer neighbours (child idx, the
    // child's FK refs pointing at it) — the inbound direction cross-links need.
    let inbound = build_inbound_index(entity_metas);

    // Schema-level reachability (pure, no IO) — see the identical reasoning in
    // `post_world_model_filter_counts`. `target` unreachable in the schema
    // graph at all ⇒ nothing more to show, same as an empty BFS today.
    let reachable = schema_reachable_entities(entity_metas, &inbound, &meta_idx, seed_idx);
    if !reachable.contains(&target_idx) {
        return None;
    }

    // Fast path: same datasource ⇒ a filter on the seed's own view is enough.
    // airlayer auto-joins `target`'s view back to the seed's, however many
    // hops apart, when the caller's query later references both — no BFS
    // needed to compute the filter set at all.
    if entity_metas[target_idx].datasource == entity_metas[seed_idx].datasource {
        let filters = seed_self_filters(&entity_metas[seed_idx], seed_key);
        return (!filters.is_empty()).then_some(filters);
    }

    // Legacy fallback: cross-datasource pair — airlayer can't join across
    // datasources, so thread matched values through as literal filters,
    // hop by hop, same as before.
    //
    // Seed pre-fetch — learn its PK rows + outbound FK values so hop 1 can expand.
    let mut neighbor_data: HashMap<String, BfsNeighbor> = HashMap::new();
    let seed_meta = &entity_metas[seed_idx];
    if seed_meta.links.is_empty() {
        neighbor_data.insert(
            seed_entity.to_string(),
            BfsNeighbor {
                pk_rows: vec![seed_pk_row(seed_key)],
                fk_values: HashMap::new(),
            },
        );
    } else {
        let plan = wm_expansion_plan(seed_meta);
        let cfg = SemanticQueryConfig {
            topic: None,
            dimensions: plan.dims.clone(),
            measures: vec![],
            time_dimensions: vec![],
            filters: seed_self_filters(seed_meta, seed_key),
            orders: vec![],
            limit: None,
            offset: None,
        };
        let res = exec.run_expansion(&seed_meta.datasource, cfg, &plan).await;
        let pk_rows = if res.pk_rows.is_empty() {
            vec![seed_pk_row(seed_key)]
        } else {
            res.pk_rows
        };
        neighbor_data.insert(
            seed_entity.to_string(),
            BfsNeighbor {
                pk_rows,
                fk_values: res.fk_values.into_iter().collect(),
            },
        );
    }

    let mut visited: std::collections::HashSet<String> =
        std::collections::HashSet::from([seed_entity.to_string()]);
    let mut frontier: Vec<String> = vec![seed_entity.to_string()];

    while !frontier.is_empty() {
        // Assemble the filter set for every newly reachable entity (no IO here —
        // each frontier entity already carries the fuel it needs).
        let chosen = assemble_hop_filters(
            entity_metas,
            &meta_idx,
            &inbound,
            &neighbor_data,
            &frontier,
            &visited,
        );
        if chosen.is_empty() {
            break;
        }

        // Target reached — its filter set is exactly what selects its reachable
        // rows. Return before running target's own (possibly large) expansion.
        if let Some(&t_idx) = meta_idx.get(target)
            && let Some(filters) = chosen.get(&t_idx)
        {
            return Some(filters.clone());
        }

        // Expand every discovered node to fuel the next hop.
        let mut next_frontier: Vec<String> = Vec::new();
        for (idx, filters) in chosen {
            let meta = &entity_metas[idx];
            visited.insert(meta.entity_name.clone());
            let plan = wm_expansion_plan(meta);
            let cfg = SemanticQueryConfig {
                topic: None,
                dimensions: plan.dims.clone(),
                measures: vec![],
                time_dimensions: vec![],
                filters,
                orders: vec![],
                limit: None,
                offset: None,
            };
            let res = exec.run_expansion(&meta.datasource, cfg, &plan).await;
            if res.matched > 0 {
                neighbor_data.insert(
                    meta.entity_name.clone(),
                    BfsNeighbor {
                        pk_rows: res.pk_rows,
                        fk_values: res.fk_values.into_iter().collect(),
                    },
                );
                next_frontier.push(meta.entity_name.clone());
            }
        }
        frontier = next_frontier;
    }
    None
}

/// One BFS hop's filter assembly (pure): for every entity newly reachable from
/// `frontier`, produce the `SemanticFilter` set that selects its reachable rows.
/// First-writer-wins per entity so a diamond within a level yields one filter.
fn assemble_hop_filters<'a>(
    entity_metas: &'a [EntityMeta],
    meta_idx: &HashMap<&'a str, usize>,
    inbound: &HashMap<&'a str, Vec<(usize, &'a [String])>>,
    neighbor_data: &HashMap<String, BfsNeighbor>,
    frontier: &[String],
    visited: &std::collections::HashSet<String>,
) -> HashMap<usize, Vec<SemanticFilter>> {
    let mut chosen: HashMap<usize, Vec<SemanticFilter>> = HashMap::new();
    for e_name in frontier {
        let Some(nd) = neighbor_data.get(e_name) else {
            continue;
        };
        if nd.pk_rows.is_empty() {
            continue;
        }
        // Inbound (finer): children whose FK points at this frontier entity.
        if let Some(children) = inbound.get(e_name.as_str()) {
            for &(child_idx, fk_refs) in children {
                let child_name = &entity_metas[child_idx].entity_name;
                if visited.contains(child_name) || chosen.contains_key(&child_idx) {
                    continue;
                }
                let f = child_fk_filters(fk_refs, &nd.pk_rows);
                if !f.is_empty() {
                    chosen.insert(child_idx, f);
                }
            }
        }
        // Outbound (coarser): targets filtered by this entity's FK values.
        let Some(&e_idx) = meta_idx.get(e_name.as_str()) else {
            continue;
        };
        for link in &entity_metas[e_idx].links {
            let Some(&t_idx) = meta_idx.get(link.target_entity.as_str()) else {
                continue;
            };
            if visited.contains(&link.target_entity) || chosen.contains_key(&t_idx) {
                continue;
            }
            let Some(values) = nd.fk_values.get(&link.target_entity) else {
                continue;
            };
            if values.is_empty() {
                continue;
            }
            let Some(pk_field) = entity_metas[t_idx].pk_dim_refs.first().cloned() else {
                continue;
            };
            chosen.insert(
                t_idx,
                vec![SemanticFilter {
                    field: pk_field,
                    filter_type: SemanticFilterType::In(ArrayFilter {
                        values: values.clone(),
                    }),
                }],
            );
        }
    }
    chosen
}

/// Map an airlayer subtree (component edges only) into UI node/edge DTOs.
/// Returns None when `root_id` is absent from the tree.
pub(super) fn breakdown_structure(
    tree: &oxy_airlayer_compat::engine::metric_tree::MetricTree,
    root_id: &str,
) -> Option<(Vec<WmBreakdownNode>, Vec<WmBreakdownEdge>)> {
    use oxy_airlayer_compat::engine::metric_tree::{EdgeKind, EdgeOperator};
    let sub = tree.subtree(root_id)?;
    let nodes = sub
        .nodes
        .iter()
        .map(|n| WmBreakdownNode {
            id: n.id.clone(),
            view: n.view.clone(),
            measure: n.measure.clone(),
            label: n.label.clone(),
            measure_type: n.measure_type.clone(),
            is_composite: n.is_composite,
            is_root: n.id == root_id,
            expr: n.expr.clone(),
        })
        .collect();
    let edges = sub
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Component)
        .map(|e| WmBreakdownEdge {
            from: e.from.clone(),
            to: e.to.clone(),
            operator: match e.operator {
                EdgeOperator::Add => "add",
                EdgeOperator::Sub => "sub",
                EdgeOperator::Mul => "mul",
                EdgeOperator::Div => "div",
            }
            .to_string(),
            sign: e.sign,
        })
        .collect();
    Some((nodes, edges))
}

/// PK-equality filters for an instance key. `key_values.len()==1` → first PK col;
/// otherwise zip each PK col with its value (composite key).
fn build_pk_filters(
    view_name: &str,
    pk_cols: &[String],
    key_values: &[String],
) -> Vec<agentic_semantic::config::SemanticFilter> {
    use agentic_semantic::config::{ScalarFilter, SemanticFilter, SemanticFilterType};
    let eq = |field: String, val: &str| SemanticFilter {
        field,
        filter_type: SemanticFilterType::Eq(ScalarFilter {
            value: serde_json::Value::String(val.to_string()),
        }),
    };
    if key_values.len() == 1 {
        vec![eq(
            format!(
                "{view_name}.{}",
                pk_cols.first().cloned().unwrap_or_default()
            ),
            &key_values[0],
        )]
    } else {
        pk_cols
            .iter()
            .zip(key_values)
            .map(|(c, v)| eq(format!("{view_name}.{c}"), v))
            .collect()
    }
}

/// Filters that scope `target_view` to the instance.
/// - `target_view == primary_view` → PK filters.
/// - else → FK column for `entity` in `target_view` eq the first key value.
///
/// Returns None when no FK path resolves (the node will be streamed unvalued).
fn instance_filter_for_view(
    target_view: &oxy_airlayer_compat::View,
    entity: &str,
    key_values: &[String],
    pk_cols: &[String],
    primary_view: &str,
) -> Option<Vec<agentic_semantic::config::SemanticFilter>> {
    if target_view.name == primary_view {
        return Some(build_pk_filters(&target_view.name, pk_cols, key_values));
    }
    let fk = entity_keys_in_view(target_view, entity, false);
    let fk_col = fk.first()?;
    Some(build_pk_filters(
        &target_view.name,
        std::slice::from_ref(fk_col),
        &key_values[..1],
    ))
}

/// An instance scope as airlayer `QueryFilter`s, for callers that drive the
/// engine directly rather than through a `SemanticQueryConfig` (the metric-tree
/// ops).
///
/// Pins the entity's **own** view by primary key — `cities.city = 'Amsterdam'` —
/// and leaves the join to airlayer, exactly as the per-instance measure queries
/// do. That is what makes it work for a measure declared several hops away:
/// `orders` has no `city` key of its own, but the join graph resolves
/// `orders → stores → cities`, and every hop is many-to-one so nothing fans out.
///
/// Deliberately *not* [`instance_filter_for_view`], which pins the *target* view
/// through a direct foreign key. That is the right rule for a breakdown, which
/// values one view at a time and can mark an unreachable view unvalued. Here it
/// would refuse every measure more than one hop from the entity — which is most
/// of what a city or region instance lists.
///
/// `key` is the instance key as the world-model API spells it: a JSON array for
/// a composite key, else a bare scalar. `None` means the entity has no primary
/// view to pin.
pub(crate) fn instance_scope_filters(
    layer: &oxy_airlayer_compat::SemanticLayer,
    entity: &str,
    key: &str,
) -> Option<Vec<oxy_airlayer_compat::engine::query::QueryFilter>> {
    use agentic_semantic::config::SemanticFilterType;

    let primary = primary_view_of(layer, entity)?;
    let pk_cols = entity_keys_in_view(primary, entity, true);
    let key_values: Vec<String> =
        serde_json::from_str::<Vec<String>>(key).unwrap_or_else(|_| vec![key.to_string()]);

    build_pk_filters(&primary.name, &pk_cols, &key_values)
        .into_iter()
        .map(|f| {
            // Only Eq is reachable — build_pk_filters emits nothing else — but
            // translate rather than assume, so a new filter kind there fails
            // loudly here instead of silently widening the scope to everything.
            let SemanticFilterType::Eq(scalar) = f.filter_type else {
                return None;
            };
            let value = match scalar.value {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            Some(oxy_airlayer_compat::engine::query::QueryFilter {
                member: Some(f.field),
                operator: Some(oxy_airlayer_compat::engine::query::FilterOperator::Equals),
                values: vec![value],
                and: None,
                or: None,
            })
        })
        .collect()
}

/// In-memory valuation plan for a breakdown: one `SemanticQueryConfig` per view
/// group (measures = that view's subtree nodes, in node order), plus the node ids
/// that have no join path to the instance (streamed unvalued).
pub(super) struct BreakdownValuePlan {
    /// (view_name, node_ids in column order, config).
    pub(super) groups: Vec<(String, Vec<String>, SemanticQueryConfig)>,
    pub(super) unvalued: Vec<String>,
}

pub(super) fn breakdown_value_plan(
    layer: &oxy_airlayer_compat::SemanticLayer,
    nodes: &[WmBreakdownNode],
    entity: &str,
    key_values: &[String],
    pk_cols: &[String],
    primary_view: &str,
) -> BreakdownValuePlan {
    use std::collections::BTreeMap;
    // Preserve node order within each view group so columns map back to node ids.
    let mut by_view: BTreeMap<String, Vec<&WmBreakdownNode>> = BTreeMap::new();
    for n in nodes {
        by_view.entry(n.view.clone()).or_default().push(n);
    }
    let mut groups = Vec::new();
    let mut unvalued = Vec::new();
    for (view_name, group_nodes) in by_view {
        let Some(target_view) = layer.views.iter().find(|v| v.name == view_name) else {
            unvalued.extend(group_nodes.iter().map(|n| n.id.clone()));
            continue;
        };
        let Some(filters) =
            instance_filter_for_view(target_view, entity, key_values, pk_cols, primary_view)
        else {
            unvalued.extend(group_nodes.iter().map(|n| n.id.clone()));
            continue;
        };
        let make_cfg = |nodes: &[&WmBreakdownNode]| SemanticQueryConfig {
            topic: None,
            dimensions: vec![],
            measures: nodes
                .iter()
                .map(|n| format!("{}.{}", n.view, n.measure))
                .collect(),
            time_dimensions: vec![],
            filters: filters.clone(),
            orders: vec![],
            limit: Some(1),
            offset: None,
        };
        // A composite node is a cross-view roll-up; bundling more than one into a
        // single SELECT co-locates their independent one-to-many joins into a
        // shared CTE and trips airlayer's fan-out guard, failing the *whole* group
        // (and any additive sibling in it) — the same batching hazard the
        // instance-detail own-measure queries avoid. Give each composite its own
        // query; keep plain single-view nodes batched into one round-trip.
        let simple: Vec<&WmBreakdownNode> = group_nodes
            .iter()
            .copied()
            .filter(|n| !n.is_composite)
            .collect();
        if !simple.is_empty() {
            groups.push((
                view_name.clone(),
                simple.iter().map(|n| n.id.clone()).collect(),
                make_cfg(&simple),
            ));
        }
        for n in group_nodes.iter().copied().filter(|n| n.is_composite) {
            groups.push((view_name.clone(), vec![n.id.clone()], make_cfg(&[n])));
        }
    }
    BreakdownValuePlan { groups, unvalued }
}

#[cfg(test)]
mod breakdown_tests {
    use super::*;
    use oxy_airlayer_compat::engine::metric_tree::{
        EdgeKind, EdgeOperator, MetricEdge, MetricNode, MetricTree,
    };
    use oxy_airlayer_compat::schema::models::{
        AggregateSpace, DriverConfidence, DriverDirection, DriverForm, DriverStrength,
    };

    fn node(id: &str, view: &str, measure: &str, composite: bool) -> MetricNode {
        MetricNode {
            id: id.into(),
            view: view.into(),
            measure: measure.into(),
            label: measure.into(),
            description: None,
            measure_type: "number".into(),
            is_composite: composite,
            expr: None,
            // Irrelevant to these tests — they exercise breakdown/edge
            // resolution, not drill eligibility or response arithmetic.
            drillable: false,
            // Every node here is `type: number`; the sample tree's root is a
            // product, so neither a sum nor a mean carries over a window.
            aggregate_space: AggregateSpace::Unaggregatable,
        }
    }

    // Revenue(composite) = Orders(*) × Aov(*), all on the `store` view.
    fn sample_tree() -> MetricTree {
        let mul = |from: &str, to: &str| MetricEdge {
            from: from.into(),
            to: to.into(),
            kind: EdgeKind::Component,
            sign: 1.0,
            operator: EdgeOperator::Mul,
            direction: DriverDirection::default(),
            strength: DriverStrength::Strong,
            confidence: DriverConfidence::High,
            coefficient: None,
            // Qualitative edges: no fitted response, so no basis terms,
            // moments or observed domain.
            coefficients: vec![],
            form: DriverForm::default(),
            form_declared: true,
            intercept: None,
            moments: None,
            domain: None,
            lag: None,
            description: None,
            refs: None,
        };
        MetricTree {
            nodes: vec![
                node("store.revenue", "store", "revenue", true),
                node("store.orders", "store", "orders", false),
                node("store.aov", "store", "aov", false),
            ],
            edges: vec![
                mul("store.orders", "store.revenue"),
                mul("store.aov", "store.revenue"),
            ],
            root: None,
            warnings: vec![],
        }
    }

    #[test]
    fn structure_includes_root_and_component_children() {
        let tree = sample_tree();
        let (nodes, edges) = breakdown_structure(&tree, "store.revenue").unwrap();
        assert_eq!(nodes.len(), 3);
        assert!(nodes.iter().any(|n| n.id == "store.revenue" && n.is_root));
        assert!(!edges.is_empty());
        assert!(edges.iter().all(|e| e.operator == "mul"));
    }

    #[test]
    fn leaf_measure_yields_single_node() {
        let tree = sample_tree();
        let (nodes, edges) = breakdown_structure(&tree, "store.orders").unwrap();
        assert_eq!(nodes.len(), 1);
        assert!(edges.is_empty());
    }

    #[test]
    fn primary_view_uses_pk_filter() {
        let cols = vec!["store_id".to_string()];
        let f = build_pk_filters("store", &cols, &["s1".to_string()]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].field, "store.store_id");
    }

    /// A fact view (`orders`) joined many-to-one to a dimension view (`stores`),
    /// which in turn rolls up to `cities` — so `city` sits two hops from the
    /// measure, the shape instance scoping must handle.
    fn star_layer() -> oxy_airlayer_compat::SemanticLayer {
        let orders: oxy_airlayer_compat::View = serde_json::from_value(serde_json::json!({
            "name": "orders",
            "table": "orders",
            "entities": [
                {"name": "order", "type": "primary", "key": "order_id"},
                {"name": "retail_store", "type": "foreign", "key": "store_id"},
            ],
            "dimensions": [
                {"name": "order_id", "type": "number", "expr": "id"},
                {"name": "store_id", "type": "number", "expr": "store_id"},
            ],
        }))
        .expect("valid view");
        let stores: oxy_airlayer_compat::View = serde_json::from_value(serde_json::json!({
            "name": "stores",
            "table": "stores",
            "entities": [
                {"name": "retail_store", "type": "primary", "key": "store_id"},
                {"name": "city", "type": "foreign", "key": "city"},
            ],
            "dimensions": [
                {"name": "store_id", "type": "number", "expr": "store_id"},
                {"name": "store_name", "type": "string", "expr": "store_name"},
                {"name": "city", "type": "string", "expr": "city"},
            ],
        }))
        .expect("valid view");
        let cities: oxy_airlayer_compat::View = serde_json::from_value(serde_json::json!({
            "name": "cities",
            "table": "cities",
            "entities": [{"name": "city", "type": "primary", "key": "city"}],
            "dimensions": [{"name": "city", "type": "string", "expr": "city"}],
        }))
        .expect("valid view");
        // `customers` is unrelated — nothing joins it to `retail_store`.
        let customers: oxy_airlayer_compat::View = serde_json::from_value(serde_json::json!({
            "name": "customers",
            "table": "customers",
            "entities": [{"name": "customer", "type": "primary", "key": "customer_id"}],
            "dimensions": [{"name": "customer_id", "type": "number", "expr": "id"}],
        }))
        .expect("valid view");
        oxy_airlayer_compat::SemanticLayer::new(vec![orders, stores, cities, customers], None)
    }

    #[test]
    fn instance_scope_filters_pin_the_entity_own_view_by_pk() {
        let f = instance_scope_filters(&star_layer(), "retail_store", "7")
            .expect("retail_store has a primary view");
        assert_eq!(f.len(), 1);
        // Pin the entity's own view and let airlayer resolve the join from
        // whatever view the measure lives on — the same shape the per-instance
        // measure queries use. Pinning the *target* view through a direct FK
        // instead would refuse any measure more than one hop out.
        assert_eq!(f[0].member.as_deref(), Some("stores.store_id"));
        assert_eq!(f[0].values, vec!["7".to_string()]);
    }

    #[test]
    fn instance_scope_filters_pin_an_entity_several_hops_from_the_measure() {
        // Regression: a `city` instance sizing a measure declared on `orders`.
        // `orders` has no `city` key — the path is orders → stores → cities — so
        // an FK-on-the-target-view rule refuses it, 500ing every city panel.
        let f = instance_scope_filters(&star_layer(), "city", "Amsterdam")
            .expect("city has a primary view");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].member.as_deref(), Some("cities.city"));
        assert_eq!(f[0].values, vec!["Amsterdam".to_string()]);
    }

    #[test]
    fn instance_scope_filters_refuse_an_unknown_entity() {
        // Nothing to pin — the caller must refuse rather than silently scan the
        // population under an instance header.
        assert!(instance_scope_filters(&star_layer(), "not_an_entity", "7").is_none());
    }

    fn bnode(measure: &str, composite: bool) -> WmBreakdownNode {
        WmBreakdownNode {
            id: format!("orders.{measure}"),
            view: "orders".into(),
            measure: measure.into(),
            label: measure.into(),
            measure_type: "number".into(),
            is_composite: composite,
            is_root: false,
            expr: None,
        }
    }

    // Regression: several cross-view composites of the same view must NOT be
    // bundled into one query (that trips airlayer's fan-out guard and fails the
    // whole group). Each composite gets its own group; plain nodes stay batched.
    #[test]
    fn breakdown_isolates_each_composite_into_its_own_group() {
        let orders: oxy_airlayer_compat::View = serde_json::from_value(serde_json::json!({
            "name": "orders",
            "table": "orders",
            "entities": [{"name": "order", "type": "primary", "key": "order_id"}],
            "dimensions": [{"name": "order_id", "type": "number", "expr": "id"}],
        }))
        .expect("valid view");
        let layer = oxy_airlayer_compat::SemanticLayer::new(vec![orders], None);
        let nodes = vec![
            bnode("net_revenue", true),
            bnode("total_order_value", true),
            bnode("total_shipping_costs", true),
            bnode("total_tax_collected", false),
        ];

        let plan = breakdown_value_plan(
            &layer,
            &nodes,
            "order",
            &["1".to_string()],
            &["order_id".to_string()],
            "orders",
        );

        assert!(plan.unvalued.is_empty());
        // 3 single-composite groups + 1 batched group for the plain node.
        assert_eq!(plan.groups.len(), 4);
        // No group bundles more than one measure alongside a composite.
        for (_, node_ids, cfg) in &plan.groups {
            let has_composite = node_ids
                .iter()
                .any(|id| nodes.iter().any(|n| &n.id == id && n.is_composite));
            if has_composite {
                assert_eq!(cfg.measures.len(), 1, "composite group must be isolated");
            }
        }
        // The single plain node is batched on its own here (only one exists).
        let plain = plan
            .groups
            .iter()
            .find(|(_, ids, _)| ids.iter().any(|id| id == "orders.total_tax_collected"))
            .expect("plain node group present");
        assert_eq!(plain.1, vec!["orders.total_tax_collected".to_string()]);
    }
}
