//! The Airway change check (phase 2 P1): when a previewed branch compiles, is
//! each `.airway.yml` it changed safe to merge onto the live tables?
//!
//! Airway's schema migration is additive only, so a renamed pipeline, a moved
//! destination, a changed write disposition or key, a renamed table or a
//! retyped column needs an explicit **Reset schema** after merge. The judgement
//! is `agentic_airway::schema_compat::classify` (pure); this module feeds it:
//!
//! 1. **What changed** ([`changes`]) — the staging revision's compiled
//!    `airway_pipelines` rows against the promoted revision's, by file path.
//! 2. **What each side advertises** ([`evaluate`]) — every changed definition
//!    parsed and its source connector built offline with placeholder
//!    credentials, off the async threads.
//! 3. **What production has** ([`live`]) — Airway's stored schema for the live
//!    pipeline, and, for an edited pipeline landing in the workspace's managed
//!    Airhouse, the live columns read through a `SystemPurpose::Preview` Reader.
//!    That credential is minted only when such a pipeline changed.
//!
//! Nothing here fails a check for being unable to look: a definition that will
//! not parse, a connector that will not build and an Airhouse that cannot be
//! reached are each an `Unevaluated` warning on that pipeline. The run fails
//! only on a database error, and is retried.
//!
//! The check also lists the automations the branch changed ([`transforms`],
//! phase 2 P2): each pure-Airhouse transform is `auto` and gets a queued
//! `transform_build` run under the check ([`builds`]), which
//! `previews::compare` compares with live once it is done; everything else is
//! `manual`, with the reason.
//!
//! Durable queue work: [`enqueue::ensure_enqueued`] seeds one
//! `workspace_preview_runs` row per revision (kind `analyze`) with its
//! `agentic_runs` row and a `TaskSpec::Custom { kind: "preview_analyze" }`, and
//! [`PreviewAnalyzeExecutor`] runs it on the worker fleet. The report is the
//! run's `TaskOutcome::Done` metadata — ids, names and verdicts, never rows —
//! which `previews::checks` serves.

mod builds;
mod changes;
mod enqueue;
mod evaluate;
mod live;
mod report;
#[cfg(test)]
mod requeue_tests;
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod transform_fixtures;
#[cfg(test)]
mod transform_tests;
mod transform_vars;
mod transforms;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use futures::future::FutureExt;
use sea_orm::{DatabaseConnection, DbErr, EntityTrait};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use builds::queue_builds;
pub use changes::{Change, ChangedPipeline, changed_pipelines};
pub use enqueue::{enqueue_after_staging_compile, ensure_enqueued};
/// The connector a run would build, with placeholder credentials; an Airway
/// sample counts a source's resources with it (`previews::sample`).
pub(crate) use evaluate::offline_connector;
pub use live::{AirhouseLiveTables, LiveTables};
pub use report::{AnalyzeReport, FindingView, PipelineReport};
pub use transforms::{Build, TransformReport, classify, database_types, detect};

use agentic_airway::DestinationSpec;
use evaluate::{LiveRead, Prepared};

use crate::agentic_wiring::preview_airhouse::{PreviewAirhousePorts, WorkspaceAirhouse};

/// The `TaskSpec::Custom` kind, and the analyze run's `source_type`.
pub const PREVIEW_ANALYZE_KIND: &str = "preview_analyze";

/// Check the staging revision `run` names against the workspace's promoted
/// revision. `live` answers the live-column reads.
pub async fn analyze(
    db: &DatabaseConnection,
    run: &entity::workspace_preview_runs::Model,
    live: &dyn LiveTables,
) -> Result<AnalyzeReport, DbErr> {
    let promoted = crate::server::preagg_promote::promoted_revision(db, run.workspace_id).await?;
    let changed = changed_pipelines(db, run.revision_id, promoted).await?;
    let prepared = evaluate::prepare_all(changed.clone()).await;
    let managed = match promoted {
        Some(rev) if prepared.iter().any(|p| p.live_spec().is_some()) => {
            live::managed_airhouse_databases(db, rev).await?
        }
        _ => HashSet::new(),
    };
    let mut reads: HashMap<String, LiveRead> = HashMap::new();
    let mut pipelines = Vec::with_capacity(changed.len());
    for (c, p) in changed.iter().zip(&prepared) {
        let stored = match (c.change(), p.live_spec()) {
            (Change::Modified, Some(spec)) => {
                live::stored_schema(db, run.workspace_id, &spec.name).await?
            }
            _ => None,
        };
        let read = match (stored.is_some(), airhouse_dataset(p, &managed)) {
            (true, Some(dataset)) => live_read(&mut reads, live, &dataset).await,
            _ => &NOT_READ,
        };
        let check = evaluate::check(p, stored.as_ref(), read);
        pipelines.push(PipelineReport::new(c, check));
    }
    Ok(AnalyzeReport {
        revision_id: run.revision_id,
        promoted_revision_id: promoted,
        pipelines,
        transforms: changed_transforms(db, run.revision_id, promoted).await?,
    })
}

/// The automations the branch changed, classified against the staging
/// revision's databases — its compiled `databases` row, read directly as
/// [`live::managed_airhouse_databases`] reads the promoted one: a system task
/// checking one named revision has no manager to ask.
///
/// A ready revision always has that row (the compile writes it), so a missing
/// one is the platform's fault, not the branch's: the check fails and is
/// retried, rather than calling every database the branch names unknown.
pub(super) async fn changed_transforms(
    db: &DatabaseConnection,
    staging: uuid::Uuid,
    promoted: Option<uuid::Uuid>,
) -> Result<Vec<TransformReport>, DbErr> {
    let databases = entity::workspace_compiled_configs::Entity::find_by_id(staging)
        .one(db)
        .await?
        .map(|row| row.databases)
        .ok_or_else(|| {
            DbErr::Custom(format!(
                "staging revision {staging} has no compiled config; the check will retry"
            ))
        })?;
    detect(db, staging, promoted, &database_types(&databases)).await
}

static NOT_READ: LiveRead = LiveRead::NotRead;

/// The live dataset of an edited pipeline whose destination is the
/// workspace's managed Airhouse. Drift is only judged there.
fn airhouse_dataset(prepared: &Prepared, managed: &HashSet<String>) -> Option<String> {
    match &prepared.live_spec()?.destination {
        DestinationSpec::Reference(r) if managed.contains(&r.database) => {
            Some(r.dataset_name.clone())
        }
        _ => None,
    }
}

/// One read per dataset, however many pipelines land in it.
async fn live_read<'a>(
    reads: &'a mut HashMap<String, LiveRead>,
    live: &dyn LiveTables,
    dataset: &str,
) -> &'a LiveRead {
    if !reads.contains_key(dataset) {
        let read = match live.columns(dataset).await {
            Ok(columns) => LiveRead::Columns(columns),
            Err(reason) => LiveRead::Unavailable(reason),
        };
        reads.insert(dataset.to_string(), read);
    }
    &reads[dataset]
}

/// Runs a `preview_analyze` task. Registered in
/// `router::recovery::build_custom_task_registry`. `airhouse` answers
/// whether this deployment can take a preview's writes: builds are queued
/// only where it can.
pub struct PreviewAnalyzeExecutor {
    pub db: DatabaseConnection,
    pub airhouse: Arc<dyn PreviewAirhousePorts>,
}

impl PreviewAnalyzeExecutor {
    /// On the workspace's own Airhouse.
    pub fn airhouse(db: DatabaseConnection) -> Self {
        Self {
            db,
            airhouse: WorkspaceAirhouse::shared(),
        }
    }
}

#[async_trait]
impl TaskExecutor for PreviewAnalyzeExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let run_id = preview_run_id(&assignment.spec)?;
        let (event_tx, event_rx) = mpsc::channel(16);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let (db, airhouse) = (self.db.clone(), Arc::clone(&self.airhouse));
        tokio::spawn(async move {
            let _ = event_tx
                .send((
                    "preview_analyze_started".into(),
                    serde_json::json!({ "preview_run_id": run_id }),
                ))
                .await;
            // A panic still owes the runtime a terminal outcome, or the run
            // sits `running` forever.
            let task = run_task(&db, airhouse.as_ref(), &run_id);
            let outcome = match std::panic::AssertUnwindSafe(task).catch_unwind().await {
                Ok(outcome) => outcome,
                Err(_) => {
                    finish_after_error(&db, &run_id).await;
                    TaskOutcome::Failed("the Airway change check panicked".into())
                }
            };
            let _ = outcome_tx.send(outcome).await;
        });
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel: CancellationToken::new(),
            answers: None,
        })
    }
}

fn preview_run_id(spec: &TaskSpec) -> Result<String, String> {
    let TaskSpec::Custom { kind, payload } = spec else {
        return Err(format!(
            "unexpected spec for PreviewAnalyzeExecutor: {spec:?}"
        ));
    };
    if kind != PREVIEW_ANALYZE_KIND {
        return Err(format!("unknown preview kind: {kind}"));
    }
    payload
        .get("preview_run_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "preview_analyze payload missing string preview_run_id".to_string())
}

async fn run_task(
    db: &DatabaseConnection,
    airhouse: &dyn PreviewAirhousePorts,
    run_id: &str,
) -> TaskOutcome {
    let result = async {
        let run = entity::workspace_preview_runs::Entity::find_by_id(run_id.to_string())
            .one(db)
            .await?
            .filter(|r| r.kind == "analyze")
            .ok_or_else(|| DbErr::RecordNotFound(format!("no analyze preview run {run_id}")))?;
        enqueue::mark_running(db, run_id).await?;
        let mut report = analyze(db, &run, &AirhouseLiveTables::new(run.workspace_id)).await?;
        queue_builds(db, airhouse, &run, &mut report.transforms).await?;
        enqueue::mark_finished(db, run_id).await?;
        Ok::<_, DbErr>(report)
    }
    .await;
    match result {
        Ok(report) => TaskOutcome::Done {
            answer: report.answer(),
            metadata: serde_json::to_value(&report).ok(),
        },
        Err(e) => {
            tracing::warn!(%run_id, error = %e, "previews: Airway change check failed");
            finish_after_error(db, run_id).await;
            TaskOutcome::Failed(format!("Airway change check failed: {e}"))
        }
    }
}

/// A failed attempt still ends its row: `finished`, not `running` forever when
/// no retry follows. A retry marks it `running` again. Best-effort: the
/// database may be what failed.
async fn finish_after_error(db: &DatabaseConnection, run_id: &str) {
    if let Err(e) = enqueue::mark_finished(db, run_id).await {
        tracing::warn!(%run_id, error = %e, "previews: could not mark a failed check finished");
    }
}
