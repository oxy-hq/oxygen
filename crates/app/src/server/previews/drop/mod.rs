//! Dropping a preview's expired Airhouse schemas: the `preview_schema_drop`
//! task the TTL sweep (`previews::maintenance`) queues, run on the worker fleet.
//!
//! A schema is touched only when all of these hold, checked here and not
//! trusted from the payload:
//! 1. its registry row exists, is neither dropped nor refused, and is still
//!    claimed by **this** drop run — read under a row lock, so a preview that
//!    writes again (and re-arms or touches the row) either waits for the drop
//!    to finish or makes the drop skip the schema;
//! 2. the name is well formed for the key's namespace
//!    (`^preview_[a-z0-9_]{1,24}_[0-9a-f]{6}__[a-z0-9_]+$`, and
//!    `PreviewNamespace::owns_schema`);
//! 3. the preview created it (`schema_created_at`): a row whose schema the
//!    preview never created is closed without any DDL.
//!
//! Then only the relations the preview **recorded** in its shadow map are
//! dropped (views first). When nothing else is left, the schema is dropped,
//! without `CASCADE`. When the schema holds anything the preview did not
//! record, it is left in place, its row refused (never claimed again) and a
//! warning logged: something other than the preview wrote there.
//!
//! Postgres cleanup follows in the same transaction ([`claimed`]). A drop that
//! fails leaves its rows undropped; its run ends failed, and the next sweep
//! releases the claim and queues it again, up to
//! `maintenance::MAX_DROP_ATTEMPTS` times.

mod claimed;
mod pass;

use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use airhouse::preview_sql::PreviewNamespace;
use async_trait::async_trait;
use futures::future::FutureExt;
use sea_orm::{DatabaseConnection, DbErr};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub use claimed::drop_claimed;
pub use pass::{ClaimedSchema, drop_listed};

use super::ddl::OpenSchemaDropper;

/// The `TaskSpec::Custom` kind, and the drop run's `source_type`.
pub const PREVIEW_SCHEMA_DROP_KIND: &str = "preview_schema_drop";

/// What the sweep queues: the schemas it claimed for one key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DropPayload {
    pub workspace_id: Uuid,
    pub preview_key: String,
    pub schemas: Vec<String>,
}

/// What a drop did, as the run's outcome metadata: names and counts only.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct DropReport {
    /// Each dropped schema and how many relations it held.
    pub dropped: Vec<(String, usize)>,
    /// Claimed rows whose schema the preview never created: closed, no DDL.
    pub never_created: Vec<String>,
    /// Schemas left in place: how many of the preview's relations were
    /// dropped, and the relations it did not record. Refused, never retried.
    pub orphaned: Vec<(String, usize, Vec<String>)>,
    /// Names asked for but not touched, and why: no claim by this run (the
    /// preview wrote again after the sweep claimed it, which is normal), or not
    /// a schema of this preview. Never retried.
    pub refused: Vec<(String, String)>,
    /// Names whose drop failed; the run fails and the sweep retries them.
    pub failed: Vec<(String, String)>,
}

impl DropReport {
    pub fn answer(&self) -> String {
        format!(
            "Dropped {} preview schema(s); {} never created, {} left in place, {} refused, {} failed",
            self.dropped.len(),
            self.never_created.len(),
            self.orphaned.len(),
            self.refused.len(),
            self.failed.len()
        )
    }
}

/// Runs a `preview_schema_drop` task. Registered in
/// `router::recovery::build_custom_task_registry` with Airhouse droppers.
pub struct PreviewSchemaDropExecutor {
    pub db: DatabaseConnection,
    pub droppers: Arc<dyn OpenSchemaDropper>,
}

impl PreviewSchemaDropExecutor {
    /// Production: drops through a system Writer on the workspace's Airhouse.
    pub fn airhouse(db: DatabaseConnection) -> Self {
        Self {
            db,
            droppers: Arc::new(super::ddl_airhouse::AirhouseDroppers),
        }
    }
}

#[async_trait]
impl TaskExecutor for PreviewSchemaDropExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let payload = drop_payload(&assignment.spec)?;
        let ns = PreviewNamespace::from_key(&payload.preview_key).map_err(|e| e.to_string())?;
        let dropper = self.droppers.open(payload.workspace_id, &ns);
        let (event_tx, event_rx) = mpsc::channel(16);
        let (outcome_tx, outcome_rx) = mpsc::channel(4);
        let db = self.db.clone();
        let run_id = assignment.run_id;
        tokio::spawn(async move {
            let _ = event_tx
                .send((
                    "preview_schema_drop_started".into(),
                    serde_json::json!({ "preview_key": payload.preview_key }),
                ))
                .await;
            // A panic still owes the runtime a terminal outcome.
            let run = drop_claimed(&db, dropper.as_ref(), &run_id, &payload);
            let outcome = match std::panic::AssertUnwindSafe(run).catch_unwind().await {
                Ok(result) => outcome_of(&run_id, &payload, result),
                Err(_) => TaskOutcome::Failed("the preview schema drop panicked".into()),
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

fn drop_payload(spec: &TaskSpec) -> Result<DropPayload, String> {
    match spec {
        TaskSpec::Custom { kind, payload } if kind == PREVIEW_SCHEMA_DROP_KIND => {
            serde_json::from_value(payload.clone())
                .map_err(|e| format!("preview_schema_drop payload: {e}"))
        }
        other => Err(format!(
            "unexpected spec for PreviewSchemaDropExecutor: {other:?}"
        )),
    }
}

fn outcome_of(
    run_id: &str,
    payload: &DropPayload,
    result: Result<DropReport, DbErr>,
) -> TaskOutcome {
    let key = &payload.preview_key;
    let report = match result {
        Ok(report) => report,
        Err(e) => {
            tracing::warn!(%run_id, preview_key = %key, error = %e,
                "previews: preview schema drop failed; the next sweep retries");
            return TaskOutcome::Failed(format!("preview schema drop failed: {e}"));
        }
    };
    if !report.refused.is_empty() {
        tracing::info!(%run_id, preview_key = %key, refused = ?report.refused,
            "previews: schema drop skipped names it no longer holds a claim on");
    }
    if !report.orphaned.is_empty() {
        tracing::warn!(%run_id, preview_key = %key, orphaned = ?report.orphaned,
            "previews: preview schema holds relations the preview did not create; \
             dropped the preview's own and left the schema in place");
    }
    if !report.failed.is_empty() {
        tracing::warn!(%run_id, preview_key = %key, failed = ?report.failed,
            "previews: some preview schemas could not be dropped; the next sweep retries");
        return TaskOutcome::Failed(report.answer());
    }
    tracing::info!(%run_id, preview_key = %key, dropped = report.dropped.len(),
        "previews: dropped expired preview schemas");
    TaskOutcome::Done {
        answer: report.answer(),
        metadata: serde_json::to_value(&report).ok(),
    }
}

#[cfg(test)]
mod tests;
