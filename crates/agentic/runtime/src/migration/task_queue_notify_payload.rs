//! The queue's NOTIFY names the task that became claimable.
//!
//! `agentic_task_queue_notify_fn` sent an empty payload, so a wake could not
//! say whose task it was: every run's scoped poller in every process went back
//! to the queue on every enqueue fleet-wide. It now sends `task:<task_id>` and
//! `PostgresTaskRouter` wakes only the pollers whose root covers that task
//! (`orchestrator::router::wake`). When and where it fires is unchanged.
//!
//! An id longer than [`MAX_TASK_ID_IN_PAYLOAD`] bytes falls back to the empty
//! payload — Postgres refuses a payload of 8000 bytes or more, and an empty
//! one wakes everyone. Run ids are UUIDs and children append `.<n>`, so in
//! practice the payload is under 100 bytes.
//!
//! Rolling-deploy and revert safe: the old router ignores the payload, and the
//! new one reads an empty payload (the old trigger, or `down`) as "wake
//! everyone".

use sea_orm_migration::prelude::*;

/// Longest task id the trigger names; longer ids send the empty payload.
pub const MAX_TASK_ID_IN_PAYLOAD: usize = 1024;

pub struct AddTaskQueueNotifyPayload;

/// Named by hand: `DeriveMigrationName` takes the module path (see
/// `AddCompileTaskBackoffIndex`).
impl MigrationName for AddTaskQueueNotifyPayload {
    fn name(&self) -> &str {
        "m20261102_000001_add_task_queue_notify_payload"
    }
}

fn notify_fn(payload: &str) -> String {
    format!(
        "CREATE OR REPLACE FUNCTION agentic_task_queue_notify_fn() \
         RETURNS TRIGGER AS $$ \
         BEGIN \
             IF NEW.queue_status = 'queued' \
                AND (TG_OP = 'INSERT' OR OLD.queue_status IS DISTINCT FROM 'queued') \
             THEN \
                 PERFORM pg_notify('oxy_task_enqueued', {payload}); \
             END IF; \
             RETURN NEW; \
         END; \
         $$ LANGUAGE plpgsql;"
    )
}

#[async_trait::async_trait]
impl MigrationTrait for AddTaskQueueNotifyPayload {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let payload = format!(
            "CASE WHEN octet_length(NEW.task_id) <= {MAX_TASK_ID_IN_PAYLOAD} \
             THEN 'task:' || NEW.task_id ELSE '' END"
        );
        manager
            .get_connection()
            .execute_unprepared(&notify_fn(&payload))
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(&notify_fn("''"))
            .await?;
        Ok(())
    }
}
