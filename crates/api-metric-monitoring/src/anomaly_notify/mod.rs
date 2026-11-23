//! Insights delivery's executor half: the `anomaly_notify` task a monitor scan
//! queues ([`oxy_metric_monitoring::notify::enqueue_if_due`]) and the two
//! destinations it announces to, the org's Slack channel and the people the
//! `.monitor.yml` names. Out of `oxy-app`'s `server::anomaly_notify`
//! (2026-11-21); the enqueue went down to `oxy_metric_monitoring::notify`.
//!
//! **This posts to the tenant's own Slack, never to ops.** Severity is how far
//! a number moved, not whether anything is broken, which is why anomalies were
//! taken out of workspace health (`internal-docs/admin-surfaces.md`,
//! "anomalies do not vote"); none of this touches `OXY_OPS_SLACK_*`.
//!
//! The kind rides [`tasks`] into `SurfaceSeams::tasks`. `oxy-app` requires it
//! by string (`ANOMALY_TASK_KINDS`), so a process whose seams lack it refuses
//! to start rather than queue deliveries nothing runs. The whole feature:
//! `internal-docs/anomaly-monitoring.md` ("Insights delivery").

use std::sync::Arc;

use oxy_app_core::seams::TaskSeam;
use oxy_metric_monitoring::notify::ANOMALY_NOTIFY_KIND;

pub mod executor;
pub mod people;

/// The task kind this surface runs, for `SurfaceSeams::tasks`: insights
/// delivery, queued after a monitor scan whose `.monitor.yml` has a `notify:`
/// block. Nothing can queue one before every pod parses that block — an older
/// pod refuses the file outright — so the kind and its enqueue ship together;
/// a pod without it fails the task (unknown kind) and the events stay due for
/// the next scan.
pub fn tasks() -> Vec<TaskSeam> {
    vec![TaskSeam {
        kind: ANOMALY_NOTIFY_KIND,
        build: |deps| {
            Arc::new(executor::AnomalyNotifyExecutor {
                db: deps.db.clone(),
            })
        },
    }]
}

#[cfg(test)]
mod tests {
    /// The kind string is the persisted task-queue contract, and `oxy-app`
    /// requires it by string (`ANOMALY_TASK_KINDS`), since it cannot name this
    /// crate's constant.
    #[test]
    fn the_task_kind_is_the_one_oxy_app_requires() {
        let kinds: Vec<&str> = super::tasks().iter().map(|t| t.kind).collect();
        assert_eq!(kinds, oxy_app_core::seams::ANOMALY_TASK_KINDS);
    }
}
