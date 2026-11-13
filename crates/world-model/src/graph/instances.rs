//! The instance listing: a bounded scan of an entity's primary view, for the
//! instance picker.

use agentic_semantic::compile::{CompiledQuery, resolve_and_compile, resolve_and_compile_cached};
use agentic_semantic::config::{
    ScalarFilter, SemanticFilter, SemanticFilterType, SemanticOrder, SemanticQueryConfig,
};
use http::StatusCode;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::{ConfigManager, DiskSlot, ResolveWorkspaceFile};

use super::GraphError;
use super::query::*;
use super::types::*;
use super::warehouse::GraphWarehouse;

/// Core of the world-model instance listing — shared by the workspace
/// `get_world_model_instances` handler and the customer-app gate handler
/// (`oxy-api-world-model`'s `customer_app::world_model`). Compiles a bounded
/// `SELECT <pk,label> … LIMIT n` scan over the entity's primary view and runs
/// it through the agentic connector; not cached (a bounded scan, and a
/// workspace-keyed cache wouldn't invalidate on an out-of-band working-copy
/// change). `layer` is borrowed; `scan_path` / `workspace_path` come from the
/// caller — the FS scan path for the IDE, the compile-boundary revision for the
/// gate.
/// `engine` is the caller's cached engine handle when it has one. Like
/// [`measure_breakdown_core`](super::measure_breakdown_core), the two callers differ: the workspace handler
/// holds a `SemanticEngineCacheCtx`, while the customer-app handler enters
/// through `enter_semantic_boundary`, which carries no `AppState`. `None`
/// keeps the per-request build both callers used before.
///
/// `scope_filters` go INTO the query, before the LIMIT: a viewer scoped to one
/// store whose label sorts past the page must still get that store on page
/// one, which a filter applied after the scan cannot promise. The customer-app
/// route builds them from the viewer's reach (after [`instances_view`] has
/// accepted the entity, so its refusals come first); the IDE passes none.
pub async fn instances_core<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    warehouse: &dyn GraphWarehouse,
    layer: &oxy_airlayer_compat::SemanticLayer,
    scan: oxy::config::ScanDir,
    engine: Option<CachedEngine>,
    q: &WmInstancesQuery,
    scope_filters: Vec<SemanticFilter>,
) -> Result<WmInstancesResponse, GraphError>
where
    ConfigManager<S>: ResolveWorkspaceFile,
{
    let is_search = q.search.as_deref().is_some_and(|s| !s.is_empty());
    let view = instances_view(layer, &q.entity)?;

    // Look up display_field from .world-model.yml (silently ignore load errors here —
    // the instances endpoint is a picker convenience, not security-critical).
    // Compile boundary first (serve replicas have no working copy), FS fallback.
    let display_field = crate::WorldModelConfig::resolve(&workspace_manager.config_manager)
        .await
        .ok()
        .flatten()
        .and_then(|cfg| cfg.entities.into_iter().find(|e| e.id == q.entity))
        .and_then(|ec| ec.display_field);
    let disp = EntityDisplaySpec::for_entity(view, &q.entity, display_field.as_deref());

    // Build the semantic query: PK dimension(s) + optional display label dimension.
    // resolve_and_compile handles table aliasing, dialect, and database routing.
    // `scan_path` is caller-supplied (FS or compile-boundary tempdir).
    let databases = database_configs(&workspace_manager);

    let order_by = disp.dims.first().cloned().unwrap_or_default();

    // Choose the search filter based on what the display field is:
    //   - text label dim  → Contains (substring LIKE)
    //   - PK as display   → Equals (exact key match)
    let search_filter: Vec<SemanticFilter> = match q.search.as_deref().filter(|s| !s.is_empty()) {
        None => vec![],
        Some(term) => {
            let (field, op) = if disp.has_label_dim {
                let label_field = disp.dims[disp.pk_count].clone();
                (
                    label_field,
                    SemanticFilterType::Contains(ScalarFilter { value: term.into() }),
                )
            } else {
                let pk_field = disp.dims.first().cloned().unwrap_or_default();
                (
                    pk_field,
                    SemanticFilterType::Eq(ScalarFilter { value: term.into() }),
                )
            };
            vec![SemanticFilter {
                field,
                filter_type: op,
            }]
        }
    };

    // For no-search: scan limit+1 to detect whether more records exist.
    let scan_limit = Some((q.limit as u64) + if is_search { 0 } else { 1 });

    let mut filters = search_filter;
    filters.extend(scope_filters);

    let semantic_config = SemanticQueryConfig {
        topic: None,
        dimensions: disp.dims.clone(),
        measures: vec![],
        time_dimensions: vec![],
        filters,
        orders: vec![SemanticOrder {
            field: order_by,
            direction: "asc".to_string(),
        }],
        limit: scan_limit,
        offset: None,
    };

    let layer_clone = layer.clone();
    let (base_sql, database_name) = tokio::task::spawn_blocking(move || match engine {
        // `cached.databases`, not the local `databases`: the fingerprint in
        // `cached.key` describes THAT list, and caching an engine under a
        // fingerprint that does not describe it is the dialect bug one level in.
        Some(cached) => resolve_and_compile_cached(
            &cached.cache,
            cached.key,
            &scan,
            &cached.databases,
            &semantic_config,
            None,
            Some(layer_clone),
        ),
        None => resolve_and_compile(&scan, &databases, &semantic_config, None, Some(layer_clone)),
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))
    .map(|compiled| match compiled {
        CompiledQuery::Warehouse { sql, database_name } => (sql, database_name),
        // This call passes no `PreaggContext`, so the compiler never returns
        // this variant — but if one is ever attached here, take the warehouse
        // SQL the variant carries rather than the rollup's. The rollup SQL is
        // DuckDB `read_parquet(...)` and is executed by
        // `agentic_semantic::preagg`, never by the warehouse connector this
        // path hands it to; the old arm also blanked the database name, which
        // would have routed it to whichever database `config.yml` lists first.
        CompiledQuery::Preaggregation {
            warehouse_sql,
            warehouse_database,
            ..
        } => (warehouse_sql, warehouse_database),
    })?;

    let rows = warehouse
        .query(&database_name, base_sql)
        .await
        .unwrap_or_default();

    let all_items: Vec<WmInstanceItem> = rows
        .into_iter()
        .skip(1) // skip header row
        .map(|row| {
            let key = row.first().cloned().unwrap_or_default();
            let display = disp.display_from_row(&row);
            let display = if display.is_empty() {
                key.clone()
            } else {
                display
            };
            WmInstanceItem {
                key,
                display,
                location: None,
            }
        })
        .collect();

    // For non-search we fetched limit+1 rows to detect overflow; trim to limit.
    let has_more = !is_search && all_items.len() > q.limit;
    let items: Vec<WmInstanceItem> = if has_more {
        all_items.into_iter().take(q.limit).collect()
    } else {
        all_items
    };
    let total = items.len();

    let response = WmInstancesResponse {
        total,
        has_more,
        items,
    };
    Ok(response)
}

/// The entity's primary view, or the refusal an instances
/// request gets: not found, no table, no key columns — in that order.
pub fn instances_view<'a>(
    layer: &'a oxy_airlayer_compat::SemanticLayer,
    entity: &str,
) -> Result<&'a oxy_airlayer_compat::View, GraphError> {
    let view = primary_view_of(layer, entity).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("Entity '{}' not found", entity),
        )
    })?;
    let _table = view
        .table
        .as_deref()
        .or(view.sql.as_deref())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("Entity '{}' has no table", entity),
            )
        })?;
    if entity_keys_in_view(view, entity, true).is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Entity '{}' has no key columns", entity),
        ));
    }
    Ok(view)
}
