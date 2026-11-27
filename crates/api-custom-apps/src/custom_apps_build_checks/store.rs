//! The `app_build_checks` rows: written `queued` with the task, claimed and
//! closed through the runner routes, read by every route.
//!
//! **A verdict is written once.** [`finish`] and [`close_open`] update a row
//! only while it is still `queued` or `running`, in the statement itself, so
//! two writers (a runner's report and a rerun closing a stalled attempt, two
//! runners on one attempt) cannot both win: the second finds nothing to write.
//!
//! **An attempt settles once.** It is settled when every row that is not a
//! model's has a verdict, an advisory row (`changes`) included
//! ([`is_settled`]). [`finish_all`] writes a runner route's verdicts under the
//! attempt's row lock and says whether THIS call took the attempt from
//! unsettled to settled; rows are never reopened, so exactly one call ever
//! does. [`close_abandoned`] is the same for the advisory check a dead runner
//! left open.

use chrono::{DateTime, Utc};
use entity::app_build_checks::{self, Column, Entity};
use sea_orm::sea_query::Expr;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, TransactionTrait,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::kind::{CheckKind, CheckStatus, Outcome};
use super::stalled::{self, NotStarted, Runner};

/// One attempt of one build's checks in one environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptKey {
    pub build_pk: Uuid,
    pub environment: String,
    pub attempt: i32,
}

/// The two statuses a row has before its verdict.
const OPEN: [&str; 2] = ["queued", "running"];

/// Write one `queued` row per kind in `kinds` for `key`, naming the run that
/// starts their runner; a kind that only informs is written `advisory`. A row
/// already there (the same attempt queued twice) is left as it is.
pub async fn insert_queued<C: ConnectionTrait>(
    c: &C,
    app_id: Uuid,
    key: &AttemptKey,
    run_id: &str,
    kinds: &[CheckKind],
) -> Result<(), DbErr> {
    let now = Utc::now().fixed_offset();
    let rows = kinds.iter().map(|kind| app_build_checks::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        build_id: ActiveValue::Set(key.build_pk),
        environment: ActiveValue::Set(key.environment.clone()),
        kind: ActiveValue::Set(kind.as_str().to_string()),
        attempt: ActiveValue::Set(key.attempt),
        status: ActiveValue::Set(CheckStatus::Queued.as_str().to_string()),
        advisory: ActiveValue::Set(kind.advisory()),
        summary: ActiveValue::Set(None),
        detail: ActiveValue::Set(None),
        notes: ActiveValue::Set(json!([])),
        run_id: ActiveValue::Set(Some(run_id.to_string())),
        runner: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        started_at: ActiveValue::Set(None),
        finished_at: ActiveValue::Set(None),
    });
    Entity::insert_many(rows)
        .on_conflict(
            OnConflict::columns([
                Column::BuildId,
                Column::Environment,
                Column::Kind,
                Column::Attempt,
            ])
            .do_nothing()
            .to_owned(),
        )
        .exec_without_returning(c)
        .await?;
    Ok(())
}

/// The newest attempt of `build_pk` in `environment`, if any was queued.
pub async fn latest_attempt<C: ConnectionTrait>(
    c: &C,
    build_pk: Uuid,
    environment: &str,
) -> Result<Option<i32>, DbErr> {
    Entity::find()
        .filter(Column::BuildId.eq(build_pk))
        .filter(Column::Environment.eq(environment))
        .select_only()
        .column_as(Column::Attempt.max(), "attempt")
        .into_tuple::<Option<i32>>()
        .one(c)
        .await
        .map(Option::flatten)
}

/// Every row of `build_pk` in `environment`, newest attempt first.
pub async fn rows_of_build<C: ConnectionTrait>(
    c: &C,
    build_pk: Uuid,
    environment: &str,
) -> Result<Vec<app_build_checks::Model>, DbErr> {
    Entity::find()
        .filter(Column::BuildId.eq(build_pk))
        .filter(Column::Environment.eq(environment))
        .order_by_desc(Column::Attempt)
        .order_by_asc(Column::CreatedAt)
        .all(c)
        .await
}

fn of_attempt(key: &AttemptKey) -> sea_orm::Condition {
    sea_orm::Condition::all()
        .add(Column::BuildId.eq(key.build_pk))
        .add(Column::Environment.eq(key.environment.as_str()))
        .add(Column::Attempt.eq(key.attempt))
}

/// The rows of one attempt.
pub async fn rows_of<C: ConnectionTrait>(
    c: &C,
    key: &AttemptKey,
) -> Result<Vec<app_build_checks::Model>, DbErr> {
    Entity::find().filter(of_attempt(key)).all(c).await
}

/// The rows of one attempt, locked until the transaction ends: a claim reads
/// and writes them as one step, so two claims of one attempt take turns.
pub async fn lock_rows<C: ConnectionTrait>(
    txn: &C,
    key: &AttemptKey,
) -> Result<Vec<app_build_checks::Model>, DbErr> {
    Entity::find()
        .filter(of_attempt(key))
        .order_by_asc(Column::Kind)
        .lock_exclusive()
        .all(txn)
        .await
}

/// Record who runs the attempt, on every row of it.
pub async fn set_runner<C: ConnectionTrait>(
    c: &C,
    key: &AttemptKey,
    runner: &Runner,
) -> Result<(), DbErr> {
    let stored = serde_json::to_value(runner).map_err(|e| DbErr::Custom(e.to_string()))?;
    Entity::update_many()
        .col_expr(Column::Runner, Some(stored).into())
        .filter(of_attempt(key))
        .exec(c)
        .await?;
    Ok(())
}

/// Record that no runner was started for the attempt, on every row of it no
/// runner has claimed: a claim's runner is never replaced by this.
pub async fn mark_not_started<C: ConnectionTrait>(
    c: &C,
    key: &AttemptKey,
    marker: &NotStarted,
) -> Result<(), DbErr> {
    let stored = serde_json::to_value(marker).map_err(|e| DbErr::Custom(e.to_string()))?;
    Entity::update_many()
        .col_expr(Column::Runner, Some(stored).into())
        .filter(of_attempt(key))
        .filter(Column::Runner.is_null())
        .exec(c)
        .await?;
    Ok(())
}

/// Every check of the attempt still `queued` has started.
pub async fn mark_queued_running<C: ConnectionTrait>(
    c: &C,
    key: &AttemptKey,
    now: DateTime<Utc>,
) -> Result<(), DbErr> {
    Entity::update_many()
        .col_expr(Column::Status, CheckStatus::Running.as_str().into())
        .col_expr(Column::StartedAt, now.fixed_offset().into())
        .filter(of_attempt(key))
        .filter(Column::Status.eq(CheckStatus::Queued.as_str()))
        .exec(c)
        .await?;
    Ok(())
}

/// Keep what a check that is still running needs to finish (the `functions`
/// check's run ids) in its detail. Only a `running` row takes it.
pub async fn keep_while_running<C: ConnectionTrait>(
    c: &C,
    key: &AttemptKey,
    kind: CheckKind,
    detail: Value,
) -> Result<(), DbErr> {
    Entity::update_many()
        .col_expr(Column::Detail, Some(detail).into())
        .filter(of_attempt(key))
        .filter(Column::Kind.eq(kind.as_str()))
        .filter(Column::Status.eq(CheckStatus::Running.as_str()))
        .exec(c)
        .await?;
    Ok(())
}

/// Write the check's verdict, unless it already has one. `true` when this
/// call wrote it.
pub async fn finish<C: ConnectionTrait>(
    c: &C,
    key: &AttemptKey,
    kind: CheckKind,
    outcome: &Outcome,
) -> Result<bool, DbErr> {
    let now = Utc::now().fixed_offset();
    let written = Entity::update_many()
        .col_expr(Column::Status, outcome.status.as_str().into())
        .col_expr(Column::Summary, Some(outcome.summary.clone()).into())
        .col_expr(Column::Detail, Some(outcome.detail.clone()).into())
        .col_expr(Column::FinishedAt, now.into())
        .filter(of_attempt(key))
        .filter(Column::Kind.eq(kind.as_str()))
        .filter(Column::Status.is_in(OPEN))
        .exec(c)
        .await?;
    Ok(written.rows_affected > 0)
}

/// Close every check of the attempt still without a verdict with `outcome`.
/// The number closed.
pub async fn close_open<C: ConnectionTrait>(
    c: &C,
    key: &AttemptKey,
    outcome: &Outcome,
) -> Result<u64, DbErr> {
    let now = Utc::now().fixed_offset();
    let closed = Entity::update_many()
        .col_expr(Column::Status, outcome.status.as_str().into())
        .col_expr(Column::Summary, Some(outcome.summary.clone()).into())
        .col_expr(Column::Detail, Some(outcome.detail.clone()).into())
        .col_expr(Column::FinishedAt, now.into())
        .filter(of_attempt(key))
        .filter(Column::Status.is_in(OPEN))
        .exec(c)
        .await?;
    Ok(closed.rows_affected)
}

/// Whether one attempt's `rows` are settled: there are some, and a runner
/// owes none of them a verdict (`stalled::is_owed`: every row that is not a
/// model's has one, an advisory row included).
pub fn is_settled(rows: &[app_build_checks::Model]) -> bool {
    !rows.is_empty() && !rows.iter().any(stalled::is_owed)
}

/// Close what a runner that stopped left open of an attempt it had otherwise
/// finished (`stalled::abandoned`, decided again under the attempt's lock, so
/// two readers close it once): its open checks are written `error`, saying
/// why. `true` when this call settled the attempt. No model's row is open in
/// such an attempt: they are written after it has settled.
pub async fn close_abandoned(
    db: &DatabaseConnection,
    key: &AttemptKey,
    now: DateTime<Utc>,
) -> Result<bool, DbErr> {
    let txn = db.begin().await?;
    let before = lock_rows(&txn, key).await?;
    let Some(why) = stalled::abandoned(&before, now) else {
        return Ok(false);
    };
    close_open(&txn, key, &Outcome::error(why.summary())).await?;
    let settled = settled_by(&txn, key, &before).await?;
    txn.commit().await?;
    Ok(settled)
}

/// What [`finish_all`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Finished {
    /// The kinds whose verdict this call wrote.
    pub wrote: Vec<CheckKind>,
    /// This call took the attempt from unsettled to settled.
    pub settled: bool,
}

/// Write each of `verdicts` unless its check already has one, in one
/// transaction holding the attempt's rows, and say whether the attempt
/// settled by it (module docs).
pub async fn finish_all(
    db: &DatabaseConnection,
    key: &AttemptKey,
    verdicts: &[(CheckKind, Outcome)],
) -> Result<Finished, DbErr> {
    let txn = db.begin().await?;
    let finished = finish_in(&txn, key, verdicts).await?;
    txn.commit().await?;
    Ok(finished)
}

/// [`finish_all`] inside the caller's transaction, which it commits: for a
/// caller that writes something with the verdicts (the audit row of a report
/// by hand), so both land or neither does.
pub async fn finish_in<C: ConnectionTrait>(
    txn: &C,
    key: &AttemptKey,
    verdicts: &[(CheckKind, Outcome)],
) -> Result<Finished, DbErr> {
    let before = lock_rows(txn, key).await?;
    let mut wrote = Vec::new();
    for (kind, outcome) in verdicts {
        if finish(txn, key, *kind, outcome).await? {
            wrote.push(*kind);
        }
    }
    let settled = settled_by(txn, key, &before).await?;
    Ok(Finished { wrote, settled })
}

/// Whether the transaction holding `key`'s rows (locked as `before`) has
/// settled the attempt: it was not settled then, and it is now.
pub async fn settled_by<C: ConnectionTrait>(
    txn: &C,
    key: &AttemptKey,
    before: &[app_build_checks::Model],
) -> Result<bool, DbErr> {
    if is_settled(before) {
        return Ok(false);
    }
    Ok(is_settled(&rows_of(txn, key).await?))
}

/// Merge `patch` (an object) into one row's detail, key by key, leaving every
/// other key as it is: what the platform adds to a check after its verdict
/// (who saw the changed screens, what followed a failed production check).
/// A row with no detail gets `patch` as its detail. `true` when a row took it.
pub async fn merge_detail<C: ConnectionTrait>(
    c: &C,
    row_id: uuid::Uuid,
    patch: Value,
) -> Result<bool, DbErr> {
    let merged = Entity::update_many()
        .col_expr(
            Column::Detail,
            Expr::cust_with_values("COALESCE(detail, '{}'::jsonb) || $1::jsonb", [patch]),
        )
        .filter(Column::Id.eq(row_id))
        .exec(c)
        .await?;
    Ok(merged.rows_affected > 0)
}

/// Write who saw the changed screens (`detail.seen`) on a `changes` row,
/// unless it already says: the first mark stands. `true` when this call
/// wrote it.
pub async fn mark_seen<C: ConnectionTrait>(
    c: &C,
    row_id: uuid::Uuid,
    seen: Value,
) -> Result<bool, DbErr> {
    let marked = Entity::update_many()
        .col_expr(
            Column::Detail,
            Expr::cust_with_values(
                "jsonb_set(COALESCE(detail, '{}'::jsonb), '{seen}', $1::jsonb)",
                [seen],
            ),
        )
        .filter(Column::Id.eq(row_id))
        .filter(Expr::cust(
            "COALESCE(detail->'seen', 'null'::jsonb) = 'null'::jsonb",
        ))
        .exec(c)
        .await?;
    Ok(marked.rows_affected > 0)
}
