//! `/api/projects/{project_id}/semantic/cohort` — peer-cohort comparison for
//! customer-app bundles.
//!
//! Sibling of [`super::metric_tree`]: same customer-app gate, same
//! compile-boundary layer load, same `route_fleet` mount. Where the metric
//! tree decomposes ONE subject over time, this compares MANY subjects against
//! each other at a point in time — airlayer's `engine::cohort`, added in
//! airlayer PR #116.
//!
//! **The wire types and the model-only helpers live in
//! [`crate::server::api::cohort`]**, the IDE twin, and are imported here — the
//! same direction `super::metric_tree` imports `OpportunityRequest` from
//! `api::metric_tree`. The three response contracts they carry (`excluded`
//! always explained, `min_peers` reports rather than filters, cohort + band
//! window echoed back) are documented there, next to the types that encode
//! them.
//!
//! What stays here is everything the IDE surface has no counterpart for:
//! the custom-app gate, the result cache, and — the substantive one — **reach
//! scoping**. A cohort baseline is computed over a POPULATION, so on this
//! surface the population is the security boundary: every pull is filtered to
//! the caller's locations, per bound view, in that view's own id space.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use uuid::Uuid;

use std::collections::{BTreeSet, HashMap};

use oxy_airlayer_compat::BoundView;
use oxy_airlayer_compat::engine::query::{FilterOperator, QueryFilter};

// One definition of the contract, shared with the IDE surface so the two
// cannot drift. `COHORT_TIMEOUT` is THE deadline, not a copy of it: both
// surfaces query the same warehouse.
use crate::server::api::cohort::{
    COHORT_TIMEOUT, CohortRequest, CohortResponse, parse_statistic, prepare_cohort_layer,
    resolve_cohort_name,
};
use crate::server::api::custom_apps_gates::parse_versioned_body;
use crate::server::api::operating_graph::binding::NO_REACH_SENTINEL;
use crate::server::api::projects::semantic_boundary::{
    cache_lookup, cache_store, enter_semantic_boundary, err_with_code, load_layer, wants_refresh,
};
use crate::server::router::AppState;

// ── Reach scoping (this surface only) ───────────────────────────────────────

/// The bound views that can carry this cohort's entity.
///
/// Several views routinely bind the SAME entity through DIFFERENT integrations
/// — `sales`/`toast` and `labor`/`payroll` both declaring `store`. So this
/// returns all of them and the choice is made per query, not once per request.
pub(crate) fn cohort_bound_views(bound: &[BoundView], entity: &str) -> Vec<BoundView> {
    bound
        .iter()
        .filter(|b| b.entity == entity)
        .cloned()
        .collect()
}

/// One reach filter per bound view the pull actually reads.
///
/// Which view a filter lands on decides which id space it is compared against:
/// `binding.system` selects the external ids, so filtering `labor` with Toast
/// restaurant ids matches nothing and reads as an empty cohort rather than an
/// error. `resolve_cohort` issues several pulls over different views, so the
/// filter is chosen against each pull's own members.
///
/// Refuses when the pull reads no bound view at all — answering it unscoped
/// would hand a location-scoped caller the whole population.
pub(crate) fn scope_filters_for_request(
    cohort_bound: &[BoundView],
    referenced: &BTreeSet<String>,
    keys_by_system: &HashMap<String, Vec<String>>,
) -> Result<Vec<QueryFilter>, String> {
    let touched: Vec<&BoundView> = cohort_bound
        .iter()
        .filter(|b| referenced.contains(&b.view))
        .collect();
    if touched.is_empty() {
        return Err(
            "this cohort pull reads no view bound to the locations registry, so it cannot \
             be limited to your locations"
                .to_string(),
        );
    }
    Ok(touched
        .into_iter()
        .map(|b| {
            // Per bound view, in ITS OWN id space: `binding.system` selects the
            // external ids, so a `labor` view scoped with Toast ids would match
            // nothing and read as an empty cohort rather than an error.
            let keys = keys_by_system
                .get(&b.binding.system)
                .cloned()
                .unwrap_or_default();
            reach_filter(&b.view, &b.key, &keys)
        })
        .collect())
}

/// The `view` half of every member a pull names.
pub(crate) fn referenced_views(
    q: &oxy_airlayer_compat::engine::query::QueryRequest,
) -> BTreeSet<String> {
    q.measures
        .iter()
        .chain(q.dimensions.iter())
        .chain(q.time_dimensions.iter().map(|t| &t.dimension))
        .chain(q.filters.iter().filter_map(|f| f.member.as_ref()))
        .filter_map(|m| m.split_once('.').map(|(v, _)| v.to_string()))
        .collect()
}

/// The reach filter pinning a cohort pull to the caller's stores.
///
/// An empty `keys` yields the sentinel rather than an empty `IN ()`, which is
/// not SQL in every dialect.
pub(crate) fn reach_filter(view: &str, key: &str, keys: &[String]) -> QueryFilter {
    let values = if keys.is_empty() {
        vec![NO_REACH_SENTINEL.to_string()]
    } else {
        keys.to_vec()
    };
    QueryFilter {
        // `equals` with several values compiles to an IN list.
        member: Some(format!("{view}.{key}")),
        operator: Some(FilterOperator::Equals),
        values,
        and: None,
        or: None,
    }
}

/// A stable string identifying the population a caller may see, for use in a
/// cache key, so two differently-scoped callers can never share an entry.
///
/// Keyed on the reach's location UUIDs rather than the resolved warehouse
/// keys, matching `semantic_query.rs`: an external id is warehouse-supplied and
/// may contain the separator, so two distinct reach sets could serialize
/// identically. A UUID cannot.
pub(crate) fn scope_cache_discriminator(
    reach: &crate::server::api::operating_graph::reach::Reach,
) -> String {
    format!(
        "reach:{}:{}",
        reach.everywhere,
        reach
            .locations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// `POST /api/projects/{project_id}/semantic/cohort`
pub async fn post_cohort(
    Path(project_id): Path<Uuid>,
    uri: Uri,
    headers: HeaderMap,
    State(app_state): State<AppState>,
    body: axum::body::Bytes,
) -> Response {
    // Gate before the cache read so a cached hit cannot bypass authorization.
    let boundary = match enter_semantic_boundary(&headers, project_id).await {
        Ok(b) => b,
        Err(resp) => return resp,
    };

    let req: CohortRequest = match parse_versioned_body(&body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let statistic = match parse_statistic(req.statistic.as_deref()) {
        Ok(s) => s,
        Err(msg) => return err_with_code(StatusCode::BAD_REQUEST, msg, "invalid_statistic"),
    };

    let layer = match load_layer(boundary.scan.path_buf()).await {
        Ok(l) => l,
        Err(resp) => return resp,
    };

    let cohort_name =
        match resolve_cohort_name(&layer, &req.entity, &req.measure, req.cohort.as_deref()) {
            Ok(c) => c,
            Err(msg) => {
                return err_with_code(StatusCode::BAD_REQUEST, msg, "cohort_not_resolved");
            }
        };

    // THE GUARD. `augment_layer_for_cohort` installs the synthetic
    // `__cohort_total__` measure the truncation check reads. It fails quietly,
    // so a `false` must become a refusal: without it a truncated entity pull
    // would be medianed as though it were the whole population.
    let augmented = match prepare_cohort_layer(&layer, &req.entity) {
        Ok(l) => l,
        Err(reason) => {
            return err_with_code(
                StatusCode::BAD_REQUEST,
                format!(
                    "cannot resolve a cohort on entity `{}`: {reason}",
                    req.entity
                ),
                "cohort_augment_failed",
            );
        }
    };

    // ── Population scoping ──────────────────────────────────────────────
    // A cohort baseline is computed over a population, so the population is
    // the security boundary, not just the row set. Scope it to the caller's
    // reach the way `semantic_query.rs` does.
    let reach = crate::server::api::operating_graph::reach::reach_for_viewer(
        &boundary.app.db,
        boundary.app.org_id,
        &boundary.app.user,
        project_id,
        req.app,
    )
    .await;

    // Resolve the binding BEFORE branching on `everywhere`. `apply_reach_scope`
    // refuses an unscopable query even for an everywhere caller, and for the
    // same reason: otherwise a workspace with nothing bound ships green and the
    // first location-scoped viewer in production is the one who discovers it,
    // mid-session, as a 403.
    let cohort_bound =
        cohort_bound_views(&oxy_airlayer_compat::bound_views(&augmented), &req.entity);
    if cohort_bound.is_empty() {
        return err_with_code(
            StatusCode::FORBIDDEN,
            format!(
                "entity `{}` is not bound to the locations registry, so a cohort over it \
                 cannot be limited to a caller's locations",
                req.entity
            ),
            "cohort_scope_unavailable",
        );
    }

    // One key list per bound system: two views can bind the same entity through
    // different integrations, and each pull is filtered in its own id space.
    let keys_by_system = if reach.everywhere {
        None
    } else {
        let mut by_system: HashMap<String, Vec<String>> = HashMap::new();
        for system in cohort_bound
            .iter()
            .map(|b| b.binding.system.clone())
            .collect::<BTreeSet<_>>()
        {
            match crate::server::api::operating_graph::binding::external_ids_for_locations(
                &boundary.app.db,
                boundary.app.org_id,
                &system,
                &reach.locations,
            )
            .await
            {
                Ok(k) => {
                    by_system.insert(system, k);
                }
                Err(e) => {
                    return err_with_code(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("failed to resolve your location scope: {e}"),
                        "cohort_scope_failed",
                    );
                }
            }
        }
        Some(by_system)
    };

    // Cache key includes the caller's scope, not just the body. Two callers
    // with different reach ask the same question and must NOT get the same
    // answer — keying on the body alone would serve one caller's population
    // to another. Read it only now that the scope is known.
    let cache_key = format!(
        "{}|{}",
        scope_cache_discriminator(&reach),
        String::from_utf8_lossy(&body)
    );
    if let Some(hit) = cache_lookup(&boundary, "cohort", &cache_key, wants_refresh(uri.query())) {
        return hit;
    }

    // ── Executor ────────────────────────────────────────────────────────
    let workspace_manager = boundary.proj_ctx.workspace_manager().clone();
    let databases =
        crate::agentic_wiring::metric_tree_runner::OxyMetricTreeRunner::list_databases_sync(
            &workspace_manager,
        );
    let dialects = oxy_airlayer_compat::DatasourceDialectMap::from_config_databases(&databases);
    let engine =
        match oxy_airlayer_compat::SemanticEngine::from_semantic_layer(augmented.clone(), dialects)
        {
            Ok(e) => std::sync::Arc::new(e),
            Err(e) => {
                return err_with_code(
                    StatusCode::BAD_REQUEST,
                    format!("failed to build the semantic engine: {e}"),
                    "semantic_engine_build_failed",
                );
            }
        };

    let preagg_ctx = crate::server::api::middlewares::workspace_context::PreaggCacheCtx {
        cache: app_state.preagg_cache.clone(),
        renewal_threshold_secs: app_state.preagg_renewal_threshold_secs,
    };
    let renewal_threshold_secs =
        preagg_ctx.renewal_threshold_secs_or(&workspace_manager.config_manager);

    let inner = crate::agentic_wiring::metric_tree_runner::build_query_executor(
        engine,
        databases,
        workspace_manager.into_read_only(),
        boundary.app.user.id,
        entity::workspace_members::WorkspaceRole::Viewer,
        tokio::runtime::Handle::current(),
        crate::agentic_wiring::metric_tree_runner::RunnerPreagg {
            cache: preagg_ctx.cache,
            renewal_threshold_secs,
            // A read surface, same as `/semantic-query` and `ctx.semantic`.
            freshness: crate::server::preagg_context::RollupFreshness::ServeStale,
        },
        crate::server::previews::request_hold::active(),
    );

    // Wrap rather than pre-filter: `resolve_cohort` builds its own
    // `QueryRequest`s internally (the entity-grain pull and the band-window
    // pull), so the executor is the only place that sees both. Choosing the
    // filter per pull — rather than once per request — is what keeps a pull
    // over a differently-bound view in its own id space.
    let executor: Box<oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor> =
        match keys_by_system {
            None => inner,
            Some(keys) => Box::new(
                move |q: &oxy_airlayer_compat::engine::query::QueryRequest| {
                    let filters =
                        scope_filters_for_request(&cohort_bound, &referenced_views(q), &keys)
                            .map_err(oxy_airlayer_compat::engine::EngineError::QueryError)?;
                    let mut scoped = q.clone();
                    scoped.filters.extend(filters);
                    inner(&scoped)
                },
            ),
        };

    let entity = req.entity.clone();
    let measure = req.measure.clone();
    let time_dimension = req.time_dimension.clone();
    let period = req.period.clone();
    let cohort_for_run = cohort_name.clone();

    // Sentry hubs are per thread: carry the request's onto the blocking pool,
    // so a warehouse error or a panic while resolving the cohort is captured
    // under the custom-app surface tag (`middlewares::sentry_surface`), not on
    // the pool thread's bare hub — this request entered the gate through
    // `enter_semantic_boundary`, and the tag lives on the hub it tagged.
    let hub = sentry::Hub::current();
    let run = tokio::task::spawn_blocking(move || {
        sentry::Hub::run(hub, || {
            oxy_airlayer_compat::engine::cohort::resolve_cohort(
                &augmented,
                &entity,
                &cohort_for_run,
                &measure,
                &time_dimension,
                (period.0.as_str(), period.1.as_str()),
                statistic,
                &*executor,
            )
        })
    });

    match tokio::time::timeout(COHORT_TIMEOUT, run).await {
        Ok(Ok(Ok(result))) => cache_store(
            &boundary,
            "cohort",
            &cache_key,
            &CohortResponse::from(&result),
        ),
        Ok(Ok(Err(e))) => err_with_code(StatusCode::BAD_REQUEST, e.to_string(), "cohort_failed"),
        Ok(Err(e)) => err_with_code(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cohort task panicked: {e}"),
            "cohort_panicked",
        ),
        Err(_) => err_with_code(
            StatusCode::GATEWAY_TIMEOUT,
            "cohort resolution timed out".to_string(),
            "cohort_timeout",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound(view: &str, key: &str, system: &str) -> BoundView {
        BoundView {
            view: view.into(),
            entity: "store".into(),
            key: key.into(),
            binding: oxy_airlayer_compat::EntityBinding {
                registry: "locations".into(),
                system: system.into(),
            },
        }
    }

    fn two_systems() -> (Vec<BoundView>, HashMap<String, Vec<String>>) {
        let bound = vec![
            bound("sales", "restaurant_id", "toast"),
            bound("labor", "site", "payroll"),
        ];
        let keys = HashMap::from([
            (
                "toast".to_string(),
                vec!["t1".to_string(), "t2".to_string()],
            ),
            ("payroll".to_string(), vec!["p1".to_string()]),
        ]);
        (bound, keys)
    }

    fn refs(vs: &[&str]) -> BTreeSet<String> {
        vs.iter().map(|v| v.to_string()).collect()
    }

    /// Two views can bind the SAME entity through different integrations.
    /// The filter must follow the view the pull reads, or it compares one
    /// system's ids against another's and silently matches nothing.
    #[test]
    fn scope_filter_follows_the_view_the_pull_reads() {
        let (b, keys) = two_systems();
        let cohort_bound = cohort_bound_views(&b, "store");

        let f = scope_filters_for_request(&cohort_bound, &refs(&["sales"]), &keys).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].member.as_deref(), Some("sales.restaurant_id"));
        assert_eq!(f[0].values, vec!["t1".to_string(), "t2".to_string()]);

        let f = scope_filters_for_request(&cohort_bound, &refs(&["labor"]), &keys).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(
            f[0].member.as_deref(),
            Some("labor.site"),
            "a pull over `labor` must be scoped with payroll ids, not Toast ids"
        );
        assert_eq!(f[0].values, vec!["p1".to_string()]);
    }

    #[test]
    fn scope_filters_every_bound_view_a_pull_joins() {
        let (b, keys) = two_systems();
        let cohort_bound = cohort_bound_views(&b, "store");
        let f =
            scope_filters_for_request(&cohort_bound, &refs(&["sales", "labor"]), &keys).unwrap();
        assert_eq!(f.len(), 2, "both bound views the pull reads must be pinned");
    }

    /// Refuse rather than answer a scoped caller with the whole population.
    #[test]
    fn scope_refuses_a_pull_that_reads_no_bound_view() {
        let (b, keys) = two_systems();
        let cohort_bound = cohort_bound_views(&b, "store");
        assert!(scope_filters_for_request(&cohort_bound, &refs(&["unbound"]), &keys).is_err());
    }

    #[test]
    fn cohort_bound_views_keeps_every_view_binding_the_entity() {
        let (b, _) = two_systems();
        assert_eq!(cohort_bound_views(&b, "store").len(), 2);
        assert!(cohort_bound_views(&b, "employee").is_empty());
    }

    #[test]
    fn reach_filter_pins_the_entity_key_to_the_callers_stores() {
        let f = reach_filter("restaurants", "restaurant_id", &["g1".into(), "g2".into()]);
        assert_eq!(f.member.as_deref(), Some("restaurants.restaurant_id"));
        assert_eq!(f.operator, Some(FilterOperator::Equals));
        assert_eq!(f.values, vec!["g1".to_string(), "g2".to_string()]);
    }

    /// A scoped caller whose places carry no key in the bound system must
    /// match nothing — never an empty `IN ()`, and never the whole population.
    #[test]
    fn reach_filter_with_no_keys_matches_nothing_rather_than_everything() {
        let f = reach_filter("restaurants", "restaurant_id", &[]);
        assert_eq!(f.values, vec![NO_REACH_SENTINEL.to_string()]);
    }
}
