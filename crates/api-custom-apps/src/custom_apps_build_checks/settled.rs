//! **The seam for what follows a staging attempt's verdicts**: the advisory
//! notes a model writes ([`super::ai`]) are queued here.
//!
//! [`on_staging_attempt_settled`] is called **once per staging attempt**, at
//! the moment it settles: every row that is not a model's has a verdict,
//! `changes` included (the runner writes `changes` after `smoke`, and the
//! notes read what it found, so the last non-advisory verdict is too early).
//! It runs only where the app's org has the `promote_checks_gate` flag on,
//! which is read when the attempt settles.
//!
//! **Once, by the lock, not by luck.** A verdict is written by one of three
//! runner routes (a claim, a settle, a report), each inside a transaction
//! that holds the attempt's rows (`store::finish_all`, and the claim's own).
//! Rows are never reopened, so exactly one of those transactions takes the
//! attempt from unsettled to settled, and only that one calls this
//! (`store::settled_by`). A report sent twice, two runners on one attempt and
//! a report racing a settle cannot both call it.
//!
//! **Not called for an attempt nobody judged.** An attempt closed because no
//! runner answered it (a rerun closing a stalled attempt as `error`, a runner
//! that could not be started) has no findings to write notes about, and those
//! closes do not go through the runner routes. Nor for a production attempt:
//! what follows that is `production`'s.
//!
//! **An attempt its runner left one advisory check short does settle**
//! ([`close_abandoned`]). A runner that reported its last check that counts
//! and died during `changes` leaves an attempt that was judged and will never
//! take another write from a runner route. Its next reader (`GET …/checks`)
//! closes the row (`error`, past the clock of `stalled::abandoned`) under
//! the attempt's lock, and the reader whose write settled the attempt hands
//! it on here, once, like any other.
//!
//! **What it queues** is one durable task (`ai::task`, kind
//! `custom_app_build_checks_ai`) whose run id is the attempt's, so a second
//! call would find the run the first wrote and queue nothing. It is queued
//! after the verdicts are committed, never with them: a task that cannot be
//! queued is logged, and the attempt stands with its verdicts and no notes.
//! Nothing waits on the task; a build's state and its promote read the same
//! with it queued, running, done or never queued.
//!
//! It is a plain `fn` that hands back a boxed future, so the runner route that
//! awaits it holds a pointer rather than the queueing's whole future (a debug
//! build reserves an awaited `async fn`'s future in the caller's poll frame).

use chrono::Utc;
use entity::{app_build_checks, app_builds, apps};
use futures::future::BoxFuture;
use sea_orm::{DatabaseConnection, DbErr};

use super::ai::task::{self, BuildChecksAiTask};
use super::environment::CheckEnvironment;
use super::store::{self, AttemptKey};
use super::{settings, stalled};

/// Close the advisory check a dead runner left open of one attempt of
/// `build` (module docs), and hand the attempt on when that settled it.
/// `rows` are the build's rows in `environment` as a reader just read them
/// (any attempt), `attempt` the one it shows, the newest when `None`. `true`
/// when the attempt was one to close: its rows are worth reading again.
pub async fn close_abandoned(
    db: &DatabaseConnection,
    (app, build): (&apps::Model, &app_builds::Model),
    environment: &str,
    rows: &[app_build_checks::Model],
    attempt: Option<i32>,
) -> Result<bool, DbErr> {
    let Some(attempt) = attempt.or_else(|| rows.iter().map(|row| row.attempt).max()) else {
        return Ok(false);
    };
    let shown: Vec<_> = rows
        .iter()
        .filter(|row| row.attempt == attempt)
        .cloned()
        .collect();
    let now = Utc::now();
    if stalled::abandoned(&shown, now).is_none() {
        return Ok(false);
    }
    let key = AttemptKey {
        build_pk: build.id,
        environment: environment.to_string(),
        attempt,
    };
    if store::close_abandoned(db, &key, now).await? {
        on_staging_attempt_settled(db, app, build, &key).await;
    }
    Ok(true)
}

/// A staging attempt of `build`'s checks has settled (module docs): queue its
/// advisory notes. Nothing for a production attempt, or where the gate is off
/// for the app's org.
pub fn on_staging_attempt_settled<'a>(
    db: &'a DatabaseConnection,
    app: &'a apps::Model,
    build: &'a app_builds::Model,
    key: &'a AttemptKey,
) -> BoxFuture<'a, ()> {
    Box::pin(queue_the_notes(db, app, build, key))
}

async fn queue_the_notes(
    db: &DatabaseConnection,
    app: &apps::Model,
    build: &app_builds::Model,
    key: &AttemptKey,
) {
    let staging = key.environment == CheckEnvironment::Staging.name();
    if !staging || !settings::enabled_for(app.org_id) {
        return;
    }
    let notes = BuildChecksAiTask::of(app, build, key.attempt);
    match task::enqueue(db, &notes).await {
        Ok(queued) => {
            tracing::info!(app_id = %app.id, build = %build.build_id, attempt = key.attempt,
                queued, "build checks: the staging attempt settled");
        }
        Err(e) => {
            tracing::error!(app_id = %app.id, build = %build.build_id, attempt = key.attempt,
                error = %e, "build checks: the attempt's advisory notes could not be queued");
        }
    }
}
