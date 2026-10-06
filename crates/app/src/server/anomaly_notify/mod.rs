//! Insights delivery: after a monitor scan, post the events it newly found to
//! the Slack channel the workspace's `.monitor.yml` names under `notify:`.
//!
//! The scan decides nothing about delivery. Having persisted, it calls
//! [`enqueue_if_due`], which queues one `anomaly_notify` task when the file has
//! a `notify:` block and the ledger has an unannounced event. The task
//! ([`executor`]) finds the org's Slack, then runs
//! `oxy_metric_monitoring::notify::announce` — which claims, composes, posts
//! and records. A queued task rather than a call at the end of the scan
//! because it leaves the process and can be refused: it should have its own
//! run to read, and outlive the pod that scanned.
//!
//! **This posts to the tenant's own Slack, never to ops.** Severity is how far
//! a number moved, not whether anything is broken, which is why anomalies were
//! taken out of workspace health (`internal-docs/admin-surfaces.md`,
//! "anomalies do not vote"); none of this touches `OXY_OPS_SLACK_*`.
//!
//! What is due, and why "once", is `oxy_metric_monitoring::notify::ledger`.
//! The whole feature, including rollout order: `internal-docs/anomaly-monitoring.md`
//! ("Insights delivery").

use agentic_core::delegation::TaskSpec;
use agentic_runtime::crud::{TaskScope, enqueue_task, insert_run};
use chrono::Utc;
use oxy_metric_monitoring::NotifyConfig;
use oxy_metric_monitoring::notify::{Due, ledger};
use sea_orm::{DatabaseConnection, DbErr, TransactionTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod executor;

/// The `kind` of the delivery task, and the `source_type` of its run.
pub const ANOMALY_NOTIFY_KIND: &str = "anomaly_notify";

/// What the run is called in the Orchestrator Dashboard.
const RUN_NAME: &str = "Insights delivery";

/// The task's payload: the `notify:` block as the scan read it. Carried rather
/// than re-read so the task needs no workspace files — it runs on whichever
/// pod drains the queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnomalyNotifyPayload {
    pub workspace_id: Uuid,
    pub notify: NotifyConfig,
}

/// Queue a delivery task if this scan's file asks for one and there is
/// something to announce. Returns the run id when a task was queued.
///
/// Never fails the caller: by now the scan is persisted and correct, and an
/// event that goes unqueued here is still due at the next scan.
pub async fn enqueue_if_due(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    notify: Option<&NotifyConfig>,
) -> Option<String> {
    let notify = notify?;
    let due = Due {
        workspace_id,
        min_severity: notify.min_severity,
        now: Utc::now(),
    };
    // Asked first so a quiet scan leaves no run behind: a daily "nothing new"
    // row per workspace would bury the runs that did something.
    match ledger::anything_due(db, due).await {
        Ok(true) => {}
        Ok(false) => return None,
        Err(e) => {
            tracing::warn!(
                target: "metric_anomalies",
                %workspace_id,
                error = %e,
                "insights delivery: could not read the ledger; nothing queued this scan"
            );
            return None;
        }
    }
    let payload = AnomalyNotifyPayload {
        workspace_id,
        notify: notify.clone(),
    };
    match enqueue(db, &payload).await {
        Ok(run_id) => Some(run_id),
        Err(e) => {
            tracing::warn!(
                target: "metric_anomalies",
                %workspace_id,
                error = %e,
                "insights delivery: could not queue the task; the next scan retries"
            );
            None
        }
    }
}

/// Write the run row and its one task together. The task id is the run id —
/// what the global-run driver expects of a root task — and the run row goes
/// first because the queue row references it.
async fn enqueue(db: &DatabaseConnection, payload: &AnomalyNotifyPayload) -> Result<String, DbErr> {
    let run_id = Uuid::new_v4().to_string();
    let spec = TaskSpec::Custom {
        kind: ANOMALY_NOTIFY_KIND.to_string(),
        payload: serde_json::to_value(payload).map_err(|e| DbErr::Custom(e.to_string()))?,
    };
    let metadata = serde_json::json!({
        "trigger": "monitor_scan",
        "slack_channel": payload.notify.slack_channel,
    });
    let txn = db.begin().await?;
    insert_run(
        &txn,
        &run_id,
        RUN_NAME,
        None,
        ANOMALY_NOTIFY_KIND,
        Some(metadata),
        payload.workspace_id,
    )
    .await?;
    enqueue_task(&txn, &run_id, &run_id, None, &spec, None, TaskScope::Global).await?;
    txn.commit().await?;
    Ok(run_id)
}
