//! Somebody asked to hear when a run ends.
//!
//! An answer takes long enough that people leave the app meanwhile, so a client
//! asks for a notice while the run is going (`POST …/runs/{id}/notify`). The
//! request is `agentic_runs.notify_requested_at`; nothing is sent by default.
//! The host's run observers ([`super::done_hook`], [`super::failure_hook`])
//! read it when the run ends and decide whom to tell, and
//! [`super::suspend_hook`]'s when the run stops to ask the person something.
//!
//! **The race with the end of the run is closed by the row lock.** The request
//! is an `UPDATE` of the run row that returns the status it left; the run's
//! end is an `UPDATE` of the same row followed by the observer, which reads
//! the request. Whichever writes second waits for the first to commit, so
//! either the request sees the run ended (and tells the host itself) or the
//! observer sees the request. Both can happen; the host sends at most once per
//! run.
//!
//! **A request that finds the run ended has its own port**,
//! [`NoticeRequestObserver`], not the run observers. Those announce a run's
//! end to whoever it concerns — a failed automation's starter or admins, by
//! email and Slack too — and are meant to fire once, when the run ends. A
//! request can arrive at any time after that, from the owner of the run's
//! thread, and may cause only the notice that person asked for.
//!
//! The column is read and written here only, by statement, and is not mapped on
//! the [`super::entity::run`] entity: mapping it would put a field into every
//! `run::Model` literal in four crates for a flag two queries touch.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};

/// What the host does when someone asks to hear about a run that has already
/// ended.
#[async_trait]
pub trait NoticeRequestObserver: Send + Sync {
    /// Called after the request is recorded, for a root run already `done`,
    /// `failed` or `timed_out`. Send the notice that was asked for and nothing
    /// else. Must not fail the caller: whatever goes wrong is the observer's
    /// to log.
    async fn requested_after_end(&self, run_id: &str);
}

static OBSERVER: OnceLock<Arc<dyn NoticeRequestObserver>> = OnceLock::new();

/// Install the observer. First call wins; the host does this once at boot.
pub fn set_notice_request_observer(observer: Arc<dyn NoticeRequestObserver>) {
    if OBSERVER.set(observer).is_err() {
        tracing::warn!("notice request observer already installed; ignoring the second");
    }
}

/// Record that someone wants to hear when run `run_id` ends. When it already
/// has, tell the [`NoticeRequestObserver`] now. Asking twice keeps the first
/// request's time.
///
/// `Ok(false)` when there is no such run.
pub async fn request_notice(db: &DatabaseConnection, run_id: &str) -> Result<bool, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_runs \
                SET notify_requested_at = COALESCE(notify_requested_at, now()) \
              WHERE id = $1 \
          RETURNING task_status, parent_run_id IS NULL AS is_root",
            [run_id.into()],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let status: Option<String> = row.try_get("", "task_status")?;
    let is_root: bool = row.try_get("", "is_root")?;
    let ended = matches!(status.as_deref(), Some("done" | "failed" | "timed_out"));
    if is_root
        && ended
        && let Some(observer) = OBSERVER.get()
    {
        observer.requested_after_end(run_id).await;
    }
    Ok(true)
}

/// Whether anyone asked to hear when run `run_id` ends. `false` for a run that
/// does not exist.
pub async fn notice_requested(db: &impl ConnectionTrait, run_id: &str) -> Result<bool, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT notify_requested_at IS NOT NULL AS requested FROM agentic_runs WHERE id = $1",
            [run_id.into()],
        ))
        .await?;
    match row {
        Some(row) => row.try_get("", "requested"),
        None => Ok(false),
    }
}

/// When someone first asked to hear about run `run_id`; `None` when nobody has,
/// or there is no such run. A suspension that began before this was seen by the
/// person who asked, so the host tells them only of a later one.
pub async fn notice_requested_at(
    db: &impl ConnectionTrait,
    run_id: &str,
) -> Result<Option<DateTimeWithTimeZone>, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT notify_requested_at FROM agentic_runs WHERE id = $1",
            [run_id.into()],
        ))
        .await?;
    match row {
        Some(row) => row.try_get("", "notify_requested_at"),
        None => Ok(None),
    }
}
