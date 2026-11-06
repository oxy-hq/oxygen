//! The workspace queue-health read behind `GET /coordinator/queue`.

use sea_orm::{DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement};

/// Most dead-lettered tasks [`get_queue_stats`] lists, newest first.
/// [`QueueStats::dead`] still counts all of them.
pub const DEAD_TASKS_LIMIT: u64 = 100;

/// Most stale claims [`get_queue_stats`] lists, oldest heartbeat first.
pub const STALE_TASKS_LIMIT: u64 = 50;

/// The columns the listings select — what [`QueueTaskRow`] carries and no more.
const ROW_COLUMNS: &str = "q.task_id, q.run_id, q.queue_status, q.worker_id, q.claim_count, \
     q.max_claims, q.last_heartbeat, q.created_at, q.updated_at";

/// A plain DTO for a task queue entry, avoiding leaking entity types.
#[derive(Debug, FromQueryResult)]
pub struct QueueTaskRow {
    pub task_id: String,
    pub run_id: String,
    pub queue_status: String,
    pub worker_id: Option<String>,
    pub claim_count: i32,
    pub max_claims: i32,
    pub last_heartbeat: Option<chrono::DateTime<chrono::FixedOffset>>,
    pub created_at: chrono::DateTime<chrono::FixedOffset>,
    pub updated_at: chrono::DateTime<chrono::FixedOffset>,
}

/// Queue status counts and stale/dead-lettered task details for the dashboard.
#[derive(Debug, Default)]
pub struct QueueStats {
    pub queued: u64,
    pub claimed: u64,
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub dead: u64,
    /// Tasks claimed but with heartbeat older than their visibility timeout,
    /// at most [`STALE_TASKS_LIMIT`].
    pub stale_tasks: Vec<QueueTaskRow>,
    /// Dead-lettered tasks, newest first, at most [`DEAD_TASKS_LIMIT`].
    pub dead_tasks: Vec<QueueTaskRow>,
    /// `true` when [`Self::dead`] is more than [`Self::dead_tasks`] lists.
    pub dead_tasks_truncated: bool,
}

/// **Workspace-scoped** by joining `agentic_task_queue.run_id` →
/// `agentic_runs.workspace_id`. The queue table itself doesn't carry a
/// `workspace_id` column today; the JOIN keeps the dashboard
/// tenant-correct without a schema change.
///
/// **The customer's own work only.** This is what a workspace's members read
/// of the queue, so the join also applies the run feed's rule
/// (`visibility::customer_run_sql`): a task of a workspace preview's dry run,
/// or of a custom-app check run staff queued outside production, is neither
/// counted nor listed. A dead sandbox check is not the tenant's dead job, and
/// listing it would hand out the run id every other route hides.
///
/// **Bounded.** The dead list used to be every dead row the workspace kept
/// (30 days of them) with every column; both lists now select only
/// [`QueueTaskRow`]'s columns and stop at their limit.
pub async fn get_queue_stats(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
) -> Result<QueueStats, DbErr> {
    // One predicate for all three statements, over the `r` they all join.
    let customers_own = crate::lifecycle::crud::visibility::customer_run_sql("r.");
    let scope = format!(
        "FROM agentic_task_queue q \
         INNER JOIN agentic_runs r ON r.id = q.run_id \
         WHERE r.workspace_id = $1 AND {customers_own}"
    );
    let mut stats = count_by_status(db, workspace_id, &scope).await?;
    stats.stale_tasks = list(
        db,
        workspace_id,
        &format!(
            "{scope} AND q.queue_status = 'claimed' \
               AND q.last_heartbeat < now() - (q.visibility_timeout_secs || ' seconds')::interval \
             ORDER BY q.last_heartbeat LIMIT {STALE_TASKS_LIMIT}"
        ),
    )
    .await?;
    stats.dead_tasks = list(
        db,
        workspace_id,
        &format!(
            "{scope} AND q.queue_status = 'dead' \
             ORDER BY q.updated_at DESC LIMIT {DEAD_TASKS_LIMIT}"
        ),
    )
    .await?;
    stats.dead_tasks_truncated = stats.dead > stats.dead_tasks.len() as u64;
    Ok(stats)
}

async fn count_by_status(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    scope: &str,
) -> Result<QueueStats, DbErr> {
    #[derive(Debug, FromQueryResult)]
    struct StatusCount {
        queue_status: String,
        cnt: i64,
    }

    let rows = StatusCount::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("SELECT q.queue_status, COUNT(*) AS cnt {scope} GROUP BY q.queue_status"),
        [workspace_id.into()],
    ))
    .all(db)
    .await?;

    let mut stats = QueueStats::default();
    for row in &rows {
        let count = row.cnt as u64;
        match row.queue_status.as_str() {
            "queued" => stats.queued = count,
            "claimed" => stats.claimed = count,
            "completed" => stats.completed = count,
            "failed" => stats.failed = count,
            "cancelled" => stats.cancelled = count,
            "dead" => stats.dead = count,
            _ => {}
        }
    }
    Ok(stats)
}

/// `rest` is the `FROM … WHERE …` tail, ordering and limit included.
async fn list(
    db: &DatabaseConnection,
    workspace_id: uuid::Uuid,
    rest: &str,
) -> Result<Vec<QueueTaskRow>, DbErr> {
    QueueTaskRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!("SELECT {ROW_COLUMNS} {rest}"),
        [workspace_id.into()],
    ))
    .all(db)
    .await
}
