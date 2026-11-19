//! A delivery's attempts, as the worker that runs its task records them (app
//! events design §6.6).
//!
//! The relay queues one `app_function` task per pending delivery (trigger
//! `event`, the delivery's id on the task). The executor that runs it:
//!
//! 1. [`begin`]s the delivery: anything but `pending` is skipped, an attempt an
//!    earlier run left open is closed as `crash`, and attempt n + 1 is opened.
//!    The event the listener receives is rebuilt from the delivery's own
//!    records ([`super::envelope`]), never taken from the task's copy;
//! 2. runs the listener, and
//! 3. closes the attempt with how it ended: [`finish_ok`] acks the delivery,
//!    [`finish_err`] leaves it pending for the queue's retry, [`finish_dead`]
//!    kills it.
//!
//! **A delivery leaves `pending` once.** Each `finish_*` closes its own
//! attempt whatever the delivery's status, but changes the delivery only while
//! it is still `pending`: a late finish (a worker the queue gave up on) never
//! revives a dead delivery or acks it, and the retention sweep, which kills a
//! pending delivery whose task died (`retention`, step 1), never finds one this
//! module already settled. `attempts` counts [`begin`]s, and nothing else
//! writes it.
//!
//! **The last attempt is the last.** `ctx.delivery.maxAttempts` is the count
//! the queue retries the task to (its stored policy), raised by the attempts
//! made before a redelivery, which restarts the budget
//! (`app_event_delivery_budgets`), so a handler can treat
//! its last attempt differently. The executor kills the delivery when that
//! attempt fails ([`DeliveryInfo::is_last`]), and [`begin`] answers
//! [`DeliveryStart::Dead`] rather than open one past it (the pod died during
//! the last). The queue may still hold a retry, since a claim the reaper took
//! back is not one it counts; that run finds the delivery dead and skips it.
//!
//! **A reclaimed run's late finish records nothing.** The queue reclaims a
//! task whose worker stopped heartbeating, and that worker may still be
//! running. The next run's [`begin`] closes the old attempt as `crash` and
//! opens its own, so each `finish_*` is fenced on its attempt's number: one
//! whose attempt is already closed changes neither the newer attempt nor the
//! delivery, which are the newer run's. `finish` locks the delivery row before
//! touching an attempt, in [`begin`]'s order, so the two never deadlock.
//!
//! **The trade-off on a last attempt.** If the reclaimed attempt was the last,
//! the next [`begin`] answers [`DeliveryStart::Dead`] and the delivery dies,
//! even if the old run then succeeds: its [`finish_ok`] finds its attempt
//! closed and cannot ack. The handler's work may have happened while the
//! record says `dead`; redelivering it is at-least-once, which handlers
//! already tolerate.
//!
//! **The body is read from the org's store, with the lock released.** The
//! platform keeps an event's metadata only; its body stays in the org's OLTP
//! outbox log. [`begin`] decides and opens the attempt in one locked
//! transaction, commits, and only then reads the body ([`load_event_data`]),
//! so the connection to the org's store, which may be cold, never holds the
//! delivery's row or any platform transaction. A body that is gone
//! ([`BODY_GONE`]) or can never be read (it cannot be decoded, or its
//! environment's store is not relayed) is [`DeliveryStart::Dead`]; a store
//! that cannot be reached closes the attempt as `error` and answers
//! [`DeliveryStart::Retry`], for the queue to try again (on the last attempt
//! the delivery is dead with that error: no retry follows). A store outage
//! longer than the retry window therefore dead-letters the deliveries it
//! caught; a redeliver recovers them.
//!
//! **No error text holds the body.** Every error an attempt or a delivery
//! stores is capped ([`super::cap_error`]), and the body read's own errors
//! name the event, never its content.

use agentic_core::delegation::TaskPolicy;
use chrono::Utc;
use entity::{app_event_deliveries as deliveries, app_event_delivery_attempts as attempts};
use oxy_custom_apps::events::OxyEvent;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, DatabaseTransaction, DbErr, EntityTrait, QueryFilter, QuerySelect,
    Statement, TransactionTrait,
};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::cap_error;
use super::envelope::{BodyError, Records, load_event_data};

mod finish;
pub use finish::{finish_dead, finish_err, finish_ok};

const PENDING: &str = "pending";

/// What an attempt left open by an earlier run is closed with.
pub const CRASHED: &str = "the attempt did not finish: its isolate or its worker died";

/// Why a delivery whose event has lost its payload is dead.
pub const BODY_GONE: &str = "event body no longer available";

/// The stable code a send's delivery killed for its sender's identity starts
/// with: the credential it was sent with no longer admits, or the person who
/// sent it may not open the receiving app (`app_function_executor::sender`).
pub const SENDER_REFUSED: &str = "sender_refused";

/// How one attempt ended. Stored as `app_event_delivery_attempts.outcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// The listener answered 2xx.
    Ok,
    /// It threw, or answered anything else: a bug to fix.
    Error,
    /// It ran past its `timeoutSeconds`.
    Timeout,
    /// The isolate died (its heap limit) or the pod did.
    Crash,
    /// It threw the SDK's `NonRetryableError`: no retry can help.
    NonRetryable,
}

impl AttemptOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Timeout => "timeout",
            Self::Crash => "crash",
            Self::NonRetryable => "non_retryable",
        }
    }
}

/// `ctx.delivery`: which attempt this run is, of how many, for which listener.
/// Beside the envelope, not in it, so the envelope stays one unchanging event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryInfo {
    /// 1 for the first attempt.
    pub attempt: i32,
    /// The attempts the queue makes in all: one plus the task's retries.
    pub max_attempts: i32,
    /// `<app slug>/<function>`.
    pub listener: String,
}

impl DeliveryInfo {
    /// Whether no attempt follows this one.
    pub fn is_last(&self) -> bool {
        self.attempt >= self.max_attempts
    }
}

/// What [`begin`] found.
// One per attempt, matched at once and never stored: boxing the envelope
// would buy nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum DeliveryStart {
    /// An attempt is open: run the listener with `envelope` as its body.
    Run {
        envelope: OxyEvent,
        info: DeliveryInfo,
        /// Who a send's sending run ran as (`Records::invoker`): the delivery
        /// runs as that person, or is refused. `None`: as the system.
        invoker: Option<super::provenance::Invoker>,
    },
    /// Not pending (acked, dead, held, discarded, expired) or gone: nothing to
    /// run, and nothing to record.
    Skip { reason: String },
    /// Pending, but no attempt can be made: the caller kills it with
    /// [`finish_dead`], closing `attempt` when one was opened. One is when the
    /// event's body turns out to be gone or unreadable (it cannot be decoded,
    /// or the platform does not relay its environment's store), which is read
    /// only after opening.
    Dead {
        reason: String,
        attempt: Option<i32>,
    },
    /// The attempt opened failed before the listener ran: the event's body
    /// could not be read this time (the org's store could not be reached,
    /// say). [`begin`] has closed the attempt as `error` with `reason`, and
    /// killed the delivery when that was its last attempt; the caller fails
    /// the task, so the queue retries it while attempts remain.
    Retry { reason: String },
}

/// Lock the delivery, skip it unless it is pending, close an attempt an
/// earlier run left open as `crash`, then open attempt n + 1 and count it, in
/// one transaction; then, the lock released, read the event's body from the
/// org's store.
pub async fn begin(db: &DatabaseConnection, delivery_id: Uuid) -> Result<DeliveryStart, DbErr> {
    let txn = db.begin().await?;
    let opened = open_attempt(&txn, delivery_id).await?;
    txn.commit().await?;
    let (records, info) = match opened {
        Opened::Attempt { records, info } => (records, info),
        Opened::Not(start) => return Ok(start),
    };
    let reason = match load_event_data(db, &records.event).await {
        Ok(Some(data)) => {
            let invoker = records.invoker();
            return Ok(DeliveryStart::Run {
                envelope: records.envelope(data),
                info,
                invoker,
            });
        }
        Ok(None) => BODY_GONE.to_string(),
        Err(BodyError::Permanent(reason)) => reason,
        Err(transient @ BodyError::Transient(_)) => {
            // The attempt is open: close it as what happened, rather than
            // leave the next run to guess a crash. On the last attempt no
            // retry follows, so the delivery is dead with this error rather
            // than left pending with nothing queued.
            let reason = transient.to_string();
            let closed = if info.is_last() {
                let (attempt, error) = (Some(info.attempt), AttemptOutcome::Error);
                finish_dead(db, delivery_id, attempt, None, error, &reason).await
            } else {
                let error = AttemptOutcome::Error;
                finish_err(db, delivery_id, info.attempt, None, error, &reason).await
            };
            if let Err(close) = closed {
                tracing::warn!(%delivery_id, error = %close, "app events: an attempt was left open");
            }
            return Ok(DeliveryStart::Retry { reason });
        }
    };
    Ok(DeliveryStart::Dead {
        reason,
        attempt: Some(info.attempt),
    })
}

/// What [`open_attempt`] did.
enum Opened {
    /// Attempt `info.attempt` is open, and `records` build its envelope.
    Attempt {
        records: Records,
        info: DeliveryInfo,
    },
    /// No attempt was opened: [`begin`]'s answer as it is.
    Not(DeliveryStart),
}

/// [`begin`]'s locked part, in `txn`.
async fn open_attempt(txn: &DatabaseTransaction, delivery_id: Uuid) -> Result<Opened, DbErr> {
    let Some(delivery) = lock(txn, delivery_id).await? else {
        return Ok(skip(format!("delivery {delivery_id} no longer exists")));
    };
    if delivery.status != PENDING {
        return Ok(skip(format!("the delivery is {}", delivery.status)));
    }
    let Some(records) = Records::read(txn, &delivery).await? else {
        return Ok(skip(
            "its event, its listener or its publisher is gone".into(),
        ));
    };
    // A redelivery restarts the budget: the attempts made before it, plus
    // the listener's full `maxAttempts` (`budget_base`).
    let max_attempts = budget_base(txn, delivery.id)
        .await?
        .saturating_add(max_attempts(txn, delivery.run_id.as_deref()).await?);
    let now = Utc::now().fixed_offset();
    let crashed = close_unfinished(txn, delivery.id, now).await? > 0;
    if let Some(reason) = spent(delivery.attempts, max_attempts, crashed) {
        record_begin(txn, delivery.id, crashed, None, now).await?;
        return Ok(Opened::Not(DeliveryStart::Dead {
            reason,
            attempt: None,
        }));
    }
    let attempt = delivery.attempts + 1;
    record_begin(txn, delivery.id, crashed, Some(attempt), now).await?;
    Ok(Opened::Attempt {
        info: DeliveryInfo {
            attempt,
            max_attempts,
            listener: format!("{}/{}", records.listener, delivery.function_name),
        },
        records,
    })
}

/// The delivery `delivery_id`, its row locked for `txn`.
async fn lock(
    txn: &DatabaseTransaction,
    delivery_id: Uuid,
) -> Result<Option<deliveries::Model>, DbErr> {
    deliveries::Entity::find_by_id(delivery_id)
        .lock_exclusive()
        .one(txn)
        .await
}

/// Close every attempt of `delivery_id` still open as a `crash`; how many.
async fn close_unfinished(
    txn: &DatabaseTransaction,
    delivery_id: Uuid,
    now: DateTimeWithTimeZone,
) -> Result<u64, DbErr> {
    Ok(attempts::Entity::update_many()
        .col_expr(attempts::Column::FinishedAt, Expr::value(now))
        .col_expr(
            attempts::Column::Outcome,
            Expr::value(AttemptOutcome::Crash.as_str()),
        )
        .col_expr(attempts::Column::Error, Expr::value(cap_error(CRASHED)))
        .filter(attempts::Column::DeliveryId.eq(delivery_id))
        .filter(attempts::Column::FinishedAt.is_null())
        .exec(txn)
        .await?
        .rows_affected)
}

/// Why no attempt follows `made` of `max_attempts`, when none does.
fn spent(made: i32, max_attempts: i32, crashed: bool) -> Option<String> {
    (made >= max_attempts).then(|| {
        if crashed {
            format!("attempt {made} of {max_attempts} did not finish, and no attempt is left")
        } else {
            format!("all {max_attempts} attempts are spent")
        }
    })
}

/// Open attempt `opened` when there is one and count it, and note a crash
/// closed as the delivery's last error.
async fn record_begin(
    txn: &DatabaseTransaction,
    delivery_id: Uuid,
    crashed: bool,
    opened: Option<i32>,
    now: DateTimeWithTimeZone,
) -> Result<(), DbErr> {
    let mut update = deliveries::ActiveModel {
        id: Set(delivery_id),
        updated_at: Set(now),
        ..Default::default()
    };
    if crashed {
        update.last_error = Set(Some(cap_error(CRASHED)));
    }
    if let Some(attempt) = opened {
        attempts::ActiveModel {
            id: Set(Uuid::new_v4()),
            delivery_id: Set(delivery_id),
            attempt: Set(attempt),
            started_at: Set(now),
            finished_at: Set(None),
            outcome: Set(None),
            error: Set(None),
            invocation_id: Set(None),
        }
        .insert(txn)
        .await?;
        update.attempts = Set(attempt);
    }
    update.update(txn).await?;
    Ok(())
}

fn skip(reason: String) -> Opened {
    Opened::Not(DeliveryStart::Skip { reason })
}

/// The attempts `delivery_id` had made when it was last redelivered (0 when
/// it never was), which its budget restarts from: a redelivered delivery may
/// make that many plus its listener's `maxAttempts`, so a dead delivery
/// redelivered runs again with every past attempt kept
/// (`app_event_delivery_budgets`, written by the staff redelivery).
async fn budget_base(txn: &DatabaseTransaction, delivery_id: Uuid) -> Result<i32, DbErr> {
    let row = txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT attempts_before FROM app_event_delivery_budgets WHERE delivery_id = $1",
            [delivery_id.into()],
        ))
        .await?;
    Ok(row
        .map(|r| r.try_get::<i32>("", "attempts_before"))
        .transpose()?
        .unwrap_or(0))
}

/// The attempts the queue makes of the delivery's task: one, plus the
/// retries of the policy stored on its row. One when there is no row or no
/// retry policy: the queue retries nothing then.
async fn max_attempts(txn: &DatabaseTransaction, run_id: Option<&str>) -> Result<i32, DbErr> {
    let Some(run_id) = run_id else {
        return Ok(1);
    };
    let policy: Option<Value> = txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT policy FROM agentic_task_queue WHERE task_id = $1",
            [run_id.into()],
        ))
        .await?
        .map(|row| row.try_get::<Option<Value>>("", "policy"))
        .transpose()?
        .flatten();
    let retries = policy
        .and_then(|p| serde_json::from_value::<TaskPolicy>(p).ok())
        .and_then(|p| p.retry)
        .map_or(0, |r| r.max_retries);
    Ok(i32::try_from(retries).unwrap_or(i32::MAX - 1) + 1)
}

#[cfg(test)]
mod tests;
