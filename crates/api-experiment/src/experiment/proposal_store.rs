//! The only place `experiment_proposals` is read or written. Every list query carries its `workspace_id`.
//!
//! A proposal is inserted `pending` by the handler and settled exactly once by
//! the job: each `finish_*` is `UPDATE … WHERE status = 'pending'`, so a
//! redelivered attempt cannot overwrite a settled row and two attempts cannot
//! both settle one. They return whether *this call* settled it.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset};

use entity::experiment_proposals;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, DbErr, EntityTrait, FromQueryResult, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Statement,
};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalStatus {
    Pending,
    Done,
    Refused,
    Failed,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Done => "done",
            Self::Refused => "refused",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [Self::Pending, Self::Done, Self::Refused, Self::Failed]
            .into_iter()
            .find(|v| v.as_str() == s)
    }
}

pub struct NewPending {
    pub id: Uuid,
    /// Shared by every candidate of one price request.
    pub request_id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    /// The experiment's design, and its `content_hash` as priced.
    pub design_id: Uuid,
    pub design_hash: String,
    /// The one `Candidate`, as received.
    pub request: Value,
    /// The α this candidate is priced at: its override, else `outcome.alpha`.
    pub alpha: f64,
    pub requested_by: Uuid,
}

/// Insert the `pending` row that holds a candidate. Generic over the connection
/// so the handler can write every candidate in the same transaction as the queue rows.
pub async fn insert_pending(db: &impl ConnectionTrait, p: NewPending) -> Result<(), DbErr> {
    experiment_proposals::ActiveModel {
        id: ActiveValue::Set(p.id),
        created_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
        workspace_id: ActiveValue::Set(p.workspace_id),
        name: ActiveValue::Set(p.name),
        design_id: ActiveValue::Set(Some(p.design_id)),
        design_hash: ActiveValue::Set(Some(p.design_hash)),
        request_id: ActiveValue::Set(p.request_id),
        status: ActiveValue::Set(ProposalStatus::Pending.as_str().to_string()),
        request: ActiveValue::Set(p.request),
        alpha: ActiveValue::Set(p.alpha),
        seed: ActiveValue::Set(None),
        waves: ActiveValue::Set(None),
        periods: ActiveValue::Set(None),
        strata: ActiveValue::Set(None),
        assign_by: ActiveValue::Set(None),
        power: ActiveValue::Set(None),
        mde_unit_day: ActiveValue::Set(None),
        prior_coefficient: ActiveValue::Set(None),
        warnings: ActiveValue::Set(serde_json::json!([])),
        refusal: ActiveValue::Set(None),
        requested_by: ActiveValue::Set(p.requested_by),
        finished_at: ActiveValue::Set(None),
    }
    .insert(db)
    .await?;
    Ok(())
}

pub async fn find_proposal(
    db: &DatabaseConnection,
    id: Uuid,
) -> Result<Option<experiment_proposals::Model>, DbErr> {
    experiment_proposals::Entity::find_by_id(id).one(db).await
}

/// What a `done` row carries beyond its status.
pub struct DoneFields {
    pub seed: u64,
    pub waves: Option<Value>,
    pub periods: Option<Value>,
    pub strata: Option<Value>,
    pub assign_by: Option<String>,
    pub power: Value,
    pub mde_unit_day: f64,
    pub prior_coefficient: Option<Value>,
    pub warnings: Vec<String>,
}

async fn settle(
    db: &DatabaseConnection,
    sql: &str,
    values: Vec<sea_orm::Value>,
) -> Result<bool, DbErr> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await?;
    Ok(result.rows_affected() > 0)
}

/// `pending → done`. `seed` is stored `u64 as i64`; the job only draws 53-bit
/// seeds, so the cast never sets the sign bit.
pub async fn finish_done(db: &DatabaseConnection, id: Uuid, f: DoneFields) -> Result<bool, DbErr> {
    settle(
        db,
        "UPDATE experiment_proposals \
         SET status = 'done', seed = $2, waves = $3::jsonb, periods = $4::jsonb, strata = $5::jsonb, \
             assign_by = $6, power = $7::jsonb, mde_unit_day = $8, prior_coefficient = $9::jsonb, \
             warnings = $10::jsonb, finished_at = now() \
         WHERE id = $1 AND status = 'pending'",
        vec![
            id.into(),
            (f.seed as i64).into(),
            f.waves.into(),
            f.periods.into(),
            f.strata.into(),
            f.assign_by.into(),
            f.power.into(),
            f.mde_unit_day.into(),
            f.prior_coefficient.into(),
            serde_json::json!(f.warnings).into(),
        ],
    )
    .await
}

/// `pending → refused`: a value, with what was tested.
pub async fn finish_refused(
    db: &DatabaseConnection,
    id: Uuid,
    reason: &str,
    warnings: &[String],
) -> Result<bool, DbErr> {
    settle(
        db,
        "UPDATE experiment_proposals \
         SET status = 'refused', refusal = $2, warnings = $3::jsonb, finished_at = now() \
         WHERE id = $1 AND status = 'pending'",
        vec![id.into(), reason.into(), serde_json::json!(warnings).into()],
    )
    .await
}

/// `pending → failed`: a fault; `refusal` carries the reason.
pub async fn finish_failed(db: &DatabaseConnection, id: Uuid, reason: &str) -> Result<bool, DbErr> {
    settle(
        db,
        "UPDATE experiment_proposals \
         SET status = 'failed', refusal = $2, finished_at = now() \
         WHERE id = $1 AND status = 'pending'",
        vec![id.into(), reason.into()],
    )
    .await
}

/// How many assignments were drawn — `done` rows only, **every candidate of
/// every request**: a refusal or a fault is not an assignment anyone could
/// commit, and choosing among candidates is choosing among draws.
pub async fn count_done(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
) -> Result<u64, DbErr> {
    experiment_proposals::Entity::find()
        .filter(experiment_proposals::Column::WorkspaceId.eq(workspace_id))
        .filter(experiment_proposals::Column::Name.eq(name))
        .filter(experiment_proposals::Column::Status.eq(ProposalStatus::Done.as_str()))
        .count(db)
        .await
}

/// [`count_done`], restricted to proposals created before `before` — the report's
/// count: draws logged before the design was registered (C3, C5).
pub async fn count_done_before(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
    before: DateTime<FixedOffset>,
) -> Result<u64, DbErr> {
    experiment_proposals::Entity::find()
        .filter(experiment_proposals::Column::WorkspaceId.eq(workspace_id))
        .filter(experiment_proposals::Column::Name.eq(name))
        .filter(experiment_proposals::Column::Status.eq(ProposalStatus::Done.as_str()))
        .filter(experiment_proposals::Column::CreatedAt.lt(before))
        .count(db)
        .await
}

/// Pricing's reason no MDE was priced, when there is one to state: the newest
/// proposal logged before `before`, of any status, kept only if it was refused
/// and priced the design frozen at `frozen_hash` ([`super::refusal_note`]).
pub async fn latest_refusal_before(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
    before: DateTime<FixedOffset>,
    frozen_hash: &str,
) -> Result<Option<String>, DbErr> {
    let newest = experiment_proposals::Entity::find()
        .filter(experiment_proposals::Column::WorkspaceId.eq(workspace_id))
        .filter(experiment_proposals::Column::Name.eq(name))
        .filter(experiment_proposals::Column::CreatedAt.lt(before))
        .order_by_desc(experiment_proposals::Column::CreatedAt)
        .one(db)
        .await?;
    Ok(super::refusal_note::refusal_to_quote(
        newest.as_ref(),
        frozen_hash,
    ))
}

pub async fn list_proposals(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
    limit: u64,
) -> Result<Vec<experiment_proposals::Model>, DbErr> {
    experiment_proposals::Entity::find()
        .filter(experiment_proposals::Column::WorkspaceId.eq(workspace_id))
        .filter(experiment_proposals::Column::Name.eq(name))
        .order_by_desc(experiment_proposals::Column::CreatedAt)
        .limit(limit)
        .all(db)
        .await
}

/// The candidates of one price request, in the order they were submitted.
pub async fn list_request(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    request_id: Uuid,
) -> Result<Vec<experiment_proposals::Model>, DbErr> {
    experiment_proposals::Entity::find()
        .filter(experiment_proposals::Column::WorkspaceId.eq(workspace_id))
        .filter(experiment_proposals::Column::RequestId.eq(request_id))
        .order_by_asc(experiment_proposals::Column::CreatedAt)
        .all(db)
        .await
}

#[derive(FromQueryResult)]
struct StoppedRow {
    id: Uuid,
    queue_status: Option<String>,
}

/// A `pending` proposal whose job was queued (its run row exists) but whose
/// task is no longer `queued` or `claimed`: dead-lettered after repeated worker
/// crashes, `failed` because the payload did not decode or the job's own
/// settling write errored, cancelled, or already purged by retention. No
/// attempt will ever settle it, so the read models report it `failed`.
const STOPPED_SQL: &str = "SELECT p.id, q.queue_status \
    FROM experiment_proposals p \
    JOIN agentic_runs r ON r.id = p.id::text \
    LEFT JOIN agentic_task_queue q ON q.task_id = p.id::text \
    WHERE p.workspace_id = $1 AND p.status = 'pending' \
      AND (q.queue_status IS NULL OR q.queue_status NOT IN ('queued', 'claimed'))";

/// The workspace's `pending` proposals whose price job stopped without settling
/// them, each with the reason the read models report. Derived, never written:
/// an operator who re-enqueues the dead task gets the row back as `pending`.
pub async fn stopped_jobs(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<HashMap<Uuid, String>, DbErr> {
    let rows = StoppedRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        STOPPED_SQL,
        [workspace_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let state = match r.queue_status {
                Some(s) => format!("its task is `{s}`"),
                None => "its task is no longer in the queue".to_string(),
            };
            let reason = format!("the price job stopped without settling this proposal: {state}");
            (r.id, reason)
        })
        .collect())
}
