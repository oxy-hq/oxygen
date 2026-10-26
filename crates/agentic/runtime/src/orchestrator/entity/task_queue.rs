use sea_orm::entity::prelude::*;

/// Persisted task queue entry. Each row represents a task assignment that
/// survives process crashes. Workers poll this table for work; the coordinator
/// inserts rows when delegating.
///
/// Lifecycle: `queued` -> `claimed` -> `completed` | `failed` | `cancelled` | `dead`
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "agentic_task_queue")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub task_id: String,
    pub run_id: String,
    pub parent_task_id: Option<String>,
    /// `queued` | `claimed` | `completed` | `failed` | `cancelled` | `dead`
    pub queue_status: String,
    /// Serialized `TaskSpec` as JSONB.
    pub spec: Json,
    /// Serialized `TaskPolicy` as JSONB (optional).
    pub policy: Option<Json>,
    /// Which worker claimed this task (NULL while queued).
    pub worker_id: Option<String>,
    /// Last heartbeat from the worker executing this task.
    pub last_heartbeat: Option<DateTimeWithTimeZone>,
    /// When the worker claimed this task.
    pub claimed_at: Option<DateTimeWithTimeZone>,
    /// Per-task visibility timeout in seconds. If a claimed task's heartbeat is
    /// older than this, the reaper resets it to `queued`.
    pub visibility_timeout_secs: i32,
    /// How many times this task has been claimed (incremented on each claim).
    pub claim_count: i32,
    /// Identity of the current claim: bumped by every claim and adopt, never
    /// decremented or reset (unlike `claim_count`, a retry budget). Owned
    /// writes predicate on it so a stale execution matches no row.
    pub claim_epoch: i64,
    /// Maximum number of claims before the task is dead-lettered.
    pub max_claims: i32,
    /// `true` when a co-located scoped coordinator owns this task's tree
    /// (stamped at INSERT). The global/recovery claim path filters
    /// `scope_owned = false` so it never poaches interactive runs. Preserved
    /// across `claimed -> queued` reaping.
    pub scope_owned: bool,
    /// Earliest time this task may be claimed. Defaults to `now()`, so a row
    /// that never sets it is claimable immediately — the pre-existing
    /// behaviour.
    ///
    /// This is what lets a deferral be spelled as *"not yet"* rather than as a
    /// claim that is allowed to time out. The latter was the only option
    /// before, and it burns `claim_count` toward `max_claims` while looking
    /// exactly like a worker that crashed.
    pub available_at: DateTimeWithTimeZone,
    /// When the current run of consecutive deferrals began; `NULL` when the
    /// task is not waiting. Set on the first defer and left alone by later
    /// ones, so it measures the whole streak rather than the last hop.
    pub first_deferred_at: Option<DateTimeWithTimeZone>,
    /// Declared retries this task has spent (`TaskPolicy::retry`): the
    /// execution in flight is attempt `retry_attempt + 1`. Bumped only by
    /// `crud::requeue_failed_attempt`; reset by a fresh enqueue.
    pub retry_attempt: i32,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "crate::lifecycle::entity::run::Entity",
        from = "Column::RunId",
        to = "crate::lifecycle::entity::run::Column::Id",
        on_delete = "Cascade"
    )]
    Run,
}

impl Related<crate::lifecycle::entity::run::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Run.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
