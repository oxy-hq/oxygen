//! Closing an attempt: how it ended, and what that does to its delivery
//! (the parent module's docs say when each is called, and why each is fenced
//! on its attempt's number). The error is stored capped
//! ([`cap_error`](crate::app_events::cap_error)), on the attempt and on the
//! delivery alike.

use chrono::Utc;
use entity::{app_event_deliveries as deliveries, app_event_delivery_attempts as attempts};
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ColumnTrait, DatabaseConnection, DatabaseTransaction, DbErr, EntityTrait, QueryFilter,
    TransactionTrait,
};
use uuid::Uuid;

use super::{AttemptOutcome, PENDING, lock};
use crate::app_events::cap_error;

/// The listener answered 2xx: `attempt` is `ok` and the delivery acked.
pub async fn finish_ok(
    db: &DatabaseConnection,
    delivery_id: Uuid,
    attempt: i32,
    invocation_id: Option<Uuid>,
) -> Result<(), DbErr> {
    let ended = Ended {
        attempt: Some(attempt),
        invocation_id,
        outcome: AttemptOutcome::Ok,
        error: None,
    };
    finish(db, delivery_id, ended, "acked").await
}

/// `attempt` failed and the queue retries the task: the delivery stays
/// pending with `error` as its last.
pub async fn finish_err(
    db: &DatabaseConnection,
    delivery_id: Uuid,
    attempt: i32,
    invocation_id: Option<Uuid>,
    outcome: AttemptOutcome,
    error: &str,
) -> Result<(), DbErr> {
    let ended = Ended {
        attempt: Some(attempt),
        invocation_id,
        outcome,
        error: Some(error),
    };
    finish(db, delivery_id, ended, PENDING).await
}

/// No attempt may follow: `attempt`, when [`begin`] opened one, ends with
/// `outcome` and `invocation_id` (a listener's `NonRetryableError` and its
/// failed last attempt both ran one), and the delivery is dead with `error` as
/// the reason.
pub async fn finish_dead(
    db: &DatabaseConnection,
    delivery_id: Uuid,
    attempt: Option<i32>,
    invocation_id: Option<Uuid>,
    outcome: AttemptOutcome,
    error: &str,
) -> Result<(), DbErr> {
    let ended = Ended {
        attempt,
        invocation_id,
        outcome,
        error: Some(error),
    };
    finish(db, delivery_id, ended, "dead").await
}

/// How one attempt ended, as a `finish_*` records it.
struct Ended<'a> {
    /// `None` when no attempt was opened: only the delivery moves.
    attempt: Option<i32>,
    invocation_id: Option<Uuid>,
    outcome: AttemptOutcome,
    error: Option<&'a str>,
}

impl Ended<'_> {
    /// The error as the attempt and the delivery store it.
    fn stored_error(&self) -> Option<String> {
        self.error.map(cap_error)
    }
}

/// Lock the delivery, close `ended.attempt` if it is still open, then move
/// the delivery to `status` (with the error and the invocation) only while it
/// is still pending. An attempt already closed is a reclaimed run's (module
/// docs): nothing is written.
async fn finish(
    db: &DatabaseConnection,
    delivery_id: Uuid,
    ended: Ended<'_>,
    status: &str,
) -> Result<(), DbErr> {
    let now = Utc::now().fixed_offset();
    let txn = db.begin().await?;
    if lock(&txn, delivery_id).await?.is_none() {
        return txn.commit().await;
    }
    if let Some(attempt) = ended.attempt
        && close_attempt(&txn, delivery_id, attempt, &ended, now).await? == 0
    {
        tracing::info!(
            %delivery_id,
            attempt,
            outcome = ended.outcome.as_str(),
            "app events: a reclaimed run finished an attempt already closed; nothing recorded"
        );
        return txn.commit().await;
    }
    let mut settle = deliveries::Entity::update_many()
        .col_expr(deliveries::Column::Status, Expr::value(status))
        .col_expr(deliveries::Column::UpdatedAt, Expr::value(now));
    if let Some(error) = ended.stored_error() {
        settle = settle.col_expr(deliveries::Column::LastError, Expr::value(error));
    }
    if let Some(invocation_id) = ended.invocation_id {
        settle = settle.col_expr(deliveries::Column::InvocationId, Expr::value(invocation_id));
    }
    settle
        .filter(deliveries::Column::Id.eq(delivery_id))
        .filter(deliveries::Column::Status.eq(PENDING))
        .exec(&txn)
        .await?;
    txn.commit().await
}

/// Close attempt `attempt` with `ended` if it is still open; how many rows.
async fn close_attempt(
    txn: &DatabaseTransaction,
    delivery_id: Uuid,
    attempt: i32,
    ended: &Ended<'_>,
    now: DateTimeWithTimeZone,
) -> Result<u64, DbErr> {
    Ok(attempts::Entity::update_many()
        .col_expr(attempts::Column::FinishedAt, Expr::value(now))
        .col_expr(
            attempts::Column::Outcome,
            Expr::value(ended.outcome.as_str()),
        )
        .col_expr(attempts::Column::Error, Expr::value(ended.stored_error()))
        .col_expr(
            attempts::Column::InvocationId,
            Expr::value(ended.invocation_id),
        )
        .filter(attempts::Column::DeliveryId.eq(delivery_id))
        .filter(attempts::Column::Attempt.eq(attempt))
        .filter(attempts::Column::FinishedAt.is_null())
        .exec(txn)
        .await?
        .rows_affected)
}
