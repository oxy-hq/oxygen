//! Peer-cohort comparison over the semantic model — the IDE/workspace surface.
//!
//! IDE twin of [`crate::server::api::projects::cohort`], and the pair follows
//! the metric-tree precedent exactly: **this module owns the wire types and
//! the pure helpers, the customer-app module imports them**, so the two
//! surfaces cannot answer the same question with different shapes. What
//! differs between them is only how a request gets in and where the semantic
//! model comes from:
//!
//! | | IDE (here) | Customer app (`projects::cohort`) |
//! | --- | --- | --- |
//! | Entry | `workspace_middleware` + `AuthenticatedUserExtractor` | `enter_semantic_boundary` (custom-app gate chain) |
//! | Layer | `resolve_query_scan_source` — compile boundary, working copy second | `load_layer` off the boundary's scan handle |
//! | Population | the whole workspace | scoped to the caller's reach |
//! | Result cache | none | `cache_lookup` / `cache_store`, keyed on reach |
//!
//! **The population difference is deliberate, not an omission.** A custom-app
//! viewer can be location-scoped, and a cohort baseline is computed *over a
//! population*, so on that surface the population is the security boundary. A
//! workspace member on `/ide` already sees every instance in the World Model,
//! and the `metric-tree/opportunity` section directly above this one on the
//! same panel already sizes population-wide. Reach-scoping here would invent
//! an authority rule no other IDE route has, and would make the two sections
//! of one panel disagree about who the peers are.
//!
//! **Route role: `FleetOk`**, like all ten `semantic/metric-tree*` mounts
//! beside it. Every read is the compile boundary (Postgres) with a working-copy
//! fallback via `resolve_query_scan_source`; nothing touches `.git` or the
//! local state dir, and branch pinning is `workspace_middleware`'s job (it
//! resolves the request's revision), not this handler's. See the
//! `oxy-route-classification` skill.
//!
//! Three contracts the response shape exists to preserve, all easy to break by
//! "tidying" — restated here rather than only in the twin because this is now
//! the module that owns the types:
//!
//!  1. **`excluded` is not optional detail.** A subject that cannot be
//!     compared comes back with a reason. The feature exists partly because in
//!     the hand-rolled predecessor a store "simply did not appear in the list
//!     and no screen said why".
//!  2. **`min_peers` is a reporting predicate, never a filter.** A subject
//!     below the floor is returned with its comparison computed and
//!     `sufficient: false`. Filtering it here would relocate a product
//!     decision into the transport layer.
//!  3. **The cohort name and band window are echoed back**, so a UI rendering
//!     them cannot drift from the query that produced the numbers.

mod error;
mod resolve;
mod wire;

pub use error::{CohortError, CohortErrorBody};
pub(crate) use resolve::{
    COHORT_TIMEOUT, parse_statistic, prepare_cohort_layer, resolve_cohort_name,
};
pub use wire::{CohortRequest, CohortResponse, CohortSubjectDto, ExcludedSubjectDto};

use std::sync::Arc;

use axum::Json;
use entity::workspace_members::WorkspaceRole;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::ReadOnly;
use oxy_airlayer_compat::engine::metric_tree_ops::BenchmarkStatistic;
use oxy_airlayer_compat::{DatabaseConfig, SemanticEngine, SemanticLayer};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use uuid::Uuid;

use crate::agentic_wiring::metric_tree_runner::{
    OxyMetricTreeRunner, RunnerPreagg, build_query_executor,
};
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, PreaggCacheCtx, WorkspaceManagerReadOnly,
};
use crate::server::api::semantic::resolve_query_scan_source;

/// `POST /api/{workspace_id}/semantic/cohort` — peer-cohort comparison for the
/// IDE's World Model panel.
///
/// Population-wide by construction; see the module docs for why this surface
/// does not reach-scope the way its customer-app twin does.
pub async fn post_cohort(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    preagg_ctx: PreaggCacheCtx,
    Json(req): Json<CohortRequest>,
) -> Result<Json<CohortResponse>, CohortError> {
    let statistic =
        parse_statistic(req.statistic.as_deref()).map_err(CohortError::InvalidStatistic)?;
    let prepared = prepare_ide_cohort(&workspace_manager, &req).await?;
    let preagg = RunnerPreagg {
        cache: preagg_ctx.cache.clone(),
        renewal_threshold_secs: preagg_ctx
            .renewal_threshold_secs_or(&workspace_manager.config_manager),
        // A read surface: a cohort comparison is a display of the data, so a
        // rollup a cycle behind beats a warehouse scan.
        freshness: crate::server::preagg_context::RollupFreshness::ServeStale,
    };
    let caller = Caller {
        workspace_manager,
        user_id: user.id,
        role,
        preagg,
    };
    run_cohort(prepared, req, statistic, caller).await.map(Json)
}

/// Everything resolved from the semantic model before a warehouse is touched.
struct PreparedCohort {
    augmented: SemanticLayer,
    cohort_name: String,
    engine: Arc<SemanticEngine>,
    databases: Vec<DatabaseConfig>,
}

/// Who the warehouse pulls run as, and with which rollup cache.
struct Caller {
    workspace_manager: WorkspaceManager<ReadOnly>,
    user_id: Uuid,
    role: WorkspaceRole,
    preagg: RunnerPreagg,
}

/// Load the layer, resolve the cohort, install its guard measure and build the
/// request's engine.
async fn prepare_ide_cohort(
    workspace_manager: &WorkspaceManager<ReadOnly>,
    req: &CohortRequest,
) -> Result<PreparedCohort, CohortError> {
    // `source` owns the materialised tempdir; hold it until the layer below is
    // parsed. Same invariant every metric-tree handler keeps — a
    // `spawn_blocking` task outlives handler cancellation, so nothing inside
    // one may read the scan path.
    let source = resolve_query_scan_source(workspace_manager)
        .await
        .map_err(|e| CohortError::ScanUnavailable(e.message()))?;
    let layer = OxyMetricTreeRunner::load_layer_at(&source.scan_path)
        .map_err(|e| CohortError::LayerLoad(e.to_string()))?;

    let cohort_name = resolve_cohort_name(&layer, &req.entity, &req.measure, req.cohort.as_deref())
        .map_err(CohortError::CohortNotResolved)?;

    // THE GUARD. `augment_layer_for_cohort` installs the synthetic
    // `__cohort_total__` measure the truncation check reads. It fails quietly,
    // so a `false` must become a refusal: without it a truncated entity pull
    // would be medianed as though it were the whole population.
    let augmented = prepare_cohort_layer(&layer, &req.entity).map_err(|reason| {
        CohortError::AugmentFailed(format!(
            "cannot resolve a cohort on entity `{}`: {reason}",
            req.entity
        ))
    })?;

    let databases = OxyMetricTreeRunner::list_databases_sync(workspace_manager);
    // Not cached, and deliberately: `prepare_cohort_layer` rewrote the layer
    // for this request's entity, so this engine is unique to the request and
    // shares nothing with the workspace's.
    let engine = Arc::new(
        oxy_airlayer_compat::build_engine(augmented.clone(), &databases)
            .map_err(|e| CohortError::EngineBuild(e.to_string()))?,
    );
    Ok(PreparedCohort {
        augmented,
        cohort_name,
        engine,
        databases,
    })
}

/// Resolve the cohort on a blocking thread, bounded by [`COHORT_TIMEOUT`].
async fn run_cohort(
    prepared: PreparedCohort,
    req: CohortRequest,
    statistic: BenchmarkStatistic,
    caller: Caller,
) -> Result<CohortResponse, CohortError> {
    let handle = tokio::runtime::Handle::current();
    // Read on the request's task: the blocking thread below is off it.
    let holds_writes = crate::server::previews::request_hold::active();

    let run = tokio::task::spawn_blocking(move || {
        // Unlike the customer-app twin this executor is unwrapped: there is no
        // reach filter to fold into each pull, because this surface answers
        // over the whole workspace population.
        let executor = build_query_executor(
            prepared.engine,
            prepared.databases,
            caller.workspace_manager,
            caller.user_id,
            caller.role,
            handle,
            caller.preagg,
            holds_writes,
        );
        oxy_airlayer_compat::engine::cohort::resolve_cohort(
            &prepared.augmented,
            &req.entity,
            &prepared.cohort_name,
            &req.measure,
            &req.time_dimension,
            (req.period.0.as_str(), req.period.1.as_str()),
            statistic,
            &*executor,
        )
    });

    match tokio::time::timeout(COHORT_TIMEOUT, run).await {
        Ok(Ok(Ok(result))) => Ok(CohortResponse::from(&result)),
        Ok(Ok(Err(e))) => Err(CohortError::Failed(e.to_string())),
        Ok(Err(e)) => Err(CohortError::Panicked(e.to_string())),
        Err(_) => Err(CohortError::Timeout),
    }
}
