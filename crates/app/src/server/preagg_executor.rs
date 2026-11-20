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

use std::sync::{Arc, RwLock};

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use agentic_semantic::refresh_key_cache::RefreshKeyCache;
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::preagg_workspace::{build_workspace_manager, manifest_write_lock_for};
use crate::agentic_wiring::OxyProjectContext;
use oxy_preagg::cycle::{CycleWorkspace, run_preagg_task};
use oxy_preagg::worker_config::PreaggWorkerConfig;

/// The cycle's request types, `oxy_preagg::cycle` since it moved down; named
/// here as before so callers did not change.
pub use oxy_preagg::cycle::{PreaggCycleRequest, RollupTarget};

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
    // Its own directory databases: every rollup rebuild writes first.
    let ctx = OxyProjectContext::new(workspace_manager)
        .with_preagg_renewal_threshold_secs(renewal_threshold.as_secs())
        .with_own_directory_databases();

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

// ── The cycle's workspace ─────────────────────────────────────────────────────

#[async_trait]
impl CycleWorkspace for OxyProjectContext {
    fn is_database_configured(&self, name: &str) -> bool {
        OxyProjectContext::is_database_configured(self, name)
    }

    /// Load every declared view, fleet-safe: the compile boundary first (works on
    /// any node, including one with no working copy — a `serve`/`worker`-only node
    /// draining this Custom task), the workspace's FS second (works only when this
    /// node happens to have it checked out). Mirrors
    /// `scan_source::resolve_query_scan_source` + the same loader
    /// (`ScanDir::load_layer`) every other semantic-model
    /// surface in this crate already resolves through — replaces this module's old
    /// direct `**/*.view.yml` FS glob, which only ever worked on the ide node and
    /// is exactly the limitation this rewrite exists to lift.
    async fn load_views(
        &self,
        database_override: Option<&str>,
    ) -> Result<Vec<(oxy_airlayer_compat::View, String)>, String> {
        let scan =
            crate::agentic_wiring::scan_source::resolve_query_scan_source(self.workspace_manager())
                .await
                .map_err(|e| e.message())?;
        let scan = scan.scan.clone();
        let layer = tokio::task::spawn_blocking(move || scan.load_layer())
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
}
