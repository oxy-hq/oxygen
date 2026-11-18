//! Queuing a listener's task for a delivery, as the relay does
//! (`relay::deliver`): an `app_function` task, trigger `event`, the delivery
//! on the task, the envelope with `data: null` (the delivery reads the body
//! itself when it runs), the listener's retry policy.
//!
//! **Spread out**, at [`PER_SECOND`] tasks a second per listener (§6.8): the
//! n-th task one call queues for a listener is available n / [`PER_SECOND`]
//! seconds after the first, through `available_at`, so a listener just fixed
//! is not flooded. No sleep: the queue holds each task until its time.

use std::collections::HashMap;

use agentic_core::delegation::TaskPolicy;
use agentic_pipeline::app_function_task::AppFunctionTask;
use agentic_pipeline::scheduler::{ScheduleError, enqueue_app_function_task_at};
use chrono::{DateTime, Duration, Utc};
use entity::{app_event_deliveries as deliveries, apps};
use sea_orm::{DatabaseConnection, DatabaseTransaction, DbErr, EntityTrait};
use serde_json::Value;
use uuid::Uuid;

use crate::app_events::envelope::Records;
use crate::app_events::relay::listener_task_policy;
use crate::app_events::subscriptions::listeners;

/// Tasks a second a call queues for one listener.
pub(super) const PER_SECOND: i64 = 10;

/// When each task a call queues may run.
pub(super) struct Spread {
    start: DateTime<Utc>,
    queued: HashMap<(Uuid, String), i64>,
}

impl Spread {
    pub(super) fn new(start: DateTime<Utc>) -> Self {
        Self {
            start,
            queued: HashMap::new(),
        }
    }

    /// When the next task for `function` of `app_id` may run.
    pub(super) fn next(&mut self, app_id: Uuid, function: &str) -> DateTime<Utc> {
        let n = self
            .queued
            .entry((app_id, function.to_string()))
            .or_insert(0);
        let at = self.start + Duration::milliseconds(*n * 1_000 / PER_SECOND);
        *n += 1;
        at
    }
}

/// What a call reads once per listener: its policy and its app's workspace.
pub(super) struct Queuer<'a> {
    db: &'a DatabaseConnection,
    org_id: Uuid,
    policies: HashMap<(String, String), HashMap<(Uuid, String), Value>>,
    workspaces: HashMap<Uuid, Option<Uuid>>,
    pub(super) spread: Spread,
}

impl<'a> Queuer<'a> {
    pub(super) fn new(db: &'a DatabaseConnection, org_id: Uuid) -> Self {
        Self {
            db,
            org_id,
            policies: HashMap::new(),
            workspaces: HashMap::new(),
            spread: Spread::new(Utc::now()),
        }
    }

    /// Queue `delivery`'s task in `txn`, the event of type `event_type`; its
    /// run id. `None` when its event, its listener or its publisher is gone
    /// (the delivery goes with them).
    pub(super) async fn queue(
        &mut self,
        txn: &DatabaseTransaction,
        delivery: &deliveries::Model,
        event_type: &str,
    ) -> Result<Option<String>, DbErr> {
        let Some(workspace_id) = self.workspace(delivery.app_id).await? else {
            return Ok(None);
        };
        let Some(records) = Records::read(txn, delivery).await? else {
            return Ok(None);
        };
        let policy = self.policy(delivery, event_type).await?;
        let mut task = AppFunctionTask::new(delivery.app_id.to_string(), &delivery.function_name);
        task.trigger = Some("event".to_string());
        let envelope = records.envelope(Value::Null);
        task.input =
            Some(serde_json::to_value(&envelope).map_err(|e| DbErr::Custom(e.to_string()))?);
        task.environment =
            (delivery.environment != "production").then(|| delivery.environment.clone());
        task.event_delivery_id = Some(delivery.id.to_string());
        let available_at = self.spread.next(delivery.app_id, &delivery.function_name);
        enqueue_app_function_task_at(
            txn,
            workspace_id,
            Some(policy),
            task,
            available_at.fixed_offset(),
        )
        .await
        .map(Some)
        .map_err(|e| match e {
            ScheduleError::Db(e) => e,
            other => DbErr::Custom(other.to_string()),
        })
    }

    /// The listener's retry policy, from the function's manifest in the build
    /// its subscription follows; the default policy when it no longer listens
    /// (its run then finds that and kills the delivery).
    async fn policy(
        &mut self,
        d: &deliveries::Model,
        event_type: &str,
    ) -> Result<TaskPolicy, DbErr> {
        let key = (d.environment.clone(), event_type.to_string());
        if !self.policies.contains_key(&key) {
            let found = listeners(self.db, self.org_id, &d.environment, event_type)
                .await?
                .into_iter()
                .map(|l| ((l.app_id, l.function_name), l.function_manifest))
                .collect();
            self.policies.insert(key.clone(), found);
        }
        let manifest = self.policies[&key]
            .get(&(d.app_id, d.function_name.clone()))
            .cloned()
            .unwrap_or(Value::Null);
        Ok(listener_task_policy(&manifest))
    }

    async fn workspace(&mut self, app_id: Uuid) -> Result<Option<Uuid>, DbErr> {
        if let Some(found) = self.workspaces.get(&app_id) {
            return Ok(*found);
        }
        let found = apps::Entity::find_by_id(app_id)
            .one(self.db)
            .await?
            .map(|a| a.project_id);
        self.workspaces.insert(app_id, found);
        Ok(found)
    }
}
