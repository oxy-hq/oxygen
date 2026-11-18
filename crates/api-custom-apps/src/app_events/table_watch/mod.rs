//! Table changes as events (app events design §7): a function's `on: { table
//! }` runs once per DuckLake snapshot that inserted into or deleted from the
//! table.
//!
//! | Part | What |
//! | --- | --- |
//! | [`declare`] | what a build may declare to watch, checked at publish |
//! | [`cursors`] | one cursor per (workspace, environment, table): the last snapshot delivered, the table's columns, the state |
//! | [`reader`] | `table_changes`, the snapshot catalog and the columns, read as a platform Reader |
//! | [`track`] | one pass over an environment's cursors: read, publish, move |
//! | [`publish`] | one event: the body to the org's store, then the event, its deliveries and the cursor in one platform transaction |
//! | [`breaker`] | pauses a subscription delivering above a sustained rate |
//! | [`loop_guard`] | the host's refusal of a write to the table that woke the writer |
//! | [`resync`] | `oxyc events resync`: the cursor to now, states cleared, an optional backfill |
//! | [`migrations`] | the ship gate's check that an `airhouseMigrations` change keeps a watched table's columns |
//!
//! **Waking.** A pass is a queued task, `app_table_watch`, one per (org,
//! environment), run by whichever worker takes it. Two things queue one:
//!
//! - **a nudge**: after the host writes through `ctx.airhouse` or
//!   `ctx.warehouse`, [`nudge`] queues a pass for every environment of the
//!   writer's workspace that has a cursor (a pass already waiting absorbs it);
//! - **the sweep**, a fleet job every [`SWEEP_INTERVAL`] (30 s): queues a pass
//!   for every (org, environment) that has a cursor, in an org with
//!   `app_events` on. It is what catches the writers the host never sees: a
//!   pipeline, a rollup, a person.
//!
//! A pass that left changed snapshots behind queues the next one itself.
//!
//! **Paging.** A cursor that needs a person (`broken`, `behind_horizon`) or a
//! subscription the breaker paused logs one of `app_events_table_broken`,
//! `app_events_table_behind_horizon`, `app_events_subscription_paused` at
//! `error` in production, the lines the production pager matches the way it
//! matches `app_events_outbox_stale`; elsewhere at `warn`. Logged when the
//! state is entered, and again each pass only at `warn`.
//!
//! **Inert while the org's `app_events` flag is off.**

pub mod breaker;
pub mod cursors;
pub mod declare;
pub mod loop_guard;
pub mod migrations;
pub mod publish;
pub mod reader;
pub mod resync;
pub mod track;

use std::time::Duration;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::crud::{TaskScope, enqueue_task, insert_run};
use agentic_runtime::worker::{ExecutingTask, TaskExecutor};
use async_trait::async_trait;
use chrono::Utc;
use futures::FutureExt;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// The `TaskSpec::Custom` kind, and the run's `source_type`.
pub const TASK_KIND: &str = "app_table_watch";
/// The `fleet_job_claims` row the sweep claims each interval.
pub const SWEEP_JOB: &str = "custom_app_table_watch_sweep";
/// How often the sweep queues a pass for every environment with a cursor.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// What needs a person, for [`page`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attention {
    /// A column the cursor relied on was removed, renamed or retyped, or the
    /// table is gone.
    Broken,
    /// The snapshots between the cursor and now were expired.
    BehindHorizon,
    /// The breaker paused a subscription.
    Paused,
}

/// Log `what` for `table` of `org_id`: `error` in production, `warn` anywhere
/// else, and at `warn` only when `repeat` (the state was already held).
pub fn page(
    environment: &str,
    org_id: Uuid,
    table: &str,
    what: Attention,
    detail: &str,
    repeat: bool,
) {
    let production = environment == "production" && !repeat;
    macro_rules! log {
        ($message:literal) => {
            if production {
                tracing::error!(%org_id, environment, table, detail, $message)
            } else {
                tracing::warn!(%org_id, environment, table, detail, $message)
            }
        };
    }
    match what {
        Attention::Broken => log!("app_events_table_broken"),
        Attention::BehindHorizon => log!("app_events_table_behind_horizon"),
        Attention::Paused => log!("app_events_subscription_paused"),
    }
}

/// The payload of one pass: whose cursors, and which environment's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackPayload {
    pub org_id: Uuid,
    pub environment: String,
}

impl TrackPayload {
    fn spec(&self) -> Result<TaskSpec, DbErr> {
        Ok(TaskSpec::Custom {
            kind: TASK_KIND.to_string(),
            payload: serde_json::to_value(self).map_err(|e| DbErr::Custom(e.to_string()))?,
        })
    }

    fn from_spec(spec: &TaskSpec) -> Result<Self, String> {
        match spec {
            TaskSpec::Custom { kind, payload } if kind == TASK_KIND => {
                serde_json::from_value(payload.clone())
                    .map_err(|e| format!("bad table watch payload: {e}"))
            }
            other => Err(format!("not a table watch task: {other:?}")),
        }
    }
}

/// Queue a pass over `org_id`'s `environment` cursors, unless one is already
/// waiting to run. `false` when the org has no app to queue it under.
pub async fn enqueue_track(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
) -> Result<bool, DbErr> {
    let prefix = format!("{TASK_KIND}:{org_id}:{environment}:");
    let waiting = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 AS waiting FROM agentic_task_queue \
              WHERE task_id LIKE $1 AND queue_status = 'queued' LIMIT 1",
            [format!("{prefix}%").into()],
        ))
        .await?;
    if waiting.is_some() {
        return Ok(true);
    }
    let Some(workspace_id) = super::relay::run_workspace(db, org_id).await? else {
        return Ok(false);
    };
    let payload = TrackPayload {
        org_id,
        environment: environment.to_string(),
    };
    let spec = payload.spec()?;
    let run_id = format!("{prefix}{}", Utc::now().timestamp_micros());
    let txn = db.begin().await?;
    insert_run(
        &txn,
        &run_id,
        "Track table changes",
        None,
        TASK_KIND,
        Some(json!({ "org_id": org_id, "environment": environment })),
        workspace_id,
    )
    .await?;
    enqueue_task(&txn, &run_id, &run_id, None, &spec, None, TaskScope::Global).await?;
    txn.commit().await?;
    Ok(true)
}

/// After a host write in `workspace_id`'s Airhouse: queue a pass for each
/// environment of the workspace that watches a table. A no-op while the org's
/// flag is off or nothing is watched. A failure is logged and left to the
/// sweep.
pub async fn nudge(db: &DatabaseConnection, org_id: Uuid, workspace_id: Uuid) {
    if !super::enabled_for(org_id) {
        return;
    }
    match cursors::environments_of(db, workspace_id).await {
        Ok(environments) => {
            for (org, environment) in environments {
                if let Err(e) = enqueue_track(db, org, &environment).await {
                    tracing::warn!(error = %e, "app events: a table pass could not be queued");
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "app events: a table nudge could not read the cursors")
        }
    }
}

/// What one [`sweep_once`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub queued: usize,
}

/// One sweep: queue a pass for every (org, environment) with a cursor, in an
/// org with `app_events` on.
pub async fn sweep_once(db: &DatabaseConnection) -> Result<SweepReport, DbErr> {
    let mut report = SweepReport::default();
    for (org_id, environment) in cursors::tracked(db).await? {
        if !super::enabled_for(org_id) {
            continue;
        }
        match enqueue_track(db, org_id, &environment).await {
            Ok(true) => report.queued += 1,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(%org_id, environment, error = %e, "app events: a table pass could not be queued")
            }
        }
    }
    Ok(report)
}

/// Run [`sweep_once`] (and the event log mirror's pass) once per
/// [`SWEEP_INTERVAL`] across the fleet until shutdown. Every worker starts it;
/// the [`fleet_claim`](oxy_app_core::fleet_claim) row lets one of them run per
/// interval.
pub fn spawn_sweep(db: DatabaseConnection, shutdown: CancellationToken) {
    let pass_db = db.clone();
    tokio::spawn(oxy_app_core::fleet_claim::run_claimed(
        db,
        SWEEP_JOB,
        SWEEP_INTERVAL,
        shutdown.cancelled_owned(),
        move || {
            let db = pass_db.clone();
            async move {
                if let Err(e) = sweep_once(&db).await {
                    tracing::warn!(error = %e, "app events table sweep failed");
                }
                super::mirror::mirror_pass(&db).await;
            }
        },
    ));
}

/// Runs a queued pass on the worker fleet.
pub struct TrackExecutor {
    pub db: DatabaseConnection,
}

#[async_trait]
impl TaskExecutor for TrackExecutor {
    async fn execute(&self, assignment: TaskAssignment) -> Result<ExecutingTask, String> {
        let payload = TrackPayload::from_spec(&assignment.spec)?;
        let (event_tx, event_rx) = mpsc::channel(1);
        let (outcome_tx, outcome_rx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let db = self.db.clone();
        let stop = cancel.clone();
        tokio::spawn(async move {
            // A pass dropped mid-way rolls back its platform transaction: the
            // cursor stays where it was.
            let outcome = tokio::select! {
                outcome = run_guarded(&db, &payload) => outcome,
                _ = stop.cancelled() => TaskOutcome::Failed("cancelled".to_string()),
            };
            let _ = outcome_tx.send(outcome).await;
            drop(event_tx);
        });
        Ok(ExecutingTask {
            events: event_rx,
            outcomes: outcome_rx,
            cancel,
            answers: None,
        })
    }
}

/// The task's outcome; a panic still ends in `Failed`, or the run would stay
/// `running` with no terminal event.
async fn run_guarded(db: &DatabaseConnection, payload: &TrackPayload) -> TaskOutcome {
    let TrackPayload {
        org_id,
        environment,
    } = payload;
    let pass = std::panic::AssertUnwindSafe(track::track_once(db, *org_id, environment))
        .catch_unwind()
        .await;
    match pass {
        Ok(Ok(stats)) => {
            if stats.more
                && let Err(e) = enqueue_track(db, *org_id, environment).await
            {
                tracing::warn!(%org_id, environment, error = %e, "could not queue the next table pass");
            }
            TaskOutcome::Done {
                answer: format!(
                    "published {} table events with {} deliveries",
                    stats.published, stats.deliveries
                ),
                metadata: Some(json!({ "org_id": org_id, "environment": environment })),
            }
        }
        Ok(Err(e)) => {
            tracing::warn!(%org_id, environment, error = %e, "{TASK_KIND} failed");
            TaskOutcome::Failed(e)
        }
        Err(_) => {
            tracing::error!(%org_id, environment, "{TASK_KIND} panicked");
            TaskOutcome::Failed("the table watch pass panicked".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_payload_round_trips_through_its_spec() {
        let payload = TrackPayload {
            org_id: Uuid::from_u128(7),
            environment: "staging".into(),
        };
        let spec = payload.spec().unwrap();
        assert_eq!(TrackPayload::from_spec(&spec).unwrap(), payload);
        assert!(
            TrackPayload::from_spec(&TaskSpec::Custom {
                kind: "other".into(),
                payload: json!({})
            })
            .is_err()
        );
    }
}
