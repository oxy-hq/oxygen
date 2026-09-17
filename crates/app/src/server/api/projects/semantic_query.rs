//! `POST /api/projects/{project_id}/semantic-query` — semantic-model
//! proxy for custom-app bundles.
//!
//! Bundle authors stop hand-rolling raw SQL against view-defined
//! measures. Instead they reference the topic + dimensions + measures
//! and let airlayer compile to dialect-specific SQL. When the data
//! team refactors the SQL behind a measure, the bundle picks up the
//! change without an edit.
//!
//! Pipeline:
//!   1. Shared custom-app gates (auth → origin → workspace → org).
//!   2. Versioned body parse (`v: 1` honored; absent = v1 backcompat).
//!   3. Airlayer compile via `agentic_semantic::resolve_and_compile` —
//!      same code path the IDE's `/semantic/compile` uses, so bundles
//!      stay in lockstep with what the rest of oxy renders.
//!   4. Execute through the same connector layer as `/query` (row cap,
//!      typed-stream conversion).
//!
//! Body shape matches `agentic_semantic::SemanticQueryConfig`:
//! `{ topic, dimensions[], measures[], filters[], time_dimensions[],
//!    orders[], limit?, offset? }`. We deliberately reuse the existing
//! type rather than declare a parallel one — keeping the bundle's
//! semantic-query shape identical to the IDE's prevents the surface
//! from drifting.

use std::sync::Arc;

use agentic_connector::{ConnectorError, DatabaseConnector};
use agentic_semantic::compile::CompiledQuery;
use agentic_semantic::config::SemanticQueryConfig;
use axum::Json;
use axum::extract::{Path, Query as AxumQuery, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use oxy_shared::errors::OxyError;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tracing::{error, instrument, warn};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::server::api::custom_apps_gates::{check_custom_app_gates, parse_versioned_body};
use crate::server::api::projects::query::{QueryResponse, json_objects_to_table};
use crate::server::api::typed_stream::typed_stream_to_json_objects;
use crate::server::router::AppState;

const MAX_ROWS: usize = 10_000;

#[derive(Serialize)]
struct ApiErr {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (
        status,
        Json(ApiErr {
            message: msg.into(),
            code: None,
        }),
    )
        .into_response()
}

fn err_with_code(status: StatusCode, msg: impl Into<String>, code: &'static str) -> Response {
    (
        status,
        Json(ApiErr {
            message: msg.into(),
            code: Some(code),
        }),
    )
        .into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct DebugQuery {
    /// When `1`, response includes the compiled SQL string. Off by
    /// default so production responses don't leak warehouse SQL
    /// shape to the browser (compile output may include column
    /// expressions that an operator hasn't seen). Bundle authors
    /// flip this on while debugging. When a rollup answered, the SQL
    /// is the rollup's re-aggregation. Either way its
    /// `read_parquet(...)` arguments are redacted — see
    /// `redact_parquet_sources`; that also applies to the warehouse
    /// path, whose compiled SQL can legitimately contain one when the
    /// view is backed by DuckDB or airhouse.
    #[serde(default)]
    debug: Option<u8>,
}

/// Customer-app `/semantic-query` response. Extends `QueryResponse`
/// with an optional `sql` field that's only populated when
/// `?debug=1` is passed.
/// Named explicitly for the same reason `data.rs`'s enum is: two types share
/// this Rust ident, and a component name is derived from the ident.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = SemanticQueryResult)]
pub struct SemanticQueryResponse {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<JsonValue>>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
}

/// Query the semantic layer: measures and dimensions rather than SQL.
///
/// The body is `SemanticQueryConfig` from `agentic-semantic`, whose filter
/// operators are a tagged enum and are the part nobody guesses right. They are
/// spelled out in the example below and are, in full: `eq`, `neq`, `gt`, `gte`,
/// `lt`, `lte`, `in`, `not_in`, `in_date_range`, `not_in_date_range`,
/// `contains`, `not_contains`, `starts_with`, `ends_with`, `set`, `not_set`.
///
/// **Documented by EXAMPLE, not by schema, and deliberately.** The body type
/// lives in `crates/agentic/semantic` — a domain crate — and deriving
/// `ToSchema` across it would put utoipa, a transport concern, into the
/// domain layer and then into everything that depends on it, for eight nested
/// types. An example is also the more useful artefact for the caller this
/// endpoint is documented for: it is copy-and-edit rather than assemble-from-
/// grammar. The cost is that the example can drift from the type, which a
/// schema could not — if you change `SemanticQueryConfig`, change this too.
///
/// `?debug=1` adds the compiled `sql` to the response. Off by default so a
/// production response does not leak warehouse SQL shape.
#[utoipa::path(
    method(post),
    path = "/projects/{project_id}/semantic-query",
    params(
        ("project_id" = Uuid, Path, description = "Project UUID"),
        ("debug" = Option<u8>, Query, description = "`1` adds the compiled SQL to the response")
    ),
    request_body(
        content = Object,
        description = "SemanticQueryConfig — `topic` plus any of `measures`, `dimensions`, `time_dimensions`, `filters`, `orders`, `limit`, `offset`",
        example = json!({
            "topic": "orders",
            "measures": ["orders.count", "orders.revenue"],
            "dimensions": ["orders.status"],
            "time_dimensions": [{"dimension": "orders.created_at", "granularity": "day"}],
            "filters": [{"field": "orders.status", "op": "in", "values": ["paid", "shipped"]}],
            "orders": [{"field": "orders.revenue", "direction": "desc"}],
            "limit": 100
        })
    ),
    responses(
        (status = OK, description = "Columnar result set; `sql` present only with `?debug=1`", body = SemanticQueryResponse, content_type = "application/json"),
        (status = BAD_REQUEST, description = "Body failed validation, or the topic/measure does not exist in the semantic layer"),
        (status = FORBIDDEN, description = "Origin check or org membership failed")
    ),
    security(
        ("BearerAuth" = [])
    )
)]
#[instrument(skip_all, fields(project_id = %project_id))]
pub async fn run_semantic_query(
    Path(project_id): Path<Uuid>,
    AxumQuery(debug): AxumQuery<DebugQuery>,
    State(app_state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // 1. Shared gates.
    let ctx = match check_custom_app_gates(&headers, project_id).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };

    // 2. Parse versioned body.
    let mut req: SemanticQueryConfig = match parse_versioned_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    // 2b. `scope: "reach"` — oxy's, not the query's. Read off the raw body
    //     (the config ignores keys it does not know), decided per viewer
    //     below, and folded into the cache key so two viewers with different
    //     reach never share an entry.
    let scope = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("scope").and_then(|s| s.as_str().map(str::to_string)));
    let viewer_reach = match scope.as_deref() {
        None | Some("") => None,
        Some("reach") => {
            let app = serde_json::from_slice::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v.get("app")
                        .and_then(|a| a.as_str().and_then(|a| a.parse().ok()))
                });
            Some(
                crate::server::api::operating_graph::reach::reach_for_viewer(
                    &ctx.db, ctx.org_id, &ctx.user, project_id, app,
                )
                .await,
            )
        }
        Some(other) => {
            return err_with_code(
                StatusCode::BAD_REQUEST,
                &format!("unknown scope `{other}`; the one scope is `reach`"),
                "semantic_scope_unknown",
            );
        }
    };

    // 3. Body validation. The compile step would also reject these
    //    but we want sharper error codes the SDK can pattern-match.
    if req.topic.as_deref().unwrap_or("").trim().is_empty() {
        return err_with_code(
            StatusCode::BAD_REQUEST,
            "`topic` is required",
            "semantic_topic_missing",
        );
    }
    if req.dimensions.is_empty() && req.measures.is_empty() && req.time_dimensions.is_empty() {
        return err_with_code(
            StatusCode::BAD_REQUEST,
            "at least one of dimensions, measures, or time_dimensions must be non-empty",
            "semantic_selection_empty",
        );
    }

    // 3b. Result cache read-through. Key on the raw request body so any
    //     change in topic/dimensions/measures/filters is a cache miss.
    //     Gates and body validation must run first (above), so malformed
    //     bodies still 400 and unauthenticated callers still 401/403.
    //     `?refresh` bypasses the cache to force a warehouse round-trip.
    let cache_sql = match &viewer_reach {
        None => String::from_utf8_lossy(&body).into_owned(),
        Some(reach) => format!(
            "{}\n#reach:{}:{}",
            String::from_utf8_lossy(&body),
            reach.everywhere,
            reach
                .locations
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    // `?debug=1` populates the compiled `sql` in the response body (see below), so a
    // debug response must never share a cache entry with a plain one — otherwise a
    // plain caller could read a cached debug body (leaking the compiled warehouse
    // SQL, which DebugQuery deliberately withholds), or a debug caller could read a
    // plain body missing its `sql`. Namespace by the flag so the two never collide.
    let include_sql = matches!(debug.debug, Some(1));
    let cache_ns = if include_sql {
        "semantic-debug"
    } else {
        "semantic"
    };
    let refresh = uri
        .query()
        .map(|q| {
            q.split('&')
                .any(|kv| kv == "refresh" || kv.starts_with("refresh="))
        })
        .unwrap_or(false);
    if !refresh && let Some(cached) = super::result_cache::get(project_id, cache_ns, "", &cache_sql)
    {
        return (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            (*cached).clone(),
        )
            .into_response();
    }

    // 4. Build workspace context — needed for the semantic scan path
    //    and the database connector.
    let proj_ctx = match ctx.build_project_context().await {
        Ok(ctx) => ctx,
        Err(resp) => return resp,
    };

    // 5. Compile via airlayer. Off-thread because compile is
    //    blocking-CPU work (parses every .view.yml / .topic.yml under
    //    the workspace scan path); same pattern the IDE handler uses.
    //
    // When the compile boundary is enabled, scan the promoted revision's
    // semantic_views / semantic_topics rows instead of the workspace dir. They
    // are materialised into a tempdir once per revision per process and cached
    // (`oxy::config::scan`); this handle keeps that tempdir alive until the
    // request is done.
    let materialised = match crate::server::api::semantic_scan::scan_dir(
        &proj_ctx.workspace_manager().config_manager,
    )
    .await
    {
        Ok(scan) => Some(scan),
        Err(e) => {
            tracing::warn!(
                project_id = %project_id,
                error = %e,
                "semantic scan: no scan directory available"
            );
            None
        }
    };
    // Stateless-fleet guard: on a serve replica there is no working copy, so
    // the FS fallback below (`semantics_scan_path()`) points at a directory
    // that doesn't exist — airlayer would compile against an empty dir and
    // return a misleading empty/500 result. Refuse the FS scan and return the
    // SAME NeedsRecompile contract the `workspace_context` middleware
    // established: a 503 with the `X-Oxy-Needs-Recompile` header (the FE's
    // retry signal) AND a deduped lazy compile. This path is reachable when
    // the compiled CONFIG is valid (so the middleware didn't short-circuit)
    // but the semantic materialisation is empty/failed, so the middleware's
    // own enqueue wouldn't have fired. (`materialise_semantic_scan` downgrades
    // real DB errors to `None`, so this also covers the transient-DB case — a
    // 503 retry is the right behavior there too.)
    // The predicate is the manager's, not `role == Serve`: a Worker is equally
    // diskless and fell straight through to a scan path that is not there.
    if materialised.is_none() && !proj_ctx.workspace_manager().config_manager.can_read_disk() {
        if let Ok(db) = oxy::database::client::establish_connection().await {
            crate::server::api::middlewares::workspace_context::enqueue_lazy_compile(
                &db, project_id,
            )
            .await;
        }
        let mut response = err_with_code(
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "workspace {project_id} has no compiled semantic model available on this \
                 stateless replica; a (re)compile has been enqueued — retry shortly"
            ),
            "semantic_needs_recompile",
        );
        if let Ok(val) = axum::http::HeaderValue::from_str(&project_id.to_string()) {
            response.headers_mut().insert("x-oxy-needs-recompile", val);
        }
        return response;
    }
    let scan_path = match materialised.as_ref() {
        Some(m) => m.path().to_path_buf(),
        None => proj_ctx
            .workspace_manager()
            .config_manager
            .semantics_scan_path(),
    };
    // The engine cache keys on the source that was actually read, so derive it
    // from the same match: `is_materialised()` distinguishes a compiled
    // revision from the working copy, which `revision_id()` cannot.
    let source_revision = match materialised.as_ref() {
        Some(m) if m.is_materialised() => proj_ctx.workspace_manager().config_manager.revision_id(),
        _ => None,
    };
    let databases: Vec<oxy_airlayer_compat::DatabaseConfig> = proj_ctx
        .workspace_manager()
        .config_manager
        .list_databases()
        .iter()
        // `dialect()`, not `database_type.to_string()`. The two agree for most
        // databases and diverge for exactly the ones that matter: `airhouse` and
        // `airhouse_managed` both speak DuckDB, and `motherduck` does too. Feed
        // airlayer the raw type name instead and `Dialect::from_str` returns
        // None for it, which is not inert — the datasource is dropped from the
        // dialect map and `resolve` silently falls back to the map default,
        // i.e. whichever database happens to be listed first in config.yml. A
        // workspace running airhouse alongside ClickHouse then gets ClickHouse
        // SQL compiled for its DuckDB views, and step 6 below dutifully routes
        // that SQL to the airhouse connector.
        .map(|db| oxy_airlayer_compat::database_config(db.name.clone(), db.dialect()))
        .collect();

    // 5b. Attach the rollup short-circuit.
    //
    // This route is `route_fleet` (`router/public.rs`), so any replica may
    // answer it, and the short-circuit is simply inert on one whose process
    // holds no preagg cache or rollup Parquet. Nothing here forces a rollup:
    // `preagg_context` yields `None` when the process has no cache (the
    // internal API router, or no workspace path), and `try_resolve_preagg`
    // yields `None` when no rollup covers the request or the manifest is
    // stale. Both fall through to the warehouse path below.
    //
    // The threshold is resolved from THIS workspace's own
    // `pre_aggregations.refresh_worker.renewal_threshold` when the process
    // carries no global value, matching what `workspace_context` does for the
    // IDE — the read side and the rebuild side must resolve one setting, or a
    // workspace configuring `10m` silently gets 120s on every query.
    let preagg_ctx = crate::server::api::middlewares::workspace_context::PreaggCacheCtx {
        cache: app_state.preagg_cache.clone(),
        renewal_threshold_secs: app_state.preagg_renewal_threshold_secs,
    };
    let preagg = crate::server::preagg_context::preagg_context(
        proj_ctx.workspace_manager().workspace_id,
        preagg_ctx.cache.clone(),
        Some(preagg_ctx.renewal_threshold_secs_or(&proj_ctx.workspace_manager().config_manager)),
        // A read surface: the route badges its answer **Pre-aggregated**, so a
        // rollup a cycle behind is a labelled number, not a wrong one.
        crate::server::preagg_context::RollupFreshness::ServeStale,
    );

    // 5a. Pin a scoped query to the viewer's reach before it compiles. The
    //     layer comes from the process cache the workspace handlers share —
    //     keyed on the source this request actually read — and is handed to
    //     the compile below, so a scoped request walks the model at most
    //     once, and only on a cache miss.
    let mut scoped_layer: Option<std::sync::Arc<oxy_airlayer_compat::SemanticLayer>> = None;
    if let Some(reach) = &viewer_reach {
        let layers = crate::server::api::middlewares::workspace_context::SemanticLayerCacheCtx {
            cache: app_state.semantic_layer_cache.clone(),
            workspace_id: proj_ctx.workspace_manager().workspace_id,
            engine_cache: app_state.semantic_engine_cache.clone(),
        };
        let layer = match layers.get_or_load(source_revision, scan_path.clone()).await {
            Ok(layer) => layer,
            Err(e) => {
                return err_with_code(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("semantic layer failed to load: {e}"),
                    "semantic_layer_load_failed",
                );
            }
        };
        if let Err(e) = crate::server::api::operating_graph::binding::apply_reach_scope(
            &ctx.db, ctx.org_id, &layer, reach, &mut req,
        )
        .await
        {
            let (status, code) = match e {
                crate::server::api::operating_graph::binding::ScopeError::NoBoundView => {
                    (StatusCode::BAD_REQUEST, "semantic_scope_unbound")
                }
                crate::server::api::operating_graph::binding::ScopeError::Db(_) => {
                    (StatusCode::INTERNAL_SERVER_ERROR, "semantic_scope_failed")
                }
            };
            return err_with_code(status, &e.to_string(), code);
        }
        scoped_layer = Some(layer);
    }
    let req_clone = req;
    // The custom-app data plane is the highest-QPS semantic surface there is,
    // and it was rebuilding the join graph on every request. Keyed on the
    // revision this request resolved, so a bundle reading a promoted revision
    // never shares an entry with an IDE reader on the working copy.
    let engine_cache = app_state.semantic_engine_cache.clone();
    // Keyed on the source the scan actually read. `revision_id()` reports the
    // revision the request is PINNED to and is `Some` even on a node reading
    // its own working copy, so keying by it would let this plane share an
    // engine with the IDE's working-copy readers.
    let engine_key = oxy_airlayer_compat::EngineKey::for_source(
        proj_ctx.workspace_manager().workspace_id,
        source_revision,
        &databases,
    );
    // The layer a scoped request already holds is handed to the compile only
    // when the engine cache has nothing under this key — that is the one case
    // the compile would otherwise walk the model again, and cloning a whole
    // layer for a hit that never reads it is the cost the cache exists to
    // avoid.
    let pre_loaded_layer = scoped_layer
        .filter(|_| engine_cache.lookup(&engine_key).is_none())
        .map(|layer| (*layer).clone());
    // Sentry hubs are per thread: carry the request's onto the blocking pool,
    // so a panic in the compile is captured under the custom-app surface tag
    // (`middlewares::sentry_surface`) and not on the pool thread's bare hub.
    let hub = sentry::Hub::current();
    let compiled = match tokio::task::spawn_blocking(move || {
        sentry::Hub::run(hub, || {
            agentic_semantic::compile::resolve_and_compile_cached(
                &engine_cache,
                engine_key,
                &scan_path,
                &databases,
                &req_clone,
                preagg.as_ref(),
                pre_loaded_layer,
            )
        })
    })
    .await
    {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            // Airlayer compile errors are caller-input problems —
            // unknown topic, dimension typo, malformed filter. Map
            // to 400 with the airlayer message in the detail so the
            // bundle author sees exactly what to fix.
            warn!(error = %e, "semantic-query compile failed");
            return err_with_code(
                StatusCode::BAD_REQUEST,
                e.to_string(),
                "semantic_compile_failed",
            );
        }
        Err(e) => {
            error!("semantic compile task panicked: {e}");
            return err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "semantic compile task panicked",
            );
        }
    };

    // 6. Read the rollup when one covered the request.
    //
    // A rollup that won't read is not a failed query: the same question has a
    // warehouse answer, and the `Preaggregation` variant carries the SQL for
    // it. Surfacing the DuckDB/HTTP error instead would turn a routine state —
    // a manifest listing a rollup whose object hasn't been mirrored to this
    // node yet — into a 500 on exactly the nodes the blob tier exists to
    // serve. Same posture as the IDE's `/semantic` route.
    let mut rollup: Option<(JsonValue, String)> = None;
    let (sql, database_name) = match compiled {
        CompiledQuery::Warehouse { sql, database_name } => (sql, database_name),
        CompiledQuery::Preaggregation {
            preagg_sql,
            source,
            warehouse_sql,
            warehouse_database,
        } => {
            // Push the row cap into the rollup SQL the same way the warehouse
            // branch does. Without it DuckDB drains every row of a
            // high-cardinality `group_by` and builds a `serde_json::Value` per
            // cell before `preagg_columns_and_rows` throws the excess away —
            // and this connection is `open_in_memory()` with no
            // `temp_directory`, so a wide result has nowhere to spill.
            // `MAX_ROWS + 1` so the overflow is still visible as `truncated`.
            let read_sql = wrap_with_limit(&preagg_sql, MAX_ROWS + 1);
            // `?debug=1` gets the SQL *without* the wrap, matching what the
            // warehouse branch returns. The wrapper is transport, and showing
            // it on one path only would be a second incidental tell for which
            // path answered.
            let executed_preagg_sql = preagg_sql.clone();
            let src = source.clone();
            // The request's hub, not the blocking pool's — as for the compile above.
            let hub = sentry::Hub::current();
            match tokio::task::spawn_blocking(move || {
                sentry::Hub::run(hub, || {
                    agentic_semantic::preagg::execute_preagg_sql(&read_sql, &src)
                })
            })
            .await
            {
                Ok(Ok(value)) => rollup = Some((value, executed_preagg_sql)),
                Ok(Err(e)) => warn!(
                    remote = source.is_remote(),
                    error = %e,
                    "preagg rollup read failed; answering from the warehouse instead"
                ),
                Err(e) => warn!(
                    error = %e,
                    "preagg task panicked; answering from the warehouse instead"
                ),
            }
            (warehouse_sql, warehouse_database)
        }
    };

    // 7. Execute, unless step 6 already answered. The warehouse branch keeps
    //    the outer-LIMIT wrap `/query` uses so a semantic query producing
    //    millions of rows still respects the row cap; the rollup branch caps
    //    in `preagg_columns_and_rows` for the same reason, so a bundle sees
    //    one row ceiling however the question was answered.
    let (raw_columns, raw_rows, truncated, executed_sql) = match rollup {
        Some((value, preagg_sql)) => {
            let (columns, rows, truncated) = preagg_columns_and_rows(value, MAX_ROWS);
            (columns, rows, truncated, preagg_sql)
        }
        None => {
            // Resolve connector for the database the topic compiled against.
            // airlayer's resolver may pick a different db than the project's
            // default if the topic's first view declares `datasource:` — honor
            // that decision. Only reached on the warehouse path: a rollup read
            // never needs a warehouse connection.
            let connector = match proj_ctx.build_connector_for(&database_name).await {
                Ok(c) => c,
                Err(OxyError::ConfigurationError(msg)) => {
                    return err(StatusCode::BAD_REQUEST, msg);
                }
                Err(e) => {
                    // Surface the underlying error to the bundle author —
                    // mirrors `query.rs`. Agentic-connector error strings are
                    // host/protocol diagnostics, no secret values.
                    error!("connector build failed for '{database_name}': {e}");
                    return err(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("failed to connect to database '{database_name}': {e}"),
                    );
                }
            };
            // `MAX_ROWS + 1` for the same reason the rollup branch uses it:
            // asking for one row past the cap makes `truncated` exact instead
            // of a `len() == MAX_ROWS` guess that fires on a result which
            // happens to be exactly MAX_ROWS rows. Both paths now answer the
            // question the same way.
            let limited_sql = wrap_with_limit(&sql, MAX_ROWS + 1);
            let response = match execute_compiled_sql(connector, &limited_sql).await {
                Ok(r) => r,
                Err(resp) => return resp,
            };
            (response.columns, response.rows, response.truncated, sql)
        }
    };

    // Airlayer emits column aliases as `view__member` (dot in the
    // member path → double underscore in the result column).
    // Bundle authors think in member names (`store_id`,
    // `total_store_sales`), not view-prefixed names. Strip the
    // `view__` prefix when doing so doesn't introduce a collision
    // with another column in the same row. Multi-view queries that
    // would collide keep the qualified form so the bundle never sees
    // ambiguous data.
    //
    // Applied to BOTH paths on purpose. `generate_reagg_sql` aliases its
    // projection the same `view__member` way the warehouse SQL does, and a
    // bundle must not see its row keys change depending on whether a rollup
    // happened to cover the request — that break would be silent and
    // intermittent.
    let (columns, rows) = strip_view_prefix(raw_columns, raw_rows);

    let semantic_response = SemanticQueryResponse {
        columns,
        rows,
        truncated,
        // Under `?debug=1` this is the compiled SQL — the rollup's
        // re-aggregation when one served the request, the warehouse SQL
        // otherwise. Both are pre-wrap: neither carries the outer
        // `LIMIT MAX_ROWS + 1` that actually executed, so the transport is
        // invisible on both paths rather than one. A caller comparing the two
        // is the only way to tell from outside that a rollup answered.
        sql: if include_sql {
            Some(redact_parquet_sources(&executed_sql))
        } else {
            None
        },
    };

    let bytes = match serde_json::to_vec(&semantic_response) {
        Ok(b) => b,
        Err(e) => {
            error!("serialize semantic response: {e}");
            return Json(semantic_response).into_response();
        }
    };
    let arc = std::sync::Arc::new(bytes);
    super::result_cache::put(project_id, cache_ns, "", &cache_sql, arc.clone());
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        (*arc).clone(),
    )
        .into_response()
}

/// Reshape `execute_preagg_sql`'s result into the bundle's row-major form.
///
/// DuckDB hands back `{ "columns": [...], "rows": [ { col: value, ... } ] }`;
/// this route answers with `columns` plus positional `Vec<Vec<JsonValue>>`.
/// Values keep their JSON types — a number stays a number — unlike the IDE's
/// stringified table, because a bundle reads these straight into TypeScript.
///
/// `max_rows` is applied here rather than left to the rollup's own size: the
/// warehouse path wraps its SQL in `LIMIT MAX_ROWS + 1` and drops the overflow
/// row, and a bundle that got 50k rows from a rollup and 10k from the warehouse
/// for the same question would be reading the transport, not the data. Both
/// paths fetch one row past the cap, so `truncated` is exact on both.
fn preagg_columns_and_rows(
    value: JsonValue,
    max_rows: usize,
) -> (Vec<String>, Vec<Vec<JsonValue>>, bool) {
    let columns: Vec<String> = value
        .get("columns")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();

    let all = value.get("rows").and_then(|r| r.as_array());
    let total = all.map(|a| a.len()).unwrap_or(0);
    let truncated = total > max_rows;

    let rows = all
        .map(|arr| {
            arr.iter()
                .take(max_rows)
                .map(|row| {
                    columns
                        .iter()
                        // A column the row object lacks is null, not a gap:
                        // the positional form has no way to say "absent", and
                        // a short row would desync every column after it.
                        .map(|col| row.get(col).cloned().unwrap_or(JsonValue::Null))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default();

    (columns, rows, truncated)
}

/// Rewrite columns that look like `view__member` to bare `member`,
/// unless renaming would collide with another column. Bundle author
/// gets `row.store_id` instead of `row.store_performance__store_id`
/// in the common single-view case; multi-view collisions keep the
/// qualified form. Rows are remapped in lockstep so column index
/// invariants stay intact.
fn strip_view_prefix(
    columns: Vec<String>,
    rows: Vec<Vec<JsonValue>>,
) -> (Vec<String>, Vec<Vec<JsonValue>>) {
    // First pass: count how many times each candidate bare name
    // would appear. A bare name with >1 contributor means at least
    // two columns would collide — keep both qualified.
    let mut bare_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for col in &columns {
        let bare = bare_member_name(col).unwrap_or(col.as_str());
        *bare_counts.entry(bare).or_insert(0) += 1;
    }

    let new_columns: Vec<String> = columns
        .iter()
        .map(|col| match bare_member_name(col) {
            Some(bare) if bare_counts.get(bare).copied().unwrap_or(0) == 1 => bare.to_string(),
            _ => col.clone(),
        })
        .collect();
    (new_columns, rows)
}

/// Parse an airlayer column alias of the form `view__member` and
/// return the `member` part. Returns `None` when the column doesn't
/// match the pattern (e.g. it was already bare, or contains multiple
/// `__` segments — which airlayer doesn't produce, but we're
/// defensive).
fn bare_member_name(col: &str) -> Option<&str> {
    let (_view, member) = col.split_once("__")?;
    // If the member half itself contains `__`, abort the strip —
    // the column shape doesn't match the expected `view__member`
    // layout and we'd be guessing.
    if member.contains("__") {
        return None;
    }
    Some(member)
}

/// Wrap compiled SQL in `SELECT * FROM (...) LIMIT N`. Same pattern
/// `query.rs` uses — keeps the row cap consistent across endpoints
/// without forcing airlayer to know about it.
///
/// `pub(crate)` because a bundle's `ctx.semantic` reads its rollup through the
/// identical wrap (`custom_apps_functions::host::read_rollup`), and "a function
/// sees one row ceiling however the question was answered" is only true while
/// the two are the same code.
pub(crate) fn wrap_with_limit(sql: &str, max_rows: usize) -> String {
    let sql_trimmed = sql.trim_end().trim_end_matches(';').trim_end();
    format!("SELECT * FROM (\n{sql_trimmed}\n) AS oxy_semantic_query LIMIT {max_rows}")
}

/// Replace the argument of every `read_parquet('...')` with a placeholder.
///
/// `?debug=1` exists to show the *shape* of the query that ran, not where the
/// bytes live: a rollup's SQL embeds an `s3://<bucket>/<key>` URI or an
/// absolute state-dir path, neither of which the bundle author needs and both
/// of which describe server-side storage layout. No credentials appear in the
/// string either way — this keeps the disclosure surface the same as the
/// warehouse path's.
fn redact_parquet_sources(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    // The call-site search itself is quote-unaware: a string literal whose
    // *contents* were `read_parquet(` would start a redaction. Caller-supplied
    // filter values DO reach this string — airlayer inlines them as literals,
    // there are no bind parameters — so a bundle can inject that text. It
    // can't hide anything, though: `generate_reagg_sql` puts the real
    // `read_parquet(` in the `FROM`, ahead of any `WHERE` literal, so the
    // genuine path is always consumed first and an injected call can only
    // mangle the caller's own `?debug=1` output. Left as is on that basis —
    // and the `None` arm below redacts rather than emitting the remainder, so
    // the argument does not have to assume airlayer keeps emitting exactly one
    // call the way it does today.
    while let Some(idx) = rest.find("read_parquet(") {
        let (head, tail) = rest.split_at(idx + "read_parquet(".len());
        out.push_str(head);
        // Balance parens rather than stopping at the first `)`: airlayer emits
        // a single quoted path today, but a list or glob helper
        // (`read_parquet(list_value('a','b'))`) would otherwise be cut mid-call
        // and leave unbalanced SQL in the response.
        let mut depth = 1usize;
        let mut in_quote = false;
        let mut end = None;
        for (i, c) in tail.char_indices() {
            // Parens inside a string literal are path characters, not
            // structure: `read_parquet('/data/a)b.parquet')` would otherwise
            // close at the quoted `)` and leak the rest of the path.
            match c {
                '\'' => in_quote = !in_quote,
                '(' if !in_quote => depth += 1,
                ')' if !in_quote => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(close) => {
                out.push_str("'<redacted>'");
                rest = &tail[close..];
            }
            // Unbalanced input — an injected filter literal can leave the
            // quote state stuck. Redact and stop: emitting the remainder
            // verbatim would disclose every later `read_parquet(` path to a
            // request that controls the injection. Debug SQL is best-effort by
            // then, so a truncation is the right trade.
            None => {
                out.push_str("'<redacted>'");
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Apply the row cap to a warehouse result and say whether anything was cut.
///
/// The caller wraps in `LIMIT MAX_ROWS + 1`, so an extra row is evidence, not
/// payload: drop it before answering. `connector_truncated` is ORed in because
/// the connector's byte/row backstop can stop *below* `MAX_ROWS` on wide rows,
/// which the length check alone would miss.
///
/// Split out from `execute_compiled_sql` so the warehouse half of this route's
/// row-cap parity is testable without a connector — the rollup half is tested
/// through `preagg_columns_and_rows`.
fn cap_and_flag(mut objects: Vec<JsonValue>, connector_truncated: bool) -> (Vec<JsonValue>, bool) {
    let truncated = objects.len() > MAX_ROWS || connector_truncated;
    objects.truncate(MAX_ROWS);
    (objects, truncated)
}

async fn execute_compiled_sql(
    connector: Arc<dyn DatabaseConnector>,
    sql: &str,
) -> Result<QueryResponse, Response> {
    let stream = match connector.execute_query_full(sql).await {
        Ok(s) => s,
        Err(ConnectorError::QueryFailed(detail)) => {
            warn!(detail = ?detail.message, "semantic-query: warehouse query failed");
            return Err(err(
                StatusCode::BAD_REQUEST,
                "semantic query failed; see server logs for details",
            ));
        }
        Err(ConnectorError::ConnectionError(msg)) => {
            error!(msg = ?msg, "semantic-query: warehouse connection failed");
            return Err(err(StatusCode::BAD_GATEWAY, "warehouse connection failed"));
        }
        Err(ConnectorError::Other(msg)) => {
            error!(msg = ?msg, "semantic-query: execution error");
            return Err(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "semantic query execution failed",
            ));
        }
    };

    let (objects, connector_truncated) = match typed_stream_to_json_objects(stream).await {
        Ok(rows) => rows,
        Err(e) => {
            error!("row conversion failed: {e}");
            return Err(err(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to convert query results",
            ));
        }
    };

    let (objects, truncated) = cap_and_flag(objects, connector_truncated);
    Ok(json_objects_to_table(objects, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_with_limit_appends_outer_limit() {
        let wrapped = wrap_with_limit("SELECT 1", 100);
        assert!(wrapped.starts_with("SELECT * FROM (\nSELECT 1\n)"));
        assert!(wrapped.ends_with("LIMIT 100"));
    }

    #[test]
    fn wrap_with_limit_strips_trailing_semicolon() {
        let wrapped = wrap_with_limit("SELECT 1;", 100);
        assert!(!wrapped.contains("SELECT 1;"));
    }

    #[test]
    fn strip_view_prefix_simplifies_single_view() {
        let cols = vec![
            "store_performance__store_id".to_string(),
            "store_performance__total_sales".to_string(),
        ];
        let rows = vec![vec![JsonValue::from(1), JsonValue::from(100.0)]];
        let (new_cols, new_rows) = strip_view_prefix(cols, rows);
        assert_eq!(new_cols, vec!["store_id", "total_sales"]);
        // Rows are unchanged in order; column positions match.
        assert_eq!(new_rows[0][0], JsonValue::from(1));
        assert_eq!(new_rows[0][1], JsonValue::from(100.0));
    }

    #[test]
    fn strip_view_prefix_keeps_collisions_qualified() {
        // Two views both contribute a `store_id` dimension → keep
        // both qualified so the bundle gets unambiguous data.
        let cols = vec![
            "sales__store_id".to_string(),
            "inventory__store_id".to_string(),
        ];
        let rows = vec![vec![JsonValue::from(1), JsonValue::from(2)]];
        let (new_cols, _) = strip_view_prefix(cols.clone(), rows);
        assert_eq!(new_cols, cols, "collisions must keep view prefix");
    }

    #[test]
    fn strip_view_prefix_leaves_already_bare_alone() {
        let cols = vec!["count".to_string(), "id".to_string()];
        let rows = vec![vec![JsonValue::from(1), JsonValue::from(2)]];
        let (new_cols, _) = strip_view_prefix(cols.clone(), rows);
        assert_eq!(new_cols, cols);
    }

    #[test]
    fn strip_view_prefix_mixed_strips_only_uncollided() {
        let cols = vec![
            "sales__amount".to_string(),   // bare `amount` available, no collision
            "sales__store_id".to_string(), // bare `store_id` would collide with next
            "inventory__store_id".to_string(),
        ];
        let rows = vec![vec![
            JsonValue::from(10),
            JsonValue::from(1),
            JsonValue::from(2),
        ]];
        let (new_cols, _) = strip_view_prefix(cols, rows);
        assert_eq!(
            new_cols,
            vec!["amount", "sales__store_id", "inventory__store_id"]
        );
    }

    #[test]
    fn bare_member_name_handles_unexpected_shapes() {
        assert_eq!(bare_member_name("view__member"), Some("member"));
        assert_eq!(bare_member_name("no_underscore"), None);
        // Defensive: airlayer doesn't emit triple-underscored names,
        // but if it ever does, the parser bails out rather than guess.
        assert_eq!(bare_member_name("view__weird__name"), None);
    }

    #[test]
    fn preagg_columns_and_rows_reshapes_to_positional() {
        let value = serde_json::json!({
            "columns": ["store_id", "total"],
            "rows": [
                { "store_id": 1, "total": 10.5 },
                { "store_id": 2, "total": 20.0 },
            ],
        });
        let (cols, rows, truncated) = preagg_columns_and_rows(value, 10);
        assert_eq!(cols, vec!["store_id", "total"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec![JsonValue::from(1), JsonValue::from(10.5)]);
        assert_eq!(rows[1], vec![JsonValue::from(2), JsonValue::from(20.0)]);
        assert!(!truncated);
    }

    #[test]
    fn preagg_columns_and_rows_fills_missing_keys_with_null() {
        // The desync guard: a row object lacking a column must yield an
        // explicit null, not a short row that shifts every later column.
        let value = serde_json::json!({
            "columns": ["a", "b", "c"],
            "rows": [ { "a": 1, "c": 3 } ],
        });
        let (_, rows, _) = preagg_columns_and_rows(value, 10);
        assert_eq!(
            rows[0],
            vec![JsonValue::from(1), JsonValue::Null, JsonValue::from(3)]
        );
    }

    #[test]
    fn preagg_columns_and_rows_truncates_at_the_boundary() {
        let mk = |n: usize| {
            serde_json::json!({
                "columns": ["a"],
                "rows": (0..n).map(|i| serde_json::json!({ "a": i })).collect::<Vec<_>>(),
            })
        };
        // Exactly at the cap: kept whole, not flagged.
        let (_, rows, truncated) = preagg_columns_and_rows(mk(3), 3);
        assert_eq!(rows.len(), 3);
        assert!(!truncated);
        // One over — which is what the `MAX_ROWS + 1` SQL limit produces.
        let (_, rows, truncated) = preagg_columns_and_rows(mk(4), 3);
        assert_eq!(rows.len(), 3);
        assert!(truncated);
    }

    #[test]
    fn preagg_columns_and_rows_tolerates_a_shapeless_value() {
        let (cols, rows, truncated) = preagg_columns_and_rows(serde_json::json!({}), 10);
        assert!(cols.is_empty());
        assert!(rows.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn preagg_sql_limit_uses_the_same_wrapper_as_the_warehouse_path() {
        // Parity: both branches cap in SQL, and the rollup asks for one extra
        // row so `preagg_columns_and_rows` can still see the overflow.
        let wrapped = wrap_with_limit("SELECT 1", MAX_ROWS + 1);
        assert!(wrapped.ends_with(&format!("LIMIT {}", MAX_ROWS + 1)));
    }

    #[test]
    fn redact_parquet_sources_hides_storage_locations() {
        let sql = "SELECT a FROM read_parquet('s3://bucket/key/part.parquet') GROUP BY a";
        let out = redact_parquet_sources(sql);
        assert!(!out.contains("s3://"), "{out}");
        assert_eq!(
            out, "SELECT a FROM read_parquet('<redacted>') GROUP BY a",
            "the query shape must survive redaction"
        );
    }

    #[test]
    fn cap_and_flag_matches_the_rollup_boundary() {
        let obj = |i: usize| JsonValue::from(i);
        // Exactly at the cap: kept whole, not flagged — the same boundary
        // `preagg_columns_and_rows_truncates_at_the_boundary` asserts.
        let (rows, truncated) = cap_and_flag((0..MAX_ROWS).map(obj).collect(), false);
        assert_eq!(rows.len(), MAX_ROWS);
        assert!(!truncated);
        // One over, which is what the `MAX_ROWS + 1` wrap produces: flagged,
        // and the evidence row is dropped rather than served.
        let (rows, truncated) = cap_and_flag((0..MAX_ROWS + 1).map(obj).collect(), false);
        assert_eq!(rows.len(), MAX_ROWS);
        assert!(truncated);
    }

    #[test]
    fn cap_and_flag_honours_the_connector_backstop() {
        // A wide-row byte truncation stops below MAX_ROWS; the length check
        // alone would call that a complete result.
        let (rows, truncated) = cap_and_flag(vec![JsonValue::from(1)], true);
        assert_eq!(rows.len(), 1);
        assert!(truncated);
    }

    #[test]
    fn redact_parquet_sources_ignores_parens_inside_the_path() {
        // A `)` in an S3 key or state-dir path is structure to a naive counter
        // and would leak the rest of the path.
        let out = redact_parquet_sources("SELECT 1 FROM read_parquet('/data/a)b.parquet')");
        assert_eq!(out, "SELECT 1 FROM read_parquet('<redacted>')");
        assert!(!out.contains("b.parquet"));
    }

    #[test]
    fn redact_parquet_sources_balances_nested_parens() {
        // Not a shape airlayer emits today, but a list/glob helper would
        // otherwise be cut mid-call and leave unbalanced SQL in the response.
        let out = redact_parquet_sources("SELECT 1 FROM read_parquet(list_value('a','b')) t");
        assert_eq!(out, "SELECT 1 FROM read_parquet('<redacted>') t");
    }

    #[test]
    fn redact_parquet_sources_handles_several_and_leaves_other_sql_alone() {
        let sql = "SELECT * FROM read_parquet('/a.parquet')                    JOIN read_parquet('/b.parquet') USING (id)";
        let out = redact_parquet_sources(sql);
        assert_eq!(out.matches("'<redacted>'").count(), 2);
        assert!(!out.contains(".parquet"));
        assert_eq!(redact_parquet_sources("SELECT 1"), "SELECT 1");
        // Malformed input must not loop forever — and must not fall back to
        // emitting the remainder, which is how an injected unbalanced quote
        // would disclose a later real path.
        assert_eq!(
            redact_parquet_sources("read_parquet('unclosed"),
            "read_parquet('<redacted>'"
        );
        assert_eq!(
            redact_parquet_sources(
                "SELECT * FROM read_parquet('/real.parquet') \
                 WHERE a = 'read_parquet('x'"
            ),
            "SELECT * FROM read_parquet('<redacted>') WHERE a = 'read_parquet('<redacted>'",
            "an injected unbalanced quote truncates instead of leaking the rest"
        );
    }

    #[test]
    fn airhouse_managed_reports_the_dialect_it_actually_speaks() {
        // The invariant this file's fix rests on. `database_type` names the
        // config keyword; `dialect()` names the engine. Handing airlayer the
        // keyword is why a DuckDB view compiled as ClickHouse: `from_str`
        // cannot classify "airhouse_managed", so the datasource is dropped
        // from the dialect map and silently inherits whichever dialect
        // config.yml happens to list first.
        use oxy::config::model::{AirhouseManaged, Database, DatabaseType};

        let db = Database {
            name: "july_airhouse".to_string(),
            database_type: DatabaseType::AirhouseManaged(AirhouseManaged {}),
        };

        assert_eq!(db.database_type.to_string(), "airhouse_managed");
        assert_eq!(db.dialect(), "duckdb");
        // Deliberately NOT asserting that airlayer cannot parse the raw keyword.
        // oxy-hq/airlayer#93 teaches it exactly that, so the assertion would be
        // a tripwire that fires on the pin-bump PR -- the change least equipped
        // to interpret it, and the one where `dialect()` is still correct
        // anyway. What matters is the mapping below, which survives the bump.
        assert_eq!(
            oxy_airlayer_compat::Dialect::from_str(&db.dialect()),
            Some(oxy_airlayer_compat::Dialect::DuckDB),
            "dialect() must land on the engine airhouse actually speaks"
        );
    }
}
