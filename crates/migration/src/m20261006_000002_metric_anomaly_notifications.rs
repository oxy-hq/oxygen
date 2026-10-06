use sea_orm_migration::prelude::*;

/// Remember which insights a workspace has already been told about.
///
/// # Why
///
/// A `.monitor.yml` with a `notify:` block has each scan post the events it
/// newly found to a Slack channel (`oxy_metric_monitoring::notify`). A scan
/// re-scores a rolling window of buckets every day, two scans can finish
/// together, and a delivery task can die mid-post — so "has this event been
/// announced?" has to be a row, not something a scan infers.
///
/// `metric_anomaly_notifications` is that row: one per (workspace, event,
/// channel). A delivery task claims the due events with an upsert before it
/// posts and stamps `delivered_at` once Slack accepts the message; a claim left
/// undelivered by a task that died goes stale and can be taken again.
/// `claim_id` names the task that holds the claim, so it reads back, marks and
/// releases exactly the events it took and never another task's.
///
/// Ids and timestamps only — the observed values, labels and message text stay
/// in `metric_anomalies` and are read when the message is built. `event_id`
/// carries no foreign key on purpose: an event is a grouping column on
/// `metric_anomalies`, not a table, and `workspace_id` is loose the way it is
/// on `metric_anomalies` itself. A row lives as long as its event does: the
/// delivery task deletes the rows of events that are gone from
/// `metric_anomalies`, never by age, because an event can keep gaining buckets
/// for months and the row is what says it was already announced.
///
/// # Locking
///
/// A new table and an index on it; nothing existing is touched.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS metric_anomaly_notifications (\
               workspace_id UUID NOT NULL, \
               event_id UUID NOT NULL, \
               channel TEXT NOT NULL, \
               destination TEXT NOT NULL, \
               claim_id UUID NOT NULL, \
               claimed_at TIMESTAMPTZ NOT NULL, \
               delivered_at TIMESTAMPTZ, \
               PRIMARY KEY (workspace_id, event_id, channel)\
             )",
        )
        .await?;
        // A task reads back, marks and releases its claim by this pair.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_metric_anomaly_notifications_claim \
             ON metric_anomaly_notifications (workspace_id, claim_id)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS metric_anomaly_notifications")
            .await?;
        Ok(())
    }
}
