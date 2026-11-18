//! Queuing a listener's task for a delivery, as the relay does
//! (`relay::deliver`): an `app_function` task, trigger `event` (`call` for a
//! directed send, `ctx.apps.send`, whose function need not listen to anything), the delivery
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
use entity::{app_event_deliveries as deliveries, app_functions, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_custom_apps::calls::SendTarget;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DatabaseTransaction, DbErr, EntityTrait, QueryFilter,
};
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

/// Whether an event of `event_type` delivered to `function` is a directed send
/// (`ctx.apps.send`) to it: its outbox name, after the publisher, is a
/// [`SendTarget`] naming that function. Such a delivery is queued with trigger
/// `call`, as the relay queues it, and the executor kills a send-named event
/// queued any other way.
pub(super) fn is_directed(event_type: &str, function: &str) -> bool {
    event_type
        .split_once('/')
        .and_then(|(_, name)| SendTarget::parse(name))
        .is_some_and(|t| t.function == function)
}

/// What a call reads once per listener: its policy and its app's workspace.
pub(super) struct Queuer<'a> {
    db: &'a DatabaseConnection,
    org_id: Uuid,
    policies: HashMap<(String, String), HashMap<(Uuid, String), Value>>,
    workspaces: HashMap<Uuid, Option<Uuid>>,
    /// A directed send's retry policy, once per callee function and
    /// environment (`callee_policy`).
    callee_policies: HashMap<(Uuid, String, String), TaskPolicy>,
    pub(super) spread: Spread,
}

impl<'a> Queuer<'a> {
    pub(super) fn new(db: &'a DatabaseConnection, org_id: Uuid) -> Self {
        Self {
            db,
            org_id,
            policies: HashMap::new(),
            workspaces: HashMap::new(),
            callee_policies: HashMap::new(),
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
        // A directed send is queued as the relay queued it: trigger `call`,
        // the policy from the callee's own manifest (it has no subscription).
        let directed = is_directed(event_type, &delivery.function_name);
        let policy = if directed {
            self.callee_policy(delivery).await?
        } else {
            self.policy(delivery, event_type).await?
        };
        let mut task = AppFunctionTask::new(delivery.app_id.to_string(), &delivery.function_name);
        task.trigger = Some(if directed { "call" } else { "event" }.to_string());
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

    /// A directed send's retry policy, from the callee function's manifest in
    /// the build its environment serves now; the default when there is none.
    async fn callee_policy(&mut self, d: &deliveries::Model) -> Result<TaskPolicy, DbErr> {
        let key = (d.app_id, d.function_name.clone(), d.environment.clone());
        if let Some(policy) = self.callee_policies.get(&key) {
            return Ok(policy.clone());
        }
        let manifest = async {
            let app = apps::Entity::find_by_id(d.app_id).one(self.db).await?;
            let (Some(app), Some(env)) = (app, AppEnvironment::parse(&d.environment)) else {
                return Ok::<_, DbErr>(None);
            };
            let Some(build_id) =
                crate::custom_apps_env_resolve::resolve_function_environment(self.db, &app, &env)
                    .await?
                    .build_id
            else {
                return Ok(None);
            };
            Ok(app_functions::Entity::find()
                .filter(app_functions::Column::BuildId.eq(build_id))
                .filter(app_functions::Column::Name.eq(&d.function_name))
                .one(self.db)
                .await?
                .and_then(|f| f.manifest_json))
        }
        .await?;
        let policy = listener_task_policy(&manifest.unwrap_or(Value::Null));
        self.callee_policies.insert(key, policy.clone());
        Ok(policy)
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

#[cfg(test)]
mod tests {
    use super::is_directed;

    /// Redelivering or replaying a send queues it as a call, never as a
    /// listener's event; any other event, or a send to another function, is
    /// an event.
    #[test]
    fn only_a_send_to_this_function_is_queued_as_a_call() {
        assert!(is_directed("sender/call:callee:handle:go", "handle"));
        assert!(!is_directed("sender/call:callee:handle:go", "other"));
        assert!(!is_directed("sender/statement.uploaded", "handle"));
        assert!(!is_directed("call:callee:handle:go", "handle"));
    }
}
