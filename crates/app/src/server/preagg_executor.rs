//! PreaggTaskExecutor: runs `preagg_cycle` tasks via the agentic Worker
//! infrastructure.
//!
//! One executor instance per process, holding only a `db` handle — like
//! `HealthEvalTaskExecutor`, not like the pre-scheduling worker this replaced.
//! Everything workspace-specific (the `WorkspaceManager`, the pre-agg config,
//! the Layer-1 cache, the manifest write lock) is built fresh per task from
//! `workspace_id` in the payload, so the SAME executor instance correctly
//! serves however many workspaces' cycles the fleet schedules onto this node —
//! see `preagg_workspace::build_workspace_manager`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use agentic_automation::preagg_event::PreaggEvent;
use agentic_automation::workspace::WorkspaceContext;
use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use agentic_semantic::refresh_key_cache::RefreshKeyCache;
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use serde::{Deserialize, Serialize};

use super::preagg_freshness::{evaluate_refresh_key, rollup_refresh_key};
use super::preagg_generation::{
    GenerationSweep, PREAGG_BUILDER_GENERATION, built_rollup_hashes, read_builder_generation,
    resolve_failed_rollup, write_builder_generation,
};
use super::preagg_ledger;
use super::preagg_rebuild::{
    RebuildContext, build_layer_engine, connector_to_airlayer_dialect, rebuild_rollup,
};
use super::preagg_workspace::{build_workspace_manager, manifest_write_lock_for};
use crate::agentic_wiring::OxyProjectContext;

/// The bag `run_preagg_task`/`RebuildContext` need, built fresh per task from
/// the payload's `workspace_id` — see [`build_task_config`].
pub(super) struct PreaggWorkerConfig {
    pub renewal_threshold: std::time::Duration,
    /// The pre-aggregation cache key. Not the workspace PATH, which the
    /// reader side resolves per-branch — see `state_dir::airlayer_cache_key`.
    pub workspace_id: Uuid,
    pub schema: String,
    pub database: Option<String>,
    pub manifest_write_lock: Arc<tokio::sync::Mutex<()>>,
}

/// One rollup, addressed the way the UI addresses it.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RollupTarget {
    pub view: String,
    pub rollup: String,
}

/// What a `preagg_cycle` task was asked to do.
///
/// `workspace_id` is REQUIRED, not defaulted: there is no more single
/// startup-bound workspace to fall back to (see the module doc). A task
/// without one is a stale payload from before this shape existed — the
/// executor fails it cleanly rather than guessing.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PreaggCycleRequest {
    pub workspace_id: Uuid,
    /// Rebuild whether or not the refresh key says the rollup is stale. A
    /// person pressing Rebuild is saying the key is not the authority right
    /// now — most often because they just changed the data behind it.
    #[serde(default)]
    pub force: bool,
    /// Restrict the cycle to one rollup. `None` covers every declared one.
    #[serde(default)]
    pub target: Option<RollupTarget>,
}

impl PreaggCycleRequest {
    /// Whether this cycle should touch `view.rollup` at all.
    fn covers(&self, view: &str, rollup: &str) -> bool {
        self.target
            .as_ref()
            .is_none_or(|t| t.view == view && t.rollup == rollup)
    }
}

pub struct PreaggTaskExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for PreaggTaskExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let TaskSpec::Custom { kind, payload } = &assignment.spec else {
            return Err(format!(
                "unexpected spec for PreaggTaskExecutor: {:?}",
                assignment.spec
            ));
        };
        if kind != "preagg_cycle" {
            return Err(format!("unknown preagg kind: {kind}"));
        }
        let request: PreaggCycleRequest = serde_json::from_value(payload.clone()).map_err(|e| {
            format!(
                "bad preagg_cycle payload: {e} (a task queued before workspace_id was \
                 required in this payload can no longer be served — it will not be retried)"
            )
        })?;

        let (event_tx, event_rx) = mpsc::channel(256);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let db = self.db.clone();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            run_preagg_task_for_workspace(db, request, event_tx, outcome_tx, cancel_clone).await;
        });

        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        })
    }
}

/// Build everything `run_preagg_task` needs for one workspace: its
/// `OxyProjectContext`, the config bag, and a throwaway Layer-1 cache.
///
/// The cache is fresh per task rather than shared process-wide: unlike the
/// pre-scheduling worker (one workspace, one long-lived cache, shared with
/// every query the same process served), a cycle here may run for a different
/// workspace each time, and the cache exists only to avoid re-probing a `sql:`
/// refresh key within its `renewal_threshold` across ticks of the SAME
/// workspace. A cold cache costs one extra probe on the rare case two
/// consecutive cycles for the same workspace land on different processes;
/// sharing it would cost a lookup keyed across every workspace this process
/// ever touches for no benefit a fleet-scheduled cycle can actually collect.
async fn build_task_config(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<
    (
        OxyProjectContext,
        PreaggWorkerConfig,
        Arc<RwLock<RefreshKeyCache>>,
    ),
    String,
> {
    let workspace_manager = build_workspace_manager(db, workspace_id).await?;
    let cfg = workspace_manager
        .config_manager
        .get_config()
        .pre_aggregations
        .clone();
    let renewal_threshold = oxy::config::preagg_check::resolve_renewal_threshold(cfg.as_ref());
    let ctx = OxyProjectContext::new(workspace_manager)
        .with_preagg_renewal_threshold_secs(renewal_threshold.as_secs());

    let config = PreaggWorkerConfig {
        renewal_threshold,
        workspace_id,
        schema: oxy::config::preagg_check::resolve_schema(cfg.as_ref()),
        database: oxy::config::preagg_check::resolve_database(cfg.as_ref()),
        manifest_write_lock: manifest_write_lock_for(workspace_id),
    };
    let cache = Arc::new(RwLock::new(RefreshKeyCache::new()));
    Ok((ctx, config, cache))
}

// ── Main task function ────────────────────────────────────────────────────────

/// Resolve `request.workspace_id`'s context, then delegate to
/// [`run_preagg_task`] — the actual stale-detection + rebuild loop, unchanged
/// from before this module ran per-workspace instead of per-process.
async fn run_preagg_task_for_workspace(
    db: DatabaseConnection,
    request: PreaggCycleRequest,
    event_tx: mpsc::Sender<(String, serde_json::Value)>,
    outcome_tx: mpsc::Sender<TaskOutcome>,
    cancel: CancellationToken,
) {
    let workspace_id = request.workspace_id;
    let (ctx, config, cache) = match build_task_config(&db, workspace_id).await {
        Ok(v) => v,
        Err(e) => {
            let _ = outcome_tx
                .send(TaskOutcome::Failed(format!(
                    "preagg: could not build workspace context for {workspace_id}: {e}"
                )))
                .await;
            return;
        }
    };
    run_preagg_task(
        Arc::new(config),
        cache,
        Arc::new(ctx),
        request,
        event_tx,
        outcome_tx,
        cancel,
    )
    .await;
}

async fn run_preagg_task(
    config: Arc<PreaggWorkerConfig>,
    cache: Arc<RwLock<RefreshKeyCache>>,
    ctx: Arc<OxyProjectContext>,
    request: PreaggCycleRequest,
    event_tx: mpsc::Sender<(String, serde_json::Value)>,
    outcome_tx: mpsc::Sender<TaskOutcome>,
    cancel: CancellationToken,
) {
    use tokio::task::JoinSet;

    // Evict stale entries for rollups that may no longer exist in views.
    // Use 2× the renewal_threshold as max_age so entries older than that
    // are certainly from prior cycles and won't be re-validated this tick.
    {
        let mut guard = cache.write().expect("preagg cache lock poisoned");
        guard.sweep(config.renewal_threshold * 2);
    }

    let views = match load_views(ctx.workspace_manager(), config.database.as_deref()).await {
        Ok(v) => v,
        Err(e) => {
            let _ = outcome_tx
                .send(TaskOutcome::Failed(format!("load_view_files: {e}")))
                .await;
            return;
        }
    };

    // Resolved once, before anything reads or prunes against the declared set.
    // A rollup that refuses to resolve fails the cycle here, naming itself: the
    // layer-wide engine below would refuse the same layer anyway, but only
    // after the ledger had been pruned against whatever the other views
    // declared.
    let resolved = match resolve_cycle_rollups(&views) {
        Ok(r) => r,
        Err(e) => {
            let _ = outcome_tx
                .send(TaskOutcome::Failed(format!("preagg: {e}")))
                .await;
            return;
        }
    };

    // The planner needs the WHOLE layer, even for a targeted rebuild of one
    // rollup: cross-view `parent:` chains and measures that reach through
    // another view only resolve against the full set. One engine is built from
    // this per dialect, once per cycle — see `RebuildContext::engine`.
    let layer_views: Vec<oxy_airlayer_compat::View> =
        views.iter().map(|(view, _)| view.clone()).collect();

    // Whether the artifacts on disk were built by THIS builder. Not a
    // freshness question — no refresh key can answer it — so it is settled
    // once per cycle, before any key is probed. See
    // [`PREAGG_BUILDER_GENERATION`].
    let cache_dir = oxy::state_dir::get_airlayer_cache_dir(config.workspace_id);
    let generation_stale = read_builder_generation(&cache_dir) != Some(PREAGG_BUILDER_GENERATION);
    // Intersected with what the loaded layer still declares, which is what
    // makes the sweep converge. A manifest entry for a rollup no longer
    // declared — or for a database this workspace's config no longer selects —
    // can never be rebuilt, so requiring it would leave the stamp unwritten and
    // the cycle re-sweeping forever. What removes the leftovers depends on WHY
    // the hash went undeclared: a rollup whose definition merely changed is
    // superseded, and `commit_manifest_and_cache` reaps its manifest entry and
    // Parquet when the replacement publishes; one whose `pre_aggregations:`
    // block was DELETED outright has no replacement coming, so its artifacts
    // still sit until a retraction — which is only ever reached from a rebuild
    // — takes them. `preagg_ledger::prune` below drops the LEDGER entry either
    // way, which is a third thing again. The remaining gap is that second
    // case, and it is not this constant's problem.
    let declared: std::collections::HashSet<String> =
        resolved.iter().flatten().map(|r| r.hash.clone()).collect();
    let built = built_rollup_hashes(&cache_dir);

    // Nothing else bounds the ledger: a hash that stops existing — a rollup
    // whose dimensions were edited, or one deleted outright — is never written
    // again and never removed, while every status poll re-reads the file. Once
    // per cycle, under the publish lock, drop what the workspace no longer has.
    {
        let publish = config.manifest_write_lock.lock().await;
        preagg_ledger::prune(&cache_dir, built.union(&declared).cloned().collect()).await;
        drop(publish);
    }

    let mut sweep = if generation_stale {
        GenerationSweep::new(
            built,
            &declared,
            &preagg_ledger::RollupLedger::load(&cache_dir),
            PREAGG_BUILDER_GENERATION,
        )
    } else {
        GenerationSweep::default()
    };
    if generation_stale {
        tracing::info!(
            generation = PREAGG_BUILDER_GENERATION,
            rollups = sweep.len(),
            "preagg: builder generation changed; rebuilding every built rollup once"
        );
    }

    let today = chrono::Utc::now().format("%Y%m%dT%H%M%S").to_string();

    struct StaleWork {
        view: oxy_airlayer_compat::View,
        rollup: oxy_airlayer_compat::preagg::RollupSpec,
        current_value: Option<String>,
        database_name: String,
    }

    let mut stale_work: Vec<StaleWork> = Vec::new();
    // Counts only rollups past both skip gates (configured datasource + has
    // a refresh_key). The "N of total" outcome therefore excludes skipped
    // rollups; skip_suffix reports those counts separately.
    let mut total_rollups: usize = 0;
    let mut skipped_no_key: usize = 0;
    let mut skipped_no_datasource: usize = 0;
    // `declared` above flattens all of `resolved`, while `zip` truncates
    // silently: a length divergence would leave the generation sweep waiting
    // on hashes no iteration here ever probes.
    debug_assert_eq!(views.len(), resolved.len());
    for ((view, database_name), view_rollups) in views.iter().zip(&resolved) {
        // A targeted rebuild touches one view; don't probe refresh keys (which
        // can mean a warehouse round-trip each) across the rest of the layer.
        if let Some(t) = &request.target
            && t.view != view.name
        {
            continue;
        }
        let rollups: Vec<_> = view_rollups
            .iter()
            .filter(|r| request.covers(&view.name, &r.name))
            .cloned()
            .collect();
        if rollups.is_empty() {
            continue;
        }

        // A fresh multi-tenant workspace may not have every datasource the
        // seed/demo views reference. Skip those views rather than let each
        // rollup hard-fail on `get_connector` and fail the whole cycle.
        if !ctx.is_database_configured(database_name) {
            skipped_no_datasource += rollups.len();
            for rollup in &rollups {
                // No cycle can rebuild these until the datasource is
                // configured, so holding the generation stamp hostage to them
                // would mean sweeping the whole workspace on every tick
                // forever. Nothing removes them yet — retraction is the only
                // path that drops a manifest entry, and it is reached from a
                // rebuild — so a read still finds the Parquet, since the
                // rollup path resolves no connector.
                sweep.unrebuildable(&rollup.hash);
                let _ = event_tx
                    .send(
                        PreaggEvent::RollupSkippedNoDatasource {
                            view: view.name.clone(),
                            rollup: rollup.name.clone(),
                            database: database_name.clone(),
                        }
                        .to_wire(),
                    )
                    .await;
            }
            continue;
        }

        for rollup in rollups {
            let refresh_key = rollup_refresh_key(&rollup, view);
            // A rollup built by a previous builder generation is wrong
            // regardless of what its refresh key says, so it overrides every
            // freshness gate below exactly the way `force` does.
            let generation_forces = sweep.contains(&rollup.hash);
            // No refresh key means the heartbeat has no way to tell fresh from
            // stale, so it skips. A forced rebuild has a person's answer to
            // that question and builds it anyway.
            if refresh_key.is_none() && !request.force && !generation_forces {
                skipped_no_key += 1;
                let _ = event_tx
                    .send(
                        PreaggEvent::RollupSkippedNoRefreshKey {
                            view: view.name.clone(),
                            rollup: rollup.name.clone(),
                        }
                        .to_wire(),
                    )
                    .await;
                continue;
            }
            total_rollups += 1;

            let (current_value, mut is_stale, rk_err) = match refresh_key {
                Some(rk) => {
                    evaluate_refresh_key(rk, &rollup.hash, &cache_dir, &cache, &ctx, database_name)
                        .await
                }
                // Forced, and nothing to probe.
                None => (None, true, None),
            };
            // Evaluate the key even when forcing — its value is what lands in
            // the manifest, so skipping it would leave the next heartbeat
            // comparing against a stale probe — but let the person's request,
            // not the verdict, decide whether to rebuild.
            if request.force || generation_forces {
                is_stale = true;
            }

            // A probe that failed is reported either way, but it only *stops*
            // the heartbeat: the probe is how it decides, whereas a forced
            // rebuild has already been decided, and the build itself may well
            // succeed where the key's SQL didn't.
            let probe_failed = rk_err.is_some();
            if let Some(err_msg) = rk_err {
                let _ = event_tx
                    .send(
                        PreaggEvent::RefreshKeyError {
                            rollup_hash: rollup.hash.clone(),
                            error: err_msg,
                        }
                        .to_wire(),
                    )
                    .await;
            }

            if probe_failed && !request.force && !generation_forces {
                // reported above; the heartbeat leaves it for the next tick
            } else if is_stale {
                stale_work.push(StaleWork {
                    view: view.clone(),
                    rollup,
                    current_value,
                    database_name: database_name.clone(),
                });
            } else {
                let _ = event_tx
                    .send(
                        PreaggEvent::RollupFresh {
                            view: view.name.clone(),
                            rollup: rollup.name.clone(),
                        }
                        .to_wire(),
                    )
                    .await;
            }
        }
    }

    if stale_work.is_empty() {
        // Nothing left to invalidate: every generation-stale rollup was either
        // already replaced, or is one no cycle can rebuild (its datasource is
        // not configured, or the layer no longer declares it). Same guard as
        // the one after the fan-out, for the same reason.
        if generation_stale && sweep.is_complete() {
            write_builder_generation(&cache_dir);
        } else if generation_stale {
            // The one path where "why is this still sweeping?" would otherwise
            // have no log line: a targeted request that covered nothing.
            tracing::info!(
                remaining = sweep.len(),
                "preagg: builder generation sweep incomplete; the next cycle finishes it"
            );
        }
        let answer = format!(
            "all rollups are up to date{}",
            skip_suffix(skipped_no_key, skipped_no_datasource)
        );
        let _ = outcome_tx
            .send(TaskOutcome::Done {
                answer,
                metadata: None,
            })
            .await;
        return;
    }

    let total = total_rollups;

    // One engine per dialect for the WHOLE cycle, built before anything is
    // spawned. `SemanticEngine::from_semantic_layer` validates the layer, so
    // building it per rollup meant a single malformed view failed every rollup
    // in the workspace — well-formed ones that built yesterday included — and
    // said so once per rollup. It also deep-copied every view into each task of
    // an unbounded fan-out. A layer that will not validate is one failure, once.
    // Keyed by dialect, then indexed by database: two databases on the same
    // dialect share one engine, and a cycle spanning dialects still gets the
    // right one per rollup.
    let mut by_dialect: Vec<(
        oxy_airlayer_compat::Dialect,
        Arc<oxy_airlayer_compat::SemanticEngine>,
    )> = Vec::new();
    let mut engines: HashMap<String, Arc<oxy_airlayer_compat::SemanticEngine>> = HashMap::new();
    // Per DATABASE, not per cycle. `is_database_configured` above already
    // filtered the not-declared case, so what fails here is "configured but
    // currently unreachable" — transient, and no reason for one warehouse's
    // expired credential to take down every rollup on another. The rollups
    // behind a failed database are reported individually below, exactly as
    // they would have been had `rebuild_rollup` resolved the connector itself.
    let mut database_errors: HashMap<String, String> = HashMap::new();
    for work in &stale_work {
        if engines.contains_key(&work.database_name)
            || database_errors.contains_key(&work.database_name)
        {
            continue;
        }
        let dialect = match ctx.get_connector(&work.database_name).await {
            Ok(connector) => connector_to_airlayer_dialect(connector.dialect()),
            Err(e) => {
                tracing::warn!(
                    database = %work.database_name,
                    error = %e,
                    "preagg: database unreachable; failing its rollups and continuing"
                );
                database_errors.insert(
                    work.database_name.clone(),
                    format!("get_connector({}) failed: {e}", work.database_name),
                );
                continue;
            }
        };
        let engine = match by_dialect.iter().find(|(d, _)| *d == dialect) {
            Some((_, engine)) => Arc::clone(engine),
            None => match build_layer_engine(layer_views.clone(), &dialect) {
                Ok(engine) => {
                    by_dialect.push((dialect, Arc::clone(&engine)));
                    engine
                }
                // A layer that will not validate is one failure, once — the
                // whole point of hoisting this out of the per-rollup path.
                Err(e) => {
                    let _ = outcome_tx
                        .send(TaskOutcome::Failed(format!(
                            "preagg: the semantic model does not validate, so no rollup can be \
                             planned ({dialect:?}): {e}"
                        )))
                        .await;
                    return;
                }
            },
        };
        engines.insert(work.database_name.clone(), engine);
    }

    let mut succeeded = 0usize;
    let mut failed = 0usize;
    // A rollup that rebuilt to zero rows. Its own count because it is neither
    // outcome: the cycle did its job, and the rollup stopped being served.
    let mut retracted = 0usize;
    let mut set: JoinSet<(String, String, String, Result<bool, String>)> = JoinSet::new();

    for work in stale_work {
        if cancel.is_cancelled() {
            break;
        }

        if let Some(error) = database_errors.get(&work.database_name) {
            failed += 1;
            // The identical `get_connector` failure `rebuild_rollup` would have
            // returned as `Err` had the pre-loop happened to succeed, so it
            // resolves identically: same sweep bookkeeping, same decision about
            // the artifact. Skipping it here is what used to leave the hash
            // pending and hold the workspace's sweep open behind one
            // unreachable warehouse.
            let error = error.clone();
            resolve_failed_rollup(
                &mut sweep,
                &work.rollup.hash,
                &work.view.name,
                &work.rollup.name,
                &cache_dir,
                &config,
                &cache,
            )
            .await;
            let _ = event_tx
                .send(
                    PreaggEvent::RollupFailed {
                        view: work.view.name.clone(),
                        rollup: work.rollup.name.clone(),
                        error,
                    }
                    .to_wire(),
                )
                .await;
            continue;
        }

        // Present by construction: the loop above built an engine for every
        // database in `stale_work` that resolved, and the one case it skips —
        // a database whose connector failed — was just reported and `continue`d
        // past. A layer that fails to validate still returns rather than
        // continuing, so no `stale_work` item reaches here without an engine.
        let Some(engine) = engines.get(&work.database_name) else {
            continue;
        };
        let rebuild_ctx = RebuildContext {
            schema: config.schema.clone(),
            workspace_id: config.workspace_id,
            database_name: work.database_name.clone(),
            manifest_write_lock: Arc::clone(&config.manifest_write_lock),
            engine: Arc::clone(engine),
            builder_generation: PREAGG_BUILDER_GENERATION,
        };
        let ctx_clone = Arc::clone(&ctx);
        let cache_clone = Arc::clone(&cache);
        let event_tx_clone = event_tx.clone();
        let view_name = work.view.name.clone();
        let rollup_name = work.rollup.name.clone();
        let rollup_hash = work.rollup.hash.clone();
        let today_clone = today.clone();

        let _ = event_tx_clone
            .send(
                PreaggEvent::RollupStarted {
                    view: view_name.clone(),
                    rollup: rollup_name.clone(),
                }
                .to_wire(),
            )
            .await;

        set.spawn(async move {
            let result = rebuild_rollup(
                work.view,
                work.rollup,
                work.current_value,
                today_clone,
                ctx_clone,
                rebuild_ctx,
                cache_clone,
            )
            .await;
            (view_name, rollup_name, rollup_hash, result)
        });
    }

    while let Some(join_result) = set.join_next().await {
        match join_result {
            Ok((view_name, rollup_name, rollup_hash, result)) => {
                // Every outcome goes through the same mapping: both `Ok`s
                // discharge the hash, an `Err` leaves it pending for the
                // retraction below.
                sweep.record(&rollup_hash, &result);
                let event = match result {
                    Ok(true) => {
                        succeeded += 1;
                        PreaggEvent::RollupDone {
                            view: view_name,
                            rollup: rollup_name,
                        }
                    }
                    // Counted apart from `succeeded`, and reported as its own
                    // event, because the artifact was DELETED: answering
                    // "rebuilt N of M" for a rollup that went from Cached to
                    // Not built is the run row disagreeing with the table. The
                    // tab reads this to clear the row with an explanation
                    // instead of spinning until its deadline.
                    Ok(false) => {
                        retracted += 1;
                        PreaggEvent::RollupRetracted {
                            view: view_name,
                            rollup: rollup_name,
                        }
                    }
                    Err(e) => {
                        failed += 1;
                        tracing::warn!(
                            view = %view_name,
                            rollup = %rollup_name,
                            error = %e,
                            "preagg rollup rebuild failed"
                        );
                        resolve_failed_rollup(
                            &mut sweep,
                            &rollup_hash,
                            &view_name,
                            &rollup_name,
                            &cache_dir,
                            &config,
                            &cache,
                        )
                        .await;
                        PreaggEvent::RollupFailed {
                            view: view_name,
                            rollup: rollup_name,
                            error: e,
                        }
                    }
                };
                let _ = event_tx.send(event.to_wire()).await;
            }
            Err(join_err) => {
                failed += 1;
                tracing::warn!(error = %join_err, "preagg rollup task panicked");
            }
        }
    }

    // The stamp has to prove its own claim — see `GenerationSweep`.
    if generation_stale && sweep.is_complete() {
        write_builder_generation(&cache_dir);
    } else if generation_stale {
        tracing::info!(
            remaining = sweep.len(),
            "preagg: builder generation sweep incomplete; the next cycle finishes it"
        );
    }

    let skipped_suffix = skip_suffix(skipped_no_key, skipped_no_datasource);
    // Named, not folded into `succeeded`: a run row reading "rebuilt 3 of 3"
    // for a rollup whose artifact was deleted is the run disagreeing with the
    // table the reader is looking at.
    let retracted_suffix = if retracted > 0 {
        format!(", {retracted} retracted as empty")
    } else {
        String::new()
    };
    let outcome = if failed == 0 {
        TaskOutcome::Done {
            answer: format!(
                "rebuilt {succeeded} of {total} rollups{retracted_suffix}{skipped_suffix}"
            ),
            metadata: None,
        }
    } else {
        TaskOutcome::Failed(format!(
            "{failed} of {total} rollups failed{retracted_suffix}{skipped_suffix}"
        ))
    };
    let _ = outcome_tx.send(outcome).await;
}

// ── Helpers (moved from preagg_worker.rs) ────────────────────────────────────

/// Build the human-readable "(N skipped: …)" suffix for a cycle outcome
/// message. Returns an empty string when nothing was skipped.
fn skip_suffix(skipped_no_key: usize, skipped_no_datasource: usize) -> String {
    let total = skipped_no_key + skipped_no_datasource;
    if total == 0 {
        return String::new();
    }
    let mut parts = Vec::new();
    if skipped_no_key > 0 {
        parts.push(format!("{skipped_no_key} no refresh_key"));
    }
    if skipped_no_datasource > 0 {
        parts.push(format!("{skipped_no_datasource} datasource not configured"));
    }
    format!(" ({total} skipped: {})", parts.join(", "))
}

/// Load every declared view, fleet-safe: the compile boundary first (works on
/// any node, including one with no working copy — a `serve`/`worker`-only node
/// draining this Custom task), the workspace's FS second (works only when this
/// node happens to have it checked out). Mirrors
/// `semantic::resolve_query_scan_source` + the same loader
/// (`oxy_airlayer_compat::load_layer_from_dir`) every other semantic-model
/// surface in this crate already resolves through — replaces this module's old
/// direct `**/*.view.yml` FS glob, which only ever worked on the ide node and
/// is exactly the limitation this rewrite exists to lift.
async fn load_views(
    workspace_manager: &oxy::adapters::workspace::manager::WorkspaceManager<
        oxy::config::WorkingCopy,
    >,
    database_override: Option<&str>,
) -> Result<Vec<(oxy_airlayer_compat::View, String)>, String> {
    let scan = crate::server::api::semantic::resolve_query_scan_source(workspace_manager)
        .await
        .map_err(|e| e.message())?;
    let scan_path = scan.scan_path.clone();
    let layer =
        tokio::task::spawn_blocking(move || oxy_airlayer_compat::load_layer_from_dir(&scan_path))
            .await
            .map_err(|e| format!("load_views task panicked: {e}"))?
            .map_err(|e| format!("failed to load semantic model: {e}"))?;

    Ok(layer
        .views
        .into_iter()
        .map(|view| {
            let db = database_override
                .map(|s| s.to_string())
                .or_else(|| view.datasource.clone())
                .unwrap_or_else(|| "default".to_string());
            (view, db)
        })
        .collect())
}

/// Every loaded view's rollups, index-aligned with `views`, or every refusal
/// in one message — the failure exists to name what to fix.
///
/// airlayer refuses a rollup naming a measure its view does not declare rather
/// than resolving it one measure short (#119). `load_views` does not validate,
/// so this is where the cycle meets that refusal — and it fails the cycle
/// rather than skipping the view, because the layer-wide engine every rollup is
/// planned against refuses the same layer regardless.
fn resolve_cycle_rollups(
    views: &[(oxy_airlayer_compat::View, String)],
) -> Result<Vec<Vec<oxy_airlayer_compat::preagg::RollupSpec>>, String> {
    let mut resolved = Vec::with_capacity(views.len());
    let mut refusals = Vec::new();
    for (view, _) in views {
        match oxy_airlayer_compat::preagg::resolve_rollups(view) {
            Ok(rollups) => resolved.push(rollups),
            Err(e) => refusals.push(e.to_string()),
        }
    }
    if refusals.is_empty() {
        Ok(resolved)
    } else {
        Err(refusals.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::{PreaggCycleRequest, RollupTarget, resolve_cycle_rollups};

    fn ws() -> uuid::Uuid {
        uuid::Uuid::nil()
    }

    // ── On-demand rebuild request semantics ───────────────────────────────

    #[test]
    fn workspace_id_is_required_a_stale_null_payload_fails_cleanly() {
        // Every already-queued task carried `null` (the old baked-workspace
        // default) or `{force, target}` with no `workspace_id` at all. Neither
        // has a valid interpretation any more — there is no single workspace
        // left to guess — so both must fail to deserialize rather than the
        // executor picking one silently.
        assert!(serde_json::from_value::<PreaggCycleRequest>(serde_json::Value::Null).is_err());
        assert!(
            serde_json::from_value::<PreaggCycleRequest>(serde_json::json!({ "force": true }))
                .is_err()
        );
    }

    #[test]
    fn a_well_formed_payload_round_trips() {
        let req = PreaggCycleRequest {
            workspace_id: ws(),
            force: false,
            target: None,
        };
        let round_tripped: PreaggCycleRequest =
            serde_json::from_value(serde_json::to_value(&req).unwrap()).unwrap();
        assert_eq!(round_tripped.workspace_id, ws());
        assert!(!round_tripped.force);
        assert!(round_tripped.target.is_none());
    }

    #[test]
    fn an_untargeted_request_covers_every_rollup() {
        let req = PreaggCycleRequest {
            workspace_id: ws(),
            force: true,
            target: None,
        };
        assert!(req.covers("orders", "orders_by_month"));
        assert!(req.covers("customers", "anything"));
    }

    #[test]
    fn a_targeted_request_covers_exactly_one_rollup() {
        let req = PreaggCycleRequest {
            workspace_id: ws(),
            force: true,
            target: Some(RollupTarget {
                view: "orders".into(),
                rollup: "orders_by_month".into(),
            }),
        };
        assert!(req.covers("orders", "orders_by_month"));
        // Same rollup name on another view is a different rollup.
        assert!(!req.covers("order_items", "orders_by_month"));
        assert!(!req.covers("orders", "orders_summary"));
    }

    // ── Rollup resolution ─────────────────────────────────────────────────

    fn loaded(yaml: &str) -> (oxy_airlayer_compat::View, String) {
        let view = oxy_airlayer_compat::parse_view_yaml(yaml).expect("fixture view parses");
        (view, "local".to_string())
    }

    const ORDERS: &str = r#"
name: orders
datasource: local
table: orders.csv
dimensions:
  - name: status
    type: string
    expr: status
measures:
  - name: total_orders
    type: count
pre_aggregations:
  - name: by_status
    dimensions: [status]
    measures: [total_orders]
"#;

    #[test]
    fn a_healthy_layer_resolves_index_aligned_with_its_views() {
        let resolved = resolve_cycle_rollups(&[loaded(ORDERS)]).expect("resolves");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0][0].name, "by_status");
    }

    /// Before airlayer #119, `total_orderz` was dropped at resolution and
    /// `by_status` resolved one measure short: the sweep and ledger prune ran
    /// against that shortened hash, and only the per-rollup build caught the
    /// typo, after its refresh-key probe. The cycle now fails up front, and
    /// the failure names what to fix.
    #[test]
    fn an_unresolvable_rollup_measure_fails_the_cycle_naming_it() {
        let typo = ORDERS.replace(
            "measures: [total_orders]",
            "measures: [total_orders, total_orderz]",
        );

        let err = resolve_cycle_rollups(&[loaded(&typo)])
            .expect_err("a rollup naming an undeclared measure must fail the cycle");

        assert!(err.contains("orders"), "missing view: {err}");
        assert!(err.contains("by_status"), "missing rollup: {err}");
        assert!(err.contains("total_orderz"), "missing measure: {err}");
    }

    /// Every refusal in one message: the failure exists to name what to fix,
    /// and reporting only the first costs a fix-and-rerun per typo.
    #[test]
    fn every_refused_rollup_is_named_not_just_the_first() {
        let first = ORDERS.replace(
            "measures: [total_orders]",
            "measures: [total_orders, total_orderz]",
        );
        let second = ORDERS.replace("name: orders", "name: returns").replace(
            "measures: [total_orders]",
            "measures: [total_orders, total_returnz]",
        );

        let err = resolve_cycle_rollups(&[loaded(&first), loaded(ORDERS), loaded(&second)])
            .expect_err("refused rollups must fail the cycle");

        assert!(err.contains("total_orderz"), "first refusal missing: {err}");
        assert!(
            err.contains("total_returnz"),
            "second refusal missing: {err}"
        );
    }
}
