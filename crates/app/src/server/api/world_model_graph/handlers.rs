use axum::{
    extract::{self, Path},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
};
use futures::stream::{FuturesUnordered, StreamExt as _};
use oxy_airlayer_compat::schema::models::{EntityType, MeasureType};
use std::collections::HashMap;

use agentic_semantic::config::{
    ScalarFilter, SemanticFilter, SemanticFilterType, SemanticOrder, SemanticQueryConfig,
};
use oxy_auth::extractor::AuthenticatedUserExtractor;

use crate::server::api::data::{
    SQLParams, SemanticQueryResponse, build_connector, run_via_agentic_connector,
    run_with_connector,
};
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, SemanticEngineCacheCtx, SemanticLayerCacheCtx, WorkspaceManagerReadOnly,
};
use crate::server::api::semantic::{ErrorResponse, WorkspacePath};
use oxy::utils::create_sse_stream;

use super::source::*;
use super::warehouse::WorkspaceWarehouse;
use oxy_world_model::graph::query::*;
use oxy_world_model::graph::types::*;
use oxy_world_model::graph::{build_world_model_response, instances_core, measure_breakdown_core};

/// `GET /{workspace_id}/semantic/world-model`
///
/// Returns the entity-centric world model: every primary entity in the
/// semantic model, its own and induced measures (with operator and
/// additivity metadata), and the promotion edges between entities.
pub async fn get_world_model(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    layer_cache: SemanticLayerCacheCtx,
    Path(WorkspacePath { workspace_id: _ }): Path<WorkspacePath>,
) -> Result<extract::Json<WorldModelResponse>, (StatusCode, extract::Json<ErrorResponse>)> {
    let source = ModelSource::resolve(&workspace_manager).await?;
    let layer = source.layer(&layer_cache).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            extract::Json(ErrorResponse {
                message: format!("Failed to load semantic model: {e}"),
            }),
        )
    })?;

    let config = display_config(&workspace_manager).await?;
    build_world_model_response(&layer, config.as_ref())
        .map(extract::Json)
        .map_err(|message| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                extract::Json(ErrorResponse { message }),
            )
        })
}

/// `GET /{workspace_id}/semantic/world-model/instances`
pub async fn get_world_model_instances(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    layer_cache: SemanticLayerCacheCtx,
    engine_cache: SemanticEngineCacheCtx,
    axum::extract::State(_app_state): axum::extract::State<crate::server::router::AppState>,
    Path(WorkspacePath { workspace_id: _ }): Path<WorkspacePath>,
    axum::extract::Query(q): axum::extract::Query<WmInstancesQuery>,
) -> Result<extract::Json<WmInstancesResponse>, (StatusCode, extract::Json<ErrorResponse>)> {
    let source = ModelSource::resolve(&workspace_manager).await?;
    let layer = source.layer(&layer_cache).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            extract::Json(ErrorResponse {
                message: format!("Failed to load layer: {e}"),
            }),
        )
    })?;
    refuse_entity_database(&workspace_manager, &layer, &q.entity)?;
    let engine = Some(cached_engine_for_source(
        &engine_cache,
        &workspace_manager,
        &source,
    ));
    let warehouse = WorkspaceWarehouse {
        workspace_manager: workspace_manager.clone(),
        user_id: user.id,
        role,
    };
    // The IDE's list is the warehouse's view of the world: no reach, no places.
    instances_core(
        &workspace_manager,
        &warehouse,
        &layer,
        source.scan(),
        engine,
        &q,
        vec![],
    )
    .await
    .map(extract::Json)
    .map_err(|(status, message)| (status, extract::Json(ErrorResponse { message })))
}

/// `GET /{workspace_id}/semantic/world-model/filter-instances`
///
/// Paginated, searchable listing of the rows of `entity` reachable from the
/// selected instance (`seed_entity` / `seed_key`) — the full set the node card
/// only previews as a handful of sample chips. Backs the "+N more" sample
/// browser popover.
pub async fn get_world_model_filter_instances(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    layer_cache: SemanticLayerCacheCtx,
    engine_cache: SemanticEngineCacheCtx,
    axum::extract::State(_app_state): axum::extract::State<crate::server::router::AppState>,
    axum::extract::Query(q): axum::extract::Query<WmFilterInstancesQuery>,
) -> Result<extract::Json<WmInstancesResponse>, (StatusCode, extract::Json<ErrorResponse>)> {
    let err = |code: StatusCode, message: String| (code, extract::Json(ErrorResponse { message }));

    let (layer, promotions, source) = load_walkable_model(&workspace_manager, &layer_cache).await?;
    let wm_cfg = resolve_world_model_config(&workspace_manager).await?;
    let entity_metas = build_entity_metas(&layer, &promotions, wm_cfg.as_ref());

    let target_view = primary_view_of(&layer, &q.entity).ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            format!("Entity '{}' not found", q.entity),
        )
    })?;
    let display_field = wm_cfg
        .as_ref()
        .and_then(|cfg| cfg.entities.iter().find(|e| e.id == q.entity))
        .and_then(|ec| ec.display_field.clone());
    let disp = EntityDisplaySpec::for_entity(target_view, &q.entity, display_field.as_deref());
    if disp.dims.is_empty() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            format!("Entity '{}' has no key columns", q.entity),
        ));
    }

    let databases = database_configs(&workspace_manager);
    let engine_key = source.engine_key(&engine_cache, &databases);
    let exec = WmExecCtx {
        warehouse: std::sync::Arc::new(WorkspaceWarehouse {
            workspace_manager: workspace_manager.clone(),
            user_id: user.id,
            role: role.clone(),
        }),
        scan: source.scan(),
        databases,
        layer: (*layer).clone(),
        engine_cache: engine_cache.cache.clone(),
        engine_key,
    };

    // Resolve the reachability filter (seed instance → target entity). Unreachable
    // ⇒ empty page rather than an error (the node card simply had nothing more).
    let Some(mut filters) =
        resolve_target_filters(&exec, &entity_metas, &q.seed_entity, &q.seed_key, &q.entity).await
    else {
        return Ok(extract::Json(WmInstancesResponse {
            total: 0,
            has_more: false,
            items: vec![],
        }));
    };

    // Optional search — Contains on the label dim, Eq on a bare PK (mirrors the
    // instance-picker endpoint).
    if let Some(term) = q.search.as_deref().filter(|s| !s.is_empty()) {
        let (field, op) = if disp.has_label_dim {
            (
                disp.dims[disp.pk_count].clone(),
                SemanticFilterType::Contains(ScalarFilter { value: term.into() }),
            )
        } else {
            (
                disp.dims.first().cloned().unwrap_or_default(),
                SemanticFilterType::Eq(ScalarFilter { value: term.into() }),
            )
        };
        filters.push(SemanticFilter {
            field,
            filter_type: op,
        });
    }

    let order_by = disp.dims.first().cloned().unwrap_or_default();
    // Overflow probe: fetch limit+1 (with offset) to detect a next page.
    let cfg = SemanticQueryConfig {
        topic: None,
        dimensions: disp.dims.clone(),
        measures: vec![],
        time_dimensions: vec![],
        filters,
        orders: vec![SemanticOrder {
            field: order_by,
            direction: "asc".to_string(),
        }],
        limit: Some((q.limit as u64) + 1),
        offset: Some(q.offset as u64),
    };

    let (sql, database_name) = exec.compile_full(cfg).await.ok_or_else(|| {
        err(
            StatusCode::BAD_REQUEST,
            "failed to compile query".to_string(),
        )
    })?;

    let payload = SQLParams {
        sql,
        database: database_name,
        filters: None,
        connections: None,
        result_format: None,
        untyped: false,
    };
    let rows = match run_via_agentic_connector(&workspace_manager, user.id, role, &payload).await {
        Ok(SemanticQueryResponse::Json(r)) => r,
        _ => vec![],
    };

    let mut all_items: Vec<WmInstanceItem> = rows
        .into_iter()
        .skip(1) // header row
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

    let has_more = all_items.len() > q.limit;
    all_items.truncate(q.limit);
    let total = all_items.len();

    Ok(extract::Json(WmInstancesResponse {
        total,
        has_more,
        items: all_items,
    }))
}

/// `POST /{workspace_id}/semantic/world-model/filter-counts`
pub async fn post_world_model_filter_counts(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    layer_cache: SemanticLayerCacheCtx,
    engine_cache: SemanticEngineCacheCtx,
    axum::extract::State(_app_state): axum::extract::State<crate::server::router::AppState>,
    Path(WorkspacePath { workspace_id: _ }): Path<WorkspacePath>,
    extract::Json(req): extract::Json<WmFilterCountsRequest>,
) -> Result<
    Sse<impl futures::Stream<Item = Result<Event, axum::Error>>>,
    (StatusCode, extract::Json<ErrorResponse>),
> {
    let (layer, promotions, source) = load_walkable_model(&workspace_manager, &layer_cache).await?;

    // World-model config supplies per-entity display fields used to render sample
    // labels on descendant cards (mirrors the instance-detail handler).
    let wm_cfg = resolve_world_model_config(&workspace_manager).await?;

    // Collect per-entity metadata needed to build semantic queries (struct
    // hoisted to module scope so the expansion-plan helpers can share it).
    let entity_metas: Vec<EntityMeta> = build_entity_metas(&layer, &promotions, wm_cfg.as_ref());

    let databases = database_configs(&workspace_manager);

    // Get-or-build the cached engine. `SemanticEngine` is `Send + Sync`, so this
    // is a bare `Arc` shared by every compile below — no lock, and nothing a
    // cancelled request could drop mid-compile.
    let cached_engine = engine_cache
        .get_or_build(
            // Keyed by what the scan read: the compiled revision, or the
            // working copy on a node that owns one.
            source.engine_key(&engine_cache, &databases),
            layer.clone(),
            databases.clone(),
        )
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "world-model: semantic engine unavailable");
            e
        })
        .ok();

    // Compile all total-count queries up front against that one engine, before
    // the spawn so we can move cached_engine into the spawned task. The closure
    // is wrapped in a block so it drops (releasing its borrows) before we move
    // those variables into the background spawn below.
    let total_sqls: Vec<Option<String>> = {
        let batch_compile_outer = |cfgs: Vec<SemanticQueryConfig>| {
            let engine_arc = cached_engine.clone();
            tokio::task::spawn_blocking(move || {
                let compile_one = |cfg: &SemanticQueryConfig| -> Option<String> {
                    let engine = engine_arc.as_ref()?;
                    agentic_semantic::compile_with_engine(engine, cfg).ok()
                };
                let sqls: Vec<Option<String>> = cfgs.iter().map(compile_one).collect();
                Ok::<_, agentic_semantic::SemanticError>(sqls)
            })
        };
        let total_cfgs: Vec<SemanticQueryConfig> = entity_metas
            .iter()
            .map(|meta| SemanticQueryConfig {
                topic: None,
                dimensions: vec![],
                measures: vec![format!("{}.__oxy_row_count", meta.view_name)],
                time_dimensions: vec![],
                filters: vec![],
                orders: vec![],
                limit: None,
                offset: None,
            })
            .collect();
        let t_compile = std::time::Instant::now();
        let sqls = batch_compile_outer(total_cfgs)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_else(|| vec![None; entity_metas.len()]);
        tracing::info!(
            elapsed_ms = t_compile.elapsed().as_millis(),
            "filter-counts: compiled total-count queries"
        );
        sqls
        // batch_compile_outer drops here — borrows of cached_engine/layer/databases end
    };

    // Extract only the fields needed by the total-count task so entity_metas
    // can be moved into the BFS task without cloning the full structs.
    struct TotalWork {
        entity_name: String,
        datasource: String,
        sql: String,
    }
    let total_works: Vec<TotalWork> = entity_metas
        .iter()
        .zip(total_sqls)
        .filter_map(|(meta, sql_opt)| {
            Some(TotalWork {
                entity_name: meta.entity_name.clone(),
                datasource: meta.datasource.clone(),
                sql: sql_opt?,
            })
        })
        .collect();

    let user_id = user.id;

    // Clone data needed by the background task before moving anything.
    // The layer itself is no longer cloned in: the BFS compiles against the
    // cached engine, which already owns it.
    let wm_a = workspace_manager.clone();
    let wm_b = workspace_manager;
    let role_a = role.clone();
    let role_b = role;

    // ── Stream results back as they complete ──────────────────────────────
    //
    // Total counts (Task A) and BFS matched counts (Task B) run concurrently
    // inside a single spawned task.  Each result is sent through an mpsc
    // channel and yielded as an SSE event, so the browser sees node counts
    // appear progressively rather than waiting for all queries to finish.
    let (tx, rx) = tokio::sync::mpsc::channel::<WmFilterCountEvent>(256);

    tokio::spawn(async move {
        let tx_a = tx.clone();
        let tx_b = tx.clone();

        tokio::join!(
            // ── Task A: total count per entity — stream as each query completes
            async move {
                let mut futs: FuturesUnordered<_> = total_works
                    .into_iter()
                    .map(|w| {
                        let wm = wm_a.clone();
                        let role_c = role_a.clone();
                        async move {
                            let t0 = std::time::Instant::now();
                            let Ok(connector) =
                                build_connector(&wm, user_id, role_c, &w.datasource).await
                            else {
                                tracing::warn!(
                                    entity_name = %w.entity_name,
                                    "filter-counts: build_connector failed"
                                );
                                return (w.entity_name, 0u64);
                            };
                            let build_ms = t0.elapsed().as_millis();
                            let q0 = std::time::Instant::now();
                            let cnt = run_with_connector(&connector, &w.sql)
                                .await
                                .into_iter()
                                .next()
                                .and_then(|r| r.into_iter().next())
                                .and_then(|v: String| v.parse::<u64>().ok())
                                .unwrap_or(0);
                            let query_ms = q0.elapsed().as_millis();
                            tracing::debug!(
                                entity_name = %w.entity_name,
                                build_ms,
                                query_ms,
                                "filter-counts: total count"
                            );
                            (w.entity_name, cnt)
                        }
                    })
                    .collect();

                let t_exec = std::time::Instant::now();
                let mut n = 0usize;
                while let Some((name, total)) = futs.next().await {
                    n += 1;
                    let sent = tx_a
                        .send(WmFilterCountEvent {
                            entity_name: name,
                            total: Some(total),
                            matched: None,
                            sample: vec![],
                            sample_keys: vec![],
                            done: false,
                        })
                        .await;
                    if sent.is_err() {
                        // Receiver dropped — client disconnected (e.g. re-filtered
                        // before this stream finished). Stop draining `futs`;
                        // dropping it below cancels any still-in-flight queries
                        // instead of letting them run to completion unread.
                        tracing::debug!(
                            "filter-counts: total-count receiver dropped, stopping early"
                        );
                        break;
                    }
                }
                tracing::info!(
                    elapsed_ms = t_exec.elapsed().as_millis(),
                    n,
                    "filter-counts: total counts streamed"
                );
            },
            // ── Task B: undirected BFS over the entity link graph ─────────────
            //
            // Instance drill-down follows the *navigable link graph* — the parent
            // spine PLUS foreign cross-links, i.e. exactly the edges drawn in the
            // graph — not just the parent tree. From the selected instance we
            // expand in BOTH directions, hop by hop:
            //
            //   • coarser (outbound): the current entity's own FK → the target's
            //     PK (store → city, store → region);
            //   • finer (inbound): entities whose FK points AT the current entity
            //     (store ← order — the cross-link case the parent tree missed).
            //
            // Each hop carries the accumulated PK-filter set. A visited set keyed
            // on entity (not path) makes every entity count exactly once and
            // guards against cycles and diamonds. BFS hop distance from the seed
            // defines the streaming order (still one SSE burst per level).
            async move {
                // Reconstruct batch_compile inside this block so it's owned
                // (no reference to outer function stack after tokio::spawn).
                // The engine is a bare `Arc<SemanticEngine>` from the cache —
                // shared, not locked — so a batch is just N compiles against it.
                let batch_compile = |cfgs: Vec<SemanticQueryConfig>| {
                    let engine_arc = cached_engine.clone();
                    tokio::task::spawn_blocking(move || {
                        let sqls: Vec<Option<String>> = match engine_arc {
                            Some(ref engine) => cfgs
                                .iter()
                                .map(|cfg| agentic_semantic::compile_with_engine(engine, cfg).ok())
                                .collect(),
                            None => vec![None; cfgs.len()],
                        };
                        Ok::<_, agentic_semantic::SemanticError>(sqls)
                    })
                };

                // Seed matched = 1 (the record itself) — emit immediately.
                tx_b.send(WmFilterCountEvent {
                    entity_name: req.entity_id.clone(),
                    matched: Some(1),
                    total: None,
                    sample: vec![],
                    sample_keys: vec![],
                    done: false,
                })
                .await
                .ok();

                // Everything a BFS hop needs to expand FROM an entity, produced by
                // that entity's single expansion query: its matched PK rows (each a
                // tuple of column values, one per `pk_dim_ref`; composite keys keep
                // all columns so per-column IN filters stay correct) and, per
                // outbound link, the distinct FK values pointing at the target.
                struct NeighborData {
                    pk_rows: Vec<Vec<serde_json::Value>>,
                    /// target entity → distinct FK values on this entity's side.
                    fk_values: HashMap<String, Vec<serde_json::Value>>,
                }
                let mut neighbor_data: HashMap<String, NeighborData> = HashMap::new();

                // entity_name → index into entity_metas, for O(1) neighbour lookup.
                let meta_idx: HashMap<&str, usize> = entity_metas
                    .iter()
                    .enumerate()
                    .map(|(i, m)| (m.entity_name.as_str(), i))
                    .collect();

                // Reverse adjacency: target entity → its *finer* neighbours (the
                // inbound direction the parent tree never provided for
                // cross-links, e.g. store ← order). Shared helper — also used
                // by `resolve_target_filters`'s legacy fallback.
                let inbound = build_inbound_index(&entity_metas);

                // Schema-level reachability (pure, no IO) decides which path
                // this request takes. Every entity pair the world model draws
                // an edge for is automatically joinable by airlayer — its join
                // graph is derived from the exact same FK/PK entity metadata
                // this file already reads to build `EntityLink` — UNLESS the
                // pair spans two different datasources, since airlayer rejects
                // cross-dialect joins. So: when every reachable entity shares
                // the seed's datasource, skip the BFS entirely and fire one
                // direct-join count+sample query per entity — no per-hop
                // materialization, no unbounded expansion query. Otherwise
                // fall back to the legacy per-hop BFS below, which threads
                // matched values through as literal `IN (...)` filters and
                // stays correct across datasources.
                let seed_idx_opt = meta_idx.get(req.entity_id.as_str()).copied();
                let (reachable, any_cross_datasource) = match seed_idx_opt {
                    Some(seed_idx) => {
                        let reachable =
                            schema_reachable_entities(&entity_metas, &inbound, &meta_idx, seed_idx);
                        let seed_ds = entity_metas[seed_idx].datasource.clone();
                        let cross = reachable
                            .iter()
                            .any(|&i| entity_metas[i].datasource != seed_ds);
                        (reachable, cross)
                    }
                    None => (vec![], false),
                };

                if let Some(seed_idx) = seed_idx_opt.filter(|_| !any_cross_datasource) {
                    // ── Fast path: direct join back to the seed ───────────────
                    //
                    // A single query referencing the target entity's own view
                    // (measures/dimensions) plus a filter on the seed's view
                    // lets airlayer resolve the *entire* join chain back to
                    // the seed automatically — however many hops apart.
                    //
                    // A real scalar `COUNT(*)` measure query — no row
                    // fetching, no client-side dedup — now that
                    // github.com/oxy-hq/airlayer@64163f5 fixed the bug where
                    // its fan-out-protection CTE builder derived its join
                    // scope only from `dimensions`, ignoring `filters`
                    // entirely: a filter on a view other than the measure's
                    // own (this seed filter, exactly) was silently never
                    // joined into the CTE, so the aggregate could evaluate
                    // over the wrong (or no) scope. Before that fix this had
                    // to be worked around with a dimension-only projection +
                    // client-side row fetch/dedup, which reintroduced the
                    // unbounded-memory risk this whole design set out to
                    // avoid. See [[airlayer-fanout-protection-zero-dim-bug]]
                    // in project memory for the full history.
                    let filters = seed_self_filters(&entity_metas[seed_idx], &req.key_value);
                    if filters.is_empty() {
                        return;
                    }

                    struct DirectWork<'a> {
                        meta: &'a EntityMeta,
                        count_cfg: SemanticQueryConfig,
                        sample_cfg: Option<SemanticQueryConfig>,
                    }
                    let works: Vec<DirectWork> = reachable
                        .iter()
                        .map(|&idx| {
                            let meta = &entity_metas[idx];
                            let count_cfg = SemanticQueryConfig {
                                topic: None,
                                dimensions: vec![],
                                measures: vec![format!("{}.__oxy_row_count", meta.view_name)],
                                time_dimensions: vec![],
                                filters: filters.clone(),
                                orders: vec![],
                                limit: None,
                                offset: None,
                            };
                            let sample_cfg =
                                (!meta.sample_dims.is_empty()).then(|| SemanticQueryConfig {
                                    topic: None,
                                    dimensions: meta.sample_dims.clone(),
                                    measures: vec![],
                                    time_dimensions: vec![],
                                    filters: filters.clone(),
                                    // Order ascending on the first sample dim so preview
                                    // chips are stable across reloads and align with the
                                    // ascending Sample Browser (`get_world_model_filter_instances`).
                                    orders: vec![SemanticOrder {
                                        field: meta.sample_dims[0].clone(),
                                        direction: "asc".to_string(),
                                    }],
                                    limit: Some(3),
                                    offset: None,
                                });
                            DirectWork {
                                meta,
                                count_cfg,
                                sample_cfg,
                            }
                        })
                        .collect();

                    let all_cfgs: Vec<SemanticQueryConfig> = works
                        .iter()
                        .flat_map(|w| {
                            let mut v = vec![w.count_cfg.clone()];
                            if let Some(ref s) = w.sample_cfg {
                                v.push(s.clone());
                            }
                            v
                        })
                        .collect();

                    let t_compile = std::time::Instant::now();
                    let all_sqls: Vec<Option<String>> = batch_compile(all_cfgs)
                        .await
                        .ok()
                        .and_then(|r| r.ok())
                        .unwrap_or_else(|| {
                            vec![
                                None;
                                works
                                    .iter()
                                    .map(|w| 1 + w.sample_cfg.is_some() as usize)
                                    .sum()
                            ]
                        });
                    tracing::info!(
                        elapsed_ms = t_compile.elapsed().as_millis(),
                        n = works.len(),
                        "filter-counts direct-join: compiled queries"
                    );

                    let mut sql_iter = all_sqls.into_iter();
                    let exec_futures: Vec<_> = works
                        .into_iter()
                        .map(|w| {
                            let count_sql = sql_iter.next().flatten();
                            let sample_sql = w
                                .sample_cfg
                                .as_ref()
                                .and_then(|_| sql_iter.next().flatten());
                            let datasource = w.meta.datasource.clone();
                            let entity_name = w.meta.entity_name.clone();
                            let pk_count = w.meta.pk_count;
                            let has_label_dim = w.meta.has_label_dim;
                            let wm = wm_b.clone();
                            let role_c = role_b.clone();
                            async move {
                                let connector = build_connector(&wm, user_id, role_c, &datasource)
                                    .await
                                    .ok();
                                let (matched, (sample, sample_keys)) = match connector.as_ref() {
                                    Some(c) => tokio::join!(
                                        async {
                                            match count_sql {
                                                Some(sql) => run_with_connector(c, &sql)
                                                    .await
                                                    .into_iter()
                                                    .next()
                                                    .and_then(|r| r.into_iter().next())
                                                    .and_then(|v: String| v.parse::<u64>().ok())
                                                    .unwrap_or(0),
                                                None => 0,
                                            }
                                        },
                                        async {
                                            match sample_sql {
                                                Some(sql) => run_with_connector(c, &sql)
                                                    .await
                                                    .into_iter()
                                                    .map(|r| {
                                                        sample_row_to_display_key(
                                                            &r,
                                                            pk_count,
                                                            has_label_dim,
                                                        )
                                                    })
                                                    .unzip(),
                                                None => (vec![], vec![]),
                                            }
                                        },
                                    ),
                                    None => (0, (vec![], vec![])),
                                };
                                (entity_name, matched, sample, sample_keys)
                            }
                        })
                        .collect();

                    let t_exec = std::time::Instant::now();
                    let mut futs: FuturesUnordered<_> = exec_futures.into_iter().collect();
                    let mut n = 0usize;
                    while let Some((entity_name, matched, sample, sample_keys)) = futs.next().await
                    {
                        n += 1;
                        let sent = tx_b
                            .send(WmFilterCountEvent {
                                entity_name,
                                matched: Some(matched),
                                total: None,
                                sample,
                                sample_keys,
                                done: false,
                            })
                            .await;
                        if sent.is_err() {
                            tracing::debug!(
                                "filter-counts direct-join: receiver dropped, stopping early"
                            );
                            break;
                        }
                    }
                    tracing::info!(
                        elapsed_ms = t_exec.elapsed().as_millis(),
                        n,
                        "filter-counts direct-join: all queries done"
                    );
                    return;
                }

                // ── Legacy fallback: per-hop BFS with IN(...) threading ───────
                //
                // Used when the seed entity is unknown to the model, or when
                // the reachable set spans more than one datasource (airlayer
                // cannot join across datasources, so the direct-join fast path
                // above is unavailable and matched values must be threaded
                // through as literal filters instead).
                //
                // Whether an entity has any onward neighbour to expand into — an
                // outbound link or an inbound child. Only such entities run the
                // full (all-rows) expansion query; terminal nodes get a cheap
                // scalar count + limited sample instead.
                let is_expandable = |name: &str| -> bool {
                    meta_idx
                        .get(name)
                        .is_some_and(|&i| !entity_metas[i].links.is_empty())
                        || inbound.contains_key(name)
                };

                // Run one entity's compiled expansion `sql` and parse it. Empty
                // result on any failure so the BFS just treats the node as unmatched.
                let run_expansion = |datasource: String,
                                     sql: Option<String>,
                                     plan: WmExpansionPlan| {
                    let wm = wm_b.clone();
                    let role = role_b.clone();
                    async move {
                        let empty = || WmExpansionResult {
                            matched: 0,
                            pk_rows: vec![],
                            fk_values: vec![],
                            sample: vec![],
                            sample_keys: vec![],
                        };
                        let Some(sql) = sql else { return empty() };
                        let Ok(connector) = build_connector(&wm, user_id, role, &datasource).await
                        else {
                            return empty();
                        };
                        let rows = run_with_connector(&connector, &sql).await;
                        parse_expansion_rows(&rows, &plan)
                    }
                };

                // ── Seed pre-fetch: learn the seed's outbound FK values (and PK
                // rows) so the first hop can expand from it. A seed with no links
                // only needs its PK, which we already have (the picked key).
                if is_expandable(&req.entity_id)
                    && let Some(&i) = meta_idx.get(req.entity_id.as_str())
                    && !entity_metas[i].links.is_empty()
                {
                    let meta = &entity_metas[i];
                    let plan = wm_expansion_plan(meta);
                    let seed_cfg = SemanticQueryConfig {
                        topic: None,
                        dimensions: plan.dims.clone(),
                        measures: vec![],
                        time_dimensions: vec![],
                        filters: seed_self_filters(meta, &req.key_value),
                        orders: vec![],
                        limit: None,
                        offset: None,
                    };
                    let sql = batch_compile(vec![seed_cfg])
                        .await
                        .ok()
                        .and_then(|r| r.ok())
                        .and_then(|mut v| v.pop())
                        .flatten();
                    let res = run_expansion(meta.datasource.clone(), sql, plan).await;
                    let pk_rows = if res.pk_rows.is_empty() {
                        vec![seed_pk_row(&req.key_value)]
                    } else {
                        res.pk_rows
                    };
                    neighbor_data.insert(
                        req.entity_id.clone(),
                        NeighborData {
                            pk_rows,
                            fk_values: res.fk_values.into_iter().collect(),
                        },
                    );
                } else {
                    neighbor_data.insert(
                        req.entity_id.clone(),
                        NeighborData {
                            pk_rows: vec![seed_pk_row(&req.key_value)],
                            fk_values: HashMap::new(),
                        },
                    );
                }

                let mut visited: std::collections::HashSet<String> =
                    std::collections::HashSet::from([req.entity_id.clone()]);
                let mut frontier: Vec<String> = vec![req.entity_id.clone()];
                let mut depth = 0usize;
                let t_bfs = std::time::Instant::now();

                // BFS: expand the frontier one hop at a time until nothing new is
                // reachable. Each iteration discovers the next ring of entities and
                // emits one SSE burst for them (keeps the progressive-reveal UX).
                while !frontier.is_empty() {
                    depth += 1;

                    // ── Assemble filters for every newly discovered entity ────────
                    // No queries here — each frontier entity already carries what it
                    // needs to expand (its `NeighborData`, produced by its own
                    // expansion query at the previous level / seed pre-fetch):
                    //   • inbound (finer) children resolve via `child_fk_filters`
                    //     over the frontier entity's PK rows;
                    //   • outbound (coarser) targets use the frontier entity's
                    //     pre-resolved FK values as an `IN` filter on the target PK.
                    // First-writer wins per entity so a diamond within one level
                    // still yields a single count.
                    let mut chosen: HashMap<usize, Vec<agentic_semantic::config::SemanticFilter>> =
                        HashMap::new();
                    for e_name in &frontier {
                        let Some(nd) = neighbor_data.get(e_name) else {
                            continue;
                        };
                        if nd.pk_rows.is_empty() {
                            continue;
                        }
                        // Inbound: children whose FK points at this frontier entity.
                        if let Some(children) = inbound.get(e_name.as_str()) {
                            for &(child_idx, fk_refs) in children {
                                let child_name = &entity_metas[child_idx].entity_name;
                                if visited.contains(child_name) || chosen.contains_key(&child_idx) {
                                    continue;
                                }
                                let f = child_fk_filters(fk_refs, &nd.pk_rows);
                                if f.is_empty() {
                                    continue;
                                }
                                chosen.insert(child_idx, f);
                            }
                        }
                        // Outbound: coarser targets, filtered by the pre-resolved FK
                        // values (no per-level FK-select query anymore).
                        let Some(&e_idx) = meta_idx.get(e_name.as_str()) else {
                            continue;
                        };
                        for link in &entity_metas[e_idx].links {
                            let Some(&t_idx) = meta_idx.get(link.target_entity.as_str()) else {
                                continue;
                            };
                            if visited.contains(&link.target_entity) || chosen.contains_key(&t_idx)
                            {
                                continue;
                            }
                            let Some(values) = nd.fk_values.get(&link.target_entity) else {
                                continue;
                            };
                            if values.is_empty() {
                                continue;
                            }
                            let Some(pk_field) = entity_metas[t_idx].pk_dim_refs.first().cloned()
                            else {
                                continue;
                            };
                            chosen.insert(
                                t_idx,
                                vec![agentic_semantic::config::SemanticFilter {
                                    field: pk_field,
                                    filter_type: agentic_semantic::config::SemanticFilterType::In(
                                        agentic_semantic::config::ArrayFilter {
                                            values: values.clone(),
                                        },
                                    ),
                                }],
                            );
                        }
                    }

                    if chosen.is_empty() {
                        break;
                    }

                    // ── Build each node's query. Expandable nodes run ONE expansion
                    // query (PK + outbound-FK + label columns → matched, PK rows, FK
                    // values, and sample from a single scan). Terminal nodes run a
                    // cheap scalar count + a limited sample instead.
                    struct NodeWork<'a> {
                        meta: &'a EntityMeta,
                        expandable: bool,
                        plan: WmExpansionPlan,
                        /// Expansion query (expandable) or `__oxy_row_count` (leaf).
                        primary_cfg: SemanticQueryConfig,
                        /// Only for terminal nodes: a separate limited sample query.
                        sample_cfg: Option<SemanticQueryConfig>,
                    }
                    // Entities discovered THIS level — like `visited`, they are
                    // already accounted for, so a node whose only neighbours are in
                    // this set (or visited) has nothing new to reach and takes the
                    // cheap scalar-count path instead of the all-rows expansion.
                    let chosen_names: std::collections::HashSet<&str> = chosen
                        .keys()
                        .map(|&i| entity_metas[i].entity_name.as_str())
                        .collect();
                    let has_new_neighbor = |meta: &EntityMeta| -> bool {
                        let unseen = |n: &str| !visited.contains(n) && !chosen_names.contains(n);
                        meta.links.iter().any(|l| unseen(&l.target_entity))
                            || inbound.get(meta.entity_name.as_str()).is_some_and(|ch| {
                                ch.iter()
                                    .any(|&(ci, _)| unseen(&entity_metas[ci].entity_name))
                            })
                    };
                    let node_works: Vec<NodeWork<'_>> = chosen
                        .into_iter()
                        .map(|(idx, filters)| {
                            let meta = &entity_metas[idx];
                            // Only run the full (all-rows) expansion when there is a
                            // genuinely new neighbour to reach from this node; else a
                            // scalar count + limited sample suffices.
                            let expandable = has_new_neighbor(meta);
                            let plan = wm_expansion_plan(meta);
                            let (primary_cfg, sample_cfg) = if expandable {
                                (
                                    SemanticQueryConfig {
                                        topic: None,
                                        dimensions: plan.dims.clone(),
                                        measures: vec![],
                                        time_dimensions: vec![],
                                        filters,
                                        orders: vec![],
                                        limit: None,
                                        offset: None,
                                    },
                                    None,
                                )
                            } else {
                                (
                                    SemanticQueryConfig {
                                        topic: None,
                                        dimensions: vec![],
                                        measures: vec![format!(
                                            "{}.__oxy_row_count",
                                            meta.view_name
                                        )],
                                        time_dimensions: vec![],
                                        filters: filters.clone(),
                                        orders: vec![],
                                        limit: None,
                                        offset: None,
                                    },
                                    (!meta.sample_dims.is_empty()).then(|| SemanticQueryConfig {
                                        topic: None,
                                        dimensions: meta.sample_dims.clone(),
                                        measures: vec![],
                                        time_dimensions: vec![],
                                        filters,
                                        // Order ascending on the first sample dim so preview
                                        // chips are stable across reloads and align with the
                                        // ascending Sample Browser (`get_world_model_filter_instances`).
                                        orders: vec![SemanticOrder {
                                            field: meta.sample_dims[0].clone(),
                                            direction: "asc".to_string(),
                                        }],
                                        limit: Some(3),
                                        offset: None,
                                    }),
                                )
                            };
                            NodeWork {
                                meta,
                                expandable,
                                plan,
                                primary_cfg,
                                sample_cfg,
                            }
                        })
                        .collect();

                    if node_works.is_empty() {
                        break;
                    }

                    // Compile the whole level in one batch (primary first, then the
                    // optional leaf sample), under a single engine lock.
                    let all_cfgs: Vec<SemanticQueryConfig> = node_works
                        .iter()
                        .flat_map(|w| {
                            let mut v = vec![w.primary_cfg.clone()];
                            if let Some(ref s) = w.sample_cfg {
                                v.push(s.clone());
                            }
                            v
                        })
                        .collect();

                    let t_level_compile = std::time::Instant::now();
                    let all_sqls: Vec<Option<String>> = batch_compile(all_cfgs)
                        .await
                        .ok()
                        .and_then(|r| r.ok())
                        .unwrap_or_else(|| {
                            vec![
                                None;
                                node_works
                                    .iter()
                                    .map(|w| 1 + w.sample_cfg.is_some() as usize)
                                    .sum()
                            ]
                        });
                    tracing::info!(
                        depth,
                        elapsed_ms = t_level_compile.elapsed().as_millis(),
                        "filter-counts BFS: compiled level queries"
                    );

                    let t_level_exec = std::time::Instant::now();
                    let mut sql_iter = all_sqls.into_iter();
                    struct NodeResult {
                        entity_name: String,
                        matched: u64,
                        /// Present for expandable nodes that matched → next hop.
                        neighbor: Option<NeighborData>,
                        sample: Vec<String>,
                        sample_keys: Vec<String>,
                    }
                    let exec_futures: Vec<_> = node_works
                        .into_iter()
                        .map(|w| {
                            // Pull SQL in push order: primary, then leaf sample (if any).
                            let primary_sql = sql_iter.next().flatten();
                            let sample_sql = w
                                .sample_cfg
                                .as_ref()
                                .and_then(|_| sql_iter.next().flatten());
                            let datasource = w.meta.datasource.clone();
                            let entity_name = w.meta.entity_name.clone();
                            let pk_count = w.meta.pk_count;
                            let has_label_dim = w.meta.has_label_dim;
                            let expandable = w.expandable;
                            let plan = w.plan;
                            let wm = wm_b.clone();
                            let role_c = role_b.clone();
                            let run_exp = &run_expansion;
                            async move {
                                if expandable {
                                    // One query does count + PK rows + FK values + sample.
                                    let res = run_exp(datasource, primary_sql, plan).await;
                                    let neighbor = (res.matched > 0).then(|| NeighborData {
                                        pk_rows: res.pk_rows,
                                        fk_values: res.fk_values.into_iter().collect(),
                                    });
                                    NodeResult {
                                        entity_name,
                                        matched: res.matched,
                                        neighbor,
                                        sample: res.sample,
                                        sample_keys: res.sample_keys,
                                    }
                                } else {
                                    // Terminal node: scalar count + limited sample,
                                    // fired concurrently on one connector.
                                    let connector =
                                        build_connector(&wm, user_id, role_c, &datasource)
                                            .await
                                            .ok();
                                    let (matched, (sample, sample_keys)) = match connector.as_ref()
                                    {
                                        Some(c) => tokio::join!(
                                            async {
                                                match primary_sql {
                                                    Some(sql) => run_with_connector(c, &sql)
                                                        .await
                                                        .into_iter()
                                                        .next()
                                                        .and_then(|r| r.into_iter().next())
                                                        .and_then(|v: String| v.parse::<u64>().ok())
                                                        .unwrap_or(0),
                                                    None => 0,
                                                }
                                            },
                                            async {
                                                match sample_sql {
                                                    Some(sql) => run_with_connector(c, &sql)
                                                        .await
                                                        .into_iter()
                                                        .map(|r| {
                                                            sample_row_to_display_key(
                                                                &r,
                                                                pk_count,
                                                                has_label_dim,
                                                            )
                                                        })
                                                        .unzip(),
                                                    None => (vec![], vec![]),
                                                }
                                            },
                                        ),
                                        None => (0, (vec![], vec![])),
                                    };
                                    NodeResult {
                                        entity_name,
                                        matched,
                                        neighbor: None,
                                        sample,
                                        sample_keys,
                                    }
                                }
                            }
                        })
                        .collect();

                    let results = futures::future::join_all(exec_futures).await;
                    tracing::info!(
                        depth,
                        elapsed_ms = t_level_exec.elapsed().as_millis(),
                        n = results.len(),
                        "filter-counts BFS: executed level queries"
                    );
                    // Mark every discovered entity visited (count-once, cycle guard)
                    // and seed the next frontier from expandable nodes that matched.
                    let mut next_frontier: Vec<String> = Vec::new();
                    let mut client_gone = false;
                    for r in results {
                        visited.insert(r.entity_name.clone());
                        let sent = tx_b
                            .send(WmFilterCountEvent {
                                entity_name: r.entity_name.clone(),
                                matched: Some(r.matched),
                                total: None,
                                sample: r.sample,
                                sample_keys: r.sample_keys,
                                done: false,
                            })
                            .await;
                        if sent.is_err() {
                            client_gone = true;
                            break;
                        }
                        if let Some(nd) = r.neighbor {
                            next_frontier.push(r.entity_name.clone());
                            neighbor_data.insert(r.entity_name, nd);
                        }
                    }
                    if client_gone {
                        // Receiver dropped — don't schedule further BFS levels. The
                        // level that just finished already ran to completion (its
                        // queries were in flight before we could detect this), but
                        // this stops the compounding growth described in
                        // `WmExpansionResult` from continuing hop after hop once
                        // nobody is listening.
                        tracing::debug!(
                            depth,
                            "filter-counts BFS: receiver dropped, stopping early"
                        );
                        break;
                    }
                    frontier = next_frontier;
                }
                tracing::info!(
                    elapsed_ms = t_bfs.elapsed().as_millis(),
                    "filter-counts BFS: all levels done"
                );
            }
        );

        // Both tasks finished — send done sentinel then drop tx (closes the channel).
        tx.send(WmFilterCountEvent {
            entity_name: String::new(),
            total: None,
            matched: None,
            sample: vec![],
            sample_keys: vec![],
            done: true,
        })
        .await
        .ok();
    });

    Ok(Sse::new(create_sse_stream(rx)).keep_alive(KeepAlive::default()))
}

/// `GET /{workspace_id}/semantic/world-model/instance-detail`
///
/// Streams `WmInstanceDetailEvent` via SSE so the panel renders progressively:
/// `init` (attributes) appears first, then `parent`, then individual `child` events,
/// then `measures`, then `done`.
pub async fn get_world_model_instance_detail(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    layer_cache: SemanticLayerCacheCtx,
    engine_cache: SemanticEngineCacheCtx,
    Path(WorkspacePath { workspace_id: _ }): Path<WorkspacePath>,
    axum::extract::Query(q): axum::extract::Query<WmInstanceDetailQuery>,
) -> Result<
    Sse<impl futures::Stream<Item = Result<Event, axum::Error>>>,
    (StatusCode, extract::Json<ErrorResponse>),
> {
    let (layer, promotions, source) =
        load_entity_model(&workspace_manager, &layer_cache, &q.entity).await?;

    let view = primary_view_of(&layer, &q.entity).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            extract::Json(ErrorResponse {
                message: format!("Entity '{}' not found", q.entity),
            }),
        )
    })?;
    let pk_cols = entity_keys_in_view(view, &q.entity, true);
    let datasource = view.datasource.clone().unwrap_or_default();

    // Load .world-model.yml config once — used for display_field across primary, child,
    // and parent entities. A config that is absent or does not parse degrades the display
    // to the PK fallback; a read that FAILED answers the retryable 503 (it is an allowlist).
    let wm_cfg = resolve_world_model_config(&workspace_manager).await?;
    // Per-entity allowlist + labels for the PRIMARY entity, used to filter and relabel
    // the attribute and measure sections (mirrors apply_world_model_config for the graph).
    // `None` means no allowlist → show everything observed in the view (current behavior).
    let primary_entity_cfg = wm_cfg
        .as_ref()
        .and_then(|cfg| cfg.entities.iter().find(|e| e.id == q.entity));
    // Ordered (name, label) list for dimensions; preserves config ordering in the panel.
    let dim_allow: Option<Vec<(String, Option<String>)>> = primary_entity_cfg
        .and_then(|ec| ec.dimensions.as_ref())
        .map(|dims| {
            dims.iter()
                .map(|d| (d.name.clone(), d.label.clone()))
                .collect()
        });
    // name -> label map for measures (own + induced).
    let meas_allow: Option<std::collections::HashMap<String, Option<String>>> = primary_entity_cfg
        .and_then(|ec| ec.measures.as_ref())
        .map(|ms| {
            ms.iter()
                .map(|m| (m.name.clone(), m.label.clone()))
                .collect()
        });

    let get_display_field = move |entity_id: &str| -> Option<String> {
        wm_cfg
            .as_ref()
            .and_then(|cfg| cfg.entities.iter().find(|e| e.id == entity_id))
            .and_then(|ec| ec.display_field.clone())
    };

    // Build the primary connector once for this datasource.
    let connector = build_connector(&workspace_manager, user.id, role.clone(), &datasource)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                extract::Json(ErrorResponse {
                    message: e.to_string(),
                }),
            )
        })?;

    let databases = database_configs(&workspace_manager);

    // Shared PK filters used across all per-instance queries.
    //
    // `q.key` is either:
    //   • a plain string — the first PK column value, as returned by the instance picker
    //     (`WmInstanceItem::key = row.first()`).  We can only filter on that one column.
    //   • a JSON array of per-column values — what `WmChildSample::sample_keys` encodes
    //     for composite-PK children so all columns are available.
    //
    // Only expand to N filters when we have N values; a single value always maps to the
    // first PK column so the picker flow is unchanged.
    let key_values: Vec<String> =
        serde_json::from_str::<Vec<String>>(&q.key).unwrap_or_else(|_| vec![q.key.clone()]);
    let pk_filters: Vec<agentic_semantic::config::SemanticFilter> = if key_values.len() == 1 {
        vec![agentic_semantic::config::SemanticFilter {
            field: format!(
                "{}.{}",
                view.name,
                pk_cols.first().cloned().unwrap_or_default()
            ),
            filter_type: agentic_semantic::config::SemanticFilterType::Eq(
                agentic_semantic::config::ScalarFilter {
                    value: serde_json::Value::String(key_values[0].clone()),
                },
            ),
        }]
    } else {
        pk_cols
            .iter()
            .zip(key_values.iter())
            .map(|(col, val)| agentic_semantic::config::SemanticFilter {
                field: format!("{}.{}", view.name, col),
                filter_type: agentic_semantic::config::SemanticFilterType::Eq(
                    agentic_semantic::config::ScalarFilter {
                        value: serde_json::Value::String(val.clone()),
                    },
                ),
            })
            .collect()
    };

    // --- Collect all query configs upfront (pure in-memory, no blocking) ---
    let dim_names: Vec<String> = view.dimensions.iter().map(|d| d.name.clone()).collect();
    let display_field = get_display_field(&q.entity);
    let entity_display = EntityDisplaySpec::for_entity(view, &q.entity, display_field.as_deref());

    // 1. Attrs config
    let attrs_cfg = SemanticQueryConfig {
        topic: None,
        dimensions: dim_names
            .iter()
            .map(|n| format!("{}.{}", view.name, n))
            .collect(),
        measures: vec![],
        time_dimensions: vec![],
        filters: pk_filters.clone(),
        orders: vec![],
        limit: Some(1),
        offset: None,
    };

    // 3. Child configs — (label, sample_cfg, count_cfg)
    struct ChildCfg {
        label: String,
        sample: SemanticQueryConfig,
        count: SemanticQueryConfig,
        // pk_count: how many leading columns are PK values.
        // has_label_dim: whether a label column follows the PK columns.
        // Display = label col when present, else join pk cols with " · ".
        pk_count: usize,
        has_label_dim: bool,
    }
    // Inbound neighbours of the selected instance — every entity that references
    // it via a FK. This is the union of two link kinds, both queried identically
    // (filter the child's FK-to-q.entity by the seed key):
    //   • Parent-spine children (their `parent:` is q.entity) — measure promotions
    //     like `order_item → order`; shown first.
    //   • Cross-link children (they declare q.entity as a Foreign entity without
    //     naming it parent) — e.g. `order → retail_store` ("orders at this store"),
    //     the case the parent tree missed.
    let mut inbound_children: Vec<(String, LinkKind)> = Vec::new();
    for v in &layer.views {
        let Some(primary) = v
            .entities
            .iter()
            .find(|e| e.entity_type == EntityType::Primary)
        else {
            continue;
        };
        if primary.name == q.entity {
            continue;
        }
        let parent = promotions.parent_of(&primary.name);
        for link in build_entity_links(&layer.views, v, parent) {
            if link.target_entity == q.entity {
                inbound_children.push((primary.name.clone(), link.kind));
            }
        }
    }
    // Parent-spine promotions before cross-link references (stable within groups).
    inbound_children.sort_by_key(|(_, kind)| match kind {
        LinkKind::Parent => 0,
        LinkKind::CrossLink => 1,
    });

    let child_cfgs: Vec<ChildCfg> = inbound_children
        .iter()
        .filter_map(|(child_entity, _kind)| {
            let child_view = primary_view_of(&layer, child_entity)?;
            let child_pk = entity_keys_in_view(child_view, child_entity, true);
            let fk_in_child = entity_keys_in_view(child_view, &q.entity, false);
            if child_pk.is_empty() || fk_in_child.is_empty() {
                return None;
            }
            let fk_filter = agentic_semantic::config::SemanticFilter {
                field: format!("{}.{}", child_view.name, fk_in_child[0]),
                filter_type: agentic_semantic::config::SemanticFilterType::Eq(
                    agentic_semantic::config::ScalarFilter {
                        value: serde_json::Value::String(q.key.clone()),
                    },
                ),
            };
            let child_display_field = get_display_field(child_entity);
            let child_disp = EntityDisplaySpec::for_entity(
                child_view,
                child_entity,
                child_display_field.as_deref(),
            );
            let pk_count = child_disp.pk_count;
            let has_label_dim = child_disp.has_label_dim;
            Some(ChildCfg {
                label: format!("{child_entity} → {}", q.entity),
                sample: SemanticQueryConfig {
                    topic: None,
                    dimensions: child_disp.dims,
                    measures: vec![],
                    time_dimensions: vec![],
                    filters: vec![fk_filter.clone()],
                    orders: vec![],
                    limit: Some(5),
                    offset: None,
                },
                count: SemanticQueryConfig {
                    topic: None,
                    dimensions: vec![],
                    measures: vec![count_measure_ref(child_view)],
                    time_dimensions: vec![],
                    filters: vec![fk_filter],
                    orders: vec![],
                    limit: Some(1),
                    offset: None,
                },
                pk_count,
                has_label_dim,
            })
        })
        .collect();

    // 4. Own measures. `measure_meta` (view order) seeds the frontend skeleton
    //    rows via MeasureNames; the frontend fills them by name, so the value
    //    queries below can group measures however is convenient.
    #[derive(Clone)]
    struct MeasureMeta {
        name: String,
        measure_type: String,
        label: Option<String>,
    }
    let own_measures: Vec<_> = view
        .measures
        .as_ref()
        .map(|ms| {
            ms.iter()
                .filter(|m| !m.name.starts_with('_'))
                .filter(|m| {
                    meas_allow
                        .as_ref()
                        .is_none_or(|a| a.contains_key(m.name.as_str()))
                })
                .collect()
        })
        .unwrap_or_default();
    let make_meta = |m: &oxy_airlayer_compat::Measure| MeasureMeta {
        name: m.name.clone(),
        measure_type: format!("{:?}", m.measure_type).to_lowercase(),
        label: meas_allow
            .as_ref()
            .and_then(|a| a.get(m.name.as_str()).cloned().flatten()),
    };
    let measure_meta: Vec<MeasureMeta> = own_measures.iter().map(|m| make_meta(m)).collect();

    // Value queries. A `custom` measure is a cross-view composite (its expr rolls
    // up measures from other views); bundling several into one SELECT drags each
    // one's independent one-to-many join into a shared CTE, tripping airlayer's
    // fan-out / additive-vs-non-additive guard and failing the *whole* batch. So
    // each composite gets its own query (airlayer isolates a single composite's
    // terms into per-view CTEs correctly), while plain single-view measures stay
    // batched into one round-trip.
    struct OwnGroup {
        measures: Vec<MeasureMeta>,
        cfg: SemanticQueryConfig,
    }
    let own_cfg = |measures: &[&oxy_airlayer_compat::Measure]| SemanticQueryConfig {
        topic: None,
        dimensions: vec![],
        measures: measures
            .iter()
            .map(|m| format!("{}.{}", view.name, m.name))
            .collect(),
        time_dimensions: vec![],
        filters: pk_filters.clone(),
        orders: vec![],
        limit: Some(1),
        offset: None,
    };
    let mut own_groups: Vec<OwnGroup> = Vec::new();
    let simple: Vec<&oxy_airlayer_compat::Measure> = own_measures
        .iter()
        .copied()
        .filter(|m| m.measure_type != MeasureType::Custom)
        .collect();
    if !simple.is_empty() {
        own_groups.push(OwnGroup {
            measures: simple.iter().map(|m| make_meta(m)).collect(),
            cfg: own_cfg(&simple),
        });
    }
    for m in own_measures
        .iter()
        .copied()
        .filter(|m| m.measure_type == MeasureType::Custom)
    {
        own_groups.push(OwnGroup {
            measures: vec![make_meta(m)],
            cfg: own_cfg(&[m]),
        });
    }

    // Induced measures — group by source_view so each source gets ONE value
    // query (all its measures as columns) + ONE count query.
    // Each induced group is ONE value query (its measures as columns) plus a
    // separate count query for the fiber size. A `custom` measure is a cross-view
    // composite; airlayer only isolates a single composite's per-view CTEs
    // correctly, so bundling one with any other measure — or with the count —
    // trips the fan-out guard and fails the whole batch, exactly as the
    // own-measure path guards against above. So each custom induced measure gets
    // its own value query, the plain measures batch into one, and the count is
    // always a standalone query, never bundled with a composite.
    struct InducedGroup {
        // (name, measure_type, label) — value columns only; count is separate.
        measures: Vec<(String, String, Option<String>)>,
        value_cfg: SemanticQueryConfig,
        count_cfg: SemanticQueryConfig,
    }
    let mut induced_by_source: std::collections::HashMap<
        String,
        Vec<(String, String, Option<String>)>,
    > = std::collections::HashMap::new();
    for im in promotions
        .induced_for_view(&view.name)
        .into_iter()
        .filter(|im| !im.source_measure.starts_with('_'))
        .filter(|im| {
            meas_allow
                .as_ref()
                .is_none_or(|a| a.contains_key(im.source_measure.as_str()))
        })
    {
        if let Some(source_view) = layer.views.iter().find(|v| v.name == im.source_view)
            && let Some(sm) = source_view
                .measures
                .as_ref()
                .and_then(|ms| ms.iter().find(|m| m.name == im.source_measure))
        {
            let label = meas_allow
                .as_ref()
                .and_then(|a| a.get(im.source_measure.as_str()).cloned().flatten());
            induced_by_source
                .entry(im.source_view.clone())
                .or_default()
                .push((
                    im.source_measure.clone(),
                    format!("{:?}", sm.measure_type).to_lowercase(),
                    label,
                ));
        }
    }
    let mk_induced_cfg = |refs: Vec<String>| SemanticQueryConfig {
        topic: None,
        dimensions: vec![],
        measures: refs,
        time_dimensions: vec![],
        filters: pk_filters.clone(),
        orders: vec![],
        limit: Some(1),
        offset: None,
    };
    let mut induced_groups: Vec<InducedGroup> = Vec::new();
    for (source_view_name, measures) in induced_by_source {
        let Some(sv) = layer.views.iter().find(|v| v.name == source_view_name) else {
            continue;
        };
        // One count query per source — its fiber is shared by all the source's
        // measures, and keeping it standalone avoids bundling it with a composite.
        let count_cfg = mk_induced_cfg(vec![count_measure_ref(sv)]);
        // Composites each go in their own value query; plain measures batch into one.
        let (customs, simple): (Vec<_>, Vec<_>) = measures
            .into_iter()
            .partition(|(_, ty, _)| ty.as_str() == "custom");
        if !simple.is_empty() {
            let refs = simple
                .iter()
                .map(|(n, _, _)| format!("{}.{}", source_view_name, n))
                .collect();
            induced_groups.push(InducedGroup {
                value_cfg: mk_induced_cfg(refs),
                count_cfg: count_cfg.clone(),
                measures: simple,
            });
        }
        for c in customs {
            let refs = vec![format!("{}.{}", source_view_name, c.0)];
            induced_groups.push(InducedGroup {
                value_cfg: mk_induced_cfg(refs),
                count_cfg: count_cfg.clone(),
                measures: vec![c],
            });
        }
    }

    let (child_sample_cfgs, child_count_cfgs): (Vec<_>, Vec<_>) = child_cfgs
        .iter()
        .map(|cc| (cc.sample.clone(), cc.count.clone()))
        .unzip();
    let induced_value_cfgs: Vec<SemanticQueryConfig> =
        induced_groups.iter().map(|g| g.value_cfg.clone()).collect();
    let induced_count_cfgs: Vec<SemanticQueryConfig> =
        induced_groups.iter().map(|g| g.count_cfg.clone()).collect();
    let own_cfgs: Vec<SemanticQueryConfig> = own_groups.iter().map(|g| g.cfg.clone()).collect();

    // --- Phase 1: compile ALL SQL configs (except parent which needs FK from attrs) ---
    type SqlOpt = Option<String>;
    // One engine for the whole handler — this phase and the parent lookup in
    // the spawned task below share it, so the join graph is built once per
    // workspace revision instead of twice per request.
    let cached_engine = engine_cache
        .get_or_build(
            // Keyed by what the scan read: the compiled revision, or the
            // working copy on a node that owns one.
            source.engine_key(&engine_cache, &databases),
            layer.clone(),
            databases.clone(),
        )
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "world-model: semantic engine unavailable");
            e
        })
        .ok();
    let engine_for_phase1 = cached_engine.clone();
    let phase1: Option<(
        SqlOpt,
        Vec<SqlOpt>,
        Vec<SqlOpt>,
        Vec<SqlOpt>,
        Vec<SqlOpt>,
        Vec<SqlOpt>,
    )> = tokio::task::spawn_blocking(move || {
        let engine = engine_for_phase1.ok_or_else(|| {
            agentic_semantic::SemanticError::Runtime(
                "semantic engine unavailable for this workspace".to_string(),
            )
        })?;
        let c = |cfg: &SemanticQueryConfig| {
            let result = agentic_semantic::compile_with_engine(&engine, cfg);
            if let Err(ref e) = result {
                tracing::warn!(error = %e, "instance-detail SQL compilation failed");
            }
            result.ok()
        };
        Ok::<_, agentic_semantic::SemanticError>((
            c(&attrs_cfg),
            child_sample_cfgs.iter().map(&c).collect(),
            child_count_cfgs.iter().map(&c).collect(),
            own_cfgs.iter().map(&c).collect(),
            induced_value_cfgs.iter().map(&c).collect(),
            induced_count_cfgs.iter().map(c).collect(),
        ))
    })
    .await
    .ok()
    .and_then(|r| r.ok());
    let (
        attrs_sql,
        child_sample_sqls,
        child_count_sqls,
        own_group_sqls,
        induced_value_sqls,
        induced_count_sqls,
    ) = phase1.unwrap_or_default();

    // --- Stream results: three concurrent tasks via tokio::join! ---
    //
    // Task A: attrs → emit Init → Phase 2 parent compile → exec parent → emit Parent
    // Task B: FuturesUnordered over children (each: join!(sample, count)) → emit Child
    // Task C: join_all(own_batch + induced) → emit Measures
    let (tx, rx) = tokio::sync::mpsc::channel::<WmInstanceDetailEvent>(128);
    let tx_a = tx.clone();
    let tx_b = tx.clone();
    let tx_c = tx.clone();

    let connector_a = connector.clone();
    let connector_b = connector.clone();
    let connector_c = connector;

    // Task A's phase-2 parent compile reuses the handler's engine.
    let engine_for_spawn = cached_engine.clone();

    tokio::spawn(async move {
        tokio::join!(
            // ── Task A: attrs row → Init event → Phase 2 parent → Parent event ──
            async move {
                let attr_rows = match attrs_sql {
                    Some(ref sql) => run_with_connector(&connector_a, sql).await,
                    None => vec![],
                };
                let attr_row = attr_rows.into_iter().next().unwrap_or_default();

                let attr_values: Vec<(String, String)> = dim_names
                    .iter()
                    .enumerate()
                    .map(|(i, name)| (name.clone(), attr_row.get(i).cloned().unwrap_or_default()))
                    .collect();

                let display = {
                    let d = entity_display.display_from_attrs(&attr_values);
                    if d.is_empty() { q.key.clone() } else { d }
                };

                // Filter + relabel attributes per the .world-model.yml allowlist when present;
                // otherwise emit every observed dimension. The query above always selects all
                // dimensions so parent-FK resolution below still works regardless of the filter.
                let attributes: Vec<WmAttrValue> = match &dim_allow {
                    Some(allow) => {
                        let value_map: std::collections::HashMap<&str, &str> = attr_values
                            .iter()
                            .map(|(n, v)| (n.as_str(), v.as_str()))
                            .collect();
                        allow
                            .iter()
                            .filter_map(|(name, label)| {
                                value_map.get(name.as_str()).map(|v| WmAttrValue {
                                    name: name.clone(),
                                    value: v.to_string(),
                                    label: label.clone(),
                                })
                            })
                            .collect()
                    }
                    None => attr_values
                        .into_iter()
                        .map(|(name, value)| WmAttrValue {
                            name,
                            value,
                            label: None,
                        })
                        .collect(),
                };

                tx_a.send(WmInstanceDetailEvent::Init {
                    entity_id: q.entity.clone(),
                    key_value: q.key.clone(),
                    display,
                    attributes,
                })
                .await
                .ok();

                // Phase 2: compile + exec parent lookup (FK value now known from attr_row).
                let mut promotes_to: Vec<WmParentRef> = vec![];
                'parent: {
                    let Some(parent_entity) = promotions.parent_of(&q.entity) else {
                        break 'parent;
                    };
                    let Some(parent_view) = primary_view_of(&layer, parent_entity) else {
                        break 'parent;
                    };
                    let parent_pk = entity_keys_in_view(parent_view, parent_entity, true);
                    let Some(this_view) = primary_view_of(&layer, &q.entity) else {
                        break 'parent;
                    };
                    let fk_cols = entity_keys_in_view(this_view, parent_entity, false);
                    let fk_value = fk_cols
                        .first()
                        .and_then(|fk| {
                            dim_names
                                .iter()
                                .position(|n| n == fk)
                                .and_then(|i| attr_row.get(i).cloned())
                        })
                        .unwrap_or_else(|| q.key.clone());
                    let parent_display_field = get_display_field(parent_entity);
                    let parent_disp = EntityDisplaySpec::for_entity(
                        parent_view,
                        parent_entity,
                        parent_display_field.as_deref(),
                    );
                    let parent_cfg = SemanticQueryConfig {
                        topic: None,
                        dimensions: parent_disp.dims.clone(),
                        measures: vec![],
                        time_dimensions: vec![],
                        filters: vec![agentic_semantic::config::SemanticFilter {
                            field: format!(
                                "{}.{}",
                                parent_view.name,
                                parent_pk.first().cloned().unwrap_or_default()
                            ),
                            filter_type: agentic_semantic::config::SemanticFilterType::Eq(
                                agentic_semantic::config::ScalarFilter {
                                    value: serde_json::Value::String(fk_value.clone()),
                                },
                            ),
                        }],
                        orders: vec![],
                        limit: Some(1),
                        offset: None,
                    };
                    // The handler's engine, not a second one: this compiles a
                    // single query, and building a whole join graph for it was
                    // the most expensive line in the parent lookup.
                    let engine_for_parent = engine_for_spawn.clone();
                    let parent_sql = tokio::task::spawn_blocking(move || {
                        let engine = engine_for_parent.ok_or_else(|| {
                            agentic_semantic::SemanticError::Runtime(
                                "semantic engine unavailable for this workspace".to_string(),
                            )
                        })?;
                        agentic_semantic::compile_with_engine(&engine, &parent_cfg)
                    })
                    .await
                    .ok()
                    .and_then(|r| r.ok());
                    let parent_rows = match parent_sql {
                        Some(ref sql) => run_with_connector(&connector_a, sql).await,
                        None => vec![],
                    };
                    let parent_display = parent_rows
                        .into_iter()
                        .next()
                        .map(|row| {
                            let d = parent_disp.display_from_row(&row);
                            if d.is_empty() { fk_value.clone() } else { d }
                        })
                        .unwrap_or_else(|| fk_value.clone());
                    promotes_to.push(WmParentRef {
                        promotion: format!("{} → {parent_entity}", q.entity),
                        key: fk_value,
                        display: parent_display,
                    });
                }
                tx_a.send(WmInstanceDetailEvent::Parent { promotes_to })
                    .await
                    .ok();
            },
            // ── Task B: children — FuturesUnordered, each child: join!(sample, count) ──
            async move {
                let mut futs: FuturesUnordered<_> = child_cfgs
                    .into_iter()
                    .zip(child_sample_sqls.into_iter().zip(child_count_sqls))
                    .map(|(cc, (sample_sql, count_sql))| {
                        let c = connector_b.clone();
                        async move {
                            let c2 = c.clone();
                            let (sample_rows, count_rows) = tokio::join!(
                                async move {
                                    match sample_sql {
                                        Some(ref sql) => run_with_connector(&c, sql).await,
                                        None => vec![],
                                    }
                                },
                                async move {
                                    match count_sql {
                                        Some(ref sql) => run_with_connector(&c2, sql).await,
                                        None => vec![],
                                    }
                                },
                            );
                            let fiber_count = count_rows
                                .into_iter()
                                .next()
                                .and_then(|r| r.into_iter().next())
                                .and_then(|v| v.parse::<u64>().ok())
                                .unwrap_or(0);
                            let (sample, sample_keys): (Vec<String>, Vec<String>) = sample_rows
                                .into_iter()
                                .map(|r| {
                                    sample_row_to_display_key(&r, cc.pk_count, cc.has_label_dim)
                                })
                                .unzip();
                            WmChildSample {
                                promotion: cc.label,
                                fiber_count,
                                sample,
                                sample_keys,
                            }
                        }
                    })
                    .collect();

                while let Some(child) = futs.next().await {
                    tx_b.send(WmInstanceDetailEvent::Child { child }).await.ok();
                }
            },
            // ── Task C: own measures + induced — FuturesUnordered for streaming ──
            // Emits MeasureNames immediately (schema-only, no DB), then one Measure event
            // per completed query group so the frontend can fill skeletons progressively.
            async move {
                // Phase C-0: emit all measure names/types derived from schema — no DB needed.
                let mut measure_names: Vec<WmMeasureName> = measure_meta
                    .iter()
                    .map(|m| WmMeasureName {
                        name: m.name.clone(),
                        measure_type: m.measure_type.clone(),
                        label: m.label.clone(),
                    })
                    .collect();
                for group in &induced_groups {
                    for (name, measure_type, label) in &group.measures {
                        measure_names.push(WmMeasureName {
                            name: name.clone(),
                            measure_type: measure_type.clone(),
                            label: label.clone(),
                        });
                    }
                }
                tx_c.send(WmInstanceDetailEvent::MeasureNames { measure_names })
                    .await
                    .ok();

                // Phase C-1: run all query groups concurrently; emit each as it finishes.
                // Tag distinguishes an own-measure group from an induced group so the
                // right column→measure mapping is applied on completion.
                enum GroupTag {
                    Own(usize),
                    Induced(usize),
                }
                type Rows = Vec<Vec<String>>;
                let mut futs: FuturesUnordered<
                    std::pin::Pin<Box<dyn std::future::Future<Output = (GroupTag, Rows)> + Send>>,
                > = FuturesUnordered::new();

                for (idx, sql_opt) in own_group_sqls.into_iter().enumerate() {
                    let c = connector_c.clone();
                    futs.push(Box::pin(async move {
                        let rows = match sql_opt {
                            Some(ref sql) => run_with_connector(&c, sql).await,
                            None => vec![],
                        };
                        (GroupTag::Own(idx), rows)
                    }));
                }
                for (idx, (val_sql, cnt_sql)) in induced_value_sqls
                    .into_iter()
                    .zip(induced_count_sqls)
                    .enumerate()
                {
                    let c = connector_c.clone();
                    let c2 = connector_c.clone();
                    futs.push(Box::pin(async move {
                        // Value query and its fiber-count query run concurrently; the
                        // count is appended as the trailing column so the induced
                        // consumer below reads it exactly like the old batched shape.
                        let (val_rows, cnt_rows) = tokio::join!(
                            async move {
                                match val_sql {
                                    Some(ref sql) => run_with_connector(&c, sql).await,
                                    None => vec![],
                                }
                            },
                            async move {
                                match cnt_sql {
                                    Some(ref sql) => run_with_connector(&c2, sql).await,
                                    None => vec![],
                                }
                            },
                        );
                        let mut row = val_rows.into_iter().next().unwrap_or_default();
                        // Only append the count when the value query produced a row;
                        // an empty row means the value query failed → all measures "—".
                        if !row.is_empty() {
                            let count = cnt_rows
                                .into_iter()
                                .next()
                                .and_then(|r| r.into_iter().next())
                                .unwrap_or_default();
                            row.push(count);
                        }
                        (GroupTag::Induced(idx), vec![row])
                    }));
                }

                while let Some((tag, rows)) = futs.next().await {
                    let computed_measures: Vec<WmComputedMeasure> = match tag {
                        GroupTag::Own(idx) => {
                            let group = &own_groups[idx];
                            let own_row = rows.into_iter().next().unwrap_or_default();
                            group
                                .measures
                                .iter()
                                .enumerate()
                                .map(|(i, meta)| WmComputedMeasure {
                                    name: meta.name.clone(),
                                    measure_type: meta.measure_type.clone(),
                                    value: own_row
                                        .get(i)
                                        .cloned()
                                        .unwrap_or_else(|| "—".to_string()),
                                    fiber_count: 1,
                                    label: meta.label.clone(),
                                })
                                .collect()
                        }
                        GroupTag::Induced(idx) => {
                            let group = &induced_groups[idx];
                            let row = rows.into_iter().next().unwrap_or_default();
                            let fiber_count =
                                row.last().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
                            group
                                .measures
                                .iter()
                                .enumerate()
                                .map(|(i, (name, measure_type, label))| WmComputedMeasure {
                                    name: name.clone(),
                                    measure_type: measure_type.clone(),
                                    value: row.get(i).cloned().unwrap_or_else(|| "—".to_string()),
                                    fiber_count,
                                    label: label.clone(),
                                })
                                .collect()
                        }
                    };
                    tx_c.send(WmInstanceDetailEvent::Measure { computed_measures })
                        .await
                        .ok();
                }
            },
        );

        tx.send(WmInstanceDetailEvent::Done).await.ok();
    });

    Ok(Sse::new(create_sse_stream(rx)).keep_alive(KeepAlive::default()))
}

/// `GET /{workspace_id}/semantic/world-model/measure-breakdown`
///
/// Streams the metric-tree subtree for `measure` at `entity`, valued at the
/// instance `key`: `init` (structure) → per-node `value` events → `done`.
pub async fn get_world_model_measure_breakdown(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    layer_cache: SemanticLayerCacheCtx,
    engine_cache: SemanticEngineCacheCtx,
    Path(WorkspacePath { workspace_id: _ }): Path<WorkspacePath>,
    axum::extract::Query(q): axum::extract::Query<WmMeasureBreakdownQuery>,
) -> Result<
    Sse<impl futures::Stream<Item = Result<Event, axum::Error>>>,
    (StatusCode, extract::Json<ErrorResponse>),
> {
    let source = ModelSource::resolve(&workspace_manager).await?;
    let layer = source.layer(&layer_cache).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            extract::Json(ErrorResponse {
                message: e.to_string(),
            }),
        )
    })?;
    refuse_breakdown_database(&workspace_manager, &layer, &q)?;
    let databases = database_configs(&workspace_manager);
    // Surfaced, not swallowed: this is the airlayer validation error for the
    // workspace's own semantic files, and it is what makes an empty driver-tree
    // panel diagnosable.
    let engine = Some(
        engine_cache
            .get_or_build(
                source.engine_key(&engine_cache, &databases),
                layer.clone(),
                databases.clone(),
            )
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    extract::Json(ErrorResponse {
                        message: e.to_string(),
                    }),
                )
            })?,
    );
    let warehouse = WorkspaceWarehouse {
        workspace_manager: workspace_manager.clone(),
        user_id: user.id,
        role,
    };
    let rx = measure_breakdown_core(&workspace_manager, &warehouse, &layer, engine, q)
        .await
        .map_err(|(s, message)| (s, extract::Json(ErrorResponse { message })))?;
    Ok(Sse::new(create_sse_stream(rx)).keep_alive(KeepAlive::default()))
}
