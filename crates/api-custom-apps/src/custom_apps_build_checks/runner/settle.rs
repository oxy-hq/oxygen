//! `POST …/checks/functions/settle`: where the `functions` check stands, and
//! its verdict once it has one.
//!
//! The runner polls this while `functions` is open. Each call reads the runs a
//! claim started (`functions::readings`: what the worker's own polling loop
//! read each tick, before the runner drove the checks). While one is still
//! going, and less than `functions::FUNCTIONS_DEADLINE` has passed since the
//! check started, it answers `{done: false}`; otherwise it judges and scores
//! them, writes the verdict unless the row already has one, and answers what
//! the row holds.

use chrono::Utc;
use entity::{app_build_checks, app_builds, apps};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use super::super::environment::CheckEnvironment;
use super::super::error::ChecksError;
use super::super::functions::{self, FUNCTIONS_DEADLINE, Settled};
use super::super::kind::{CheckKind, CheckStatus, Outcome};
use super::super::store::{self, AttemptKey};
use super::super::{settled, stalled};

#[derive(Debug, Default, Deserialize)]
pub struct SettleRequest {
    /// The attempt; the newest when absent or null.
    #[serde(default)]
    pub attempt: Option<i32>,
    /// `staging` or `production`; staging when absent or null. A production
    /// attempt has no `functions` check, so settling one is not-found.
    #[serde(default)]
    pub environment: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct SettleAnswer {
    /// Whether the check has its verdict.
    pub done: bool,
    /// The row's status: `queued` or `running`, then the verdict's.
    pub status: String,
    pub summary: String,
}

impl SettleAnswer {
    fn waiting(row: &app_build_checks::Model, summary: impl Into<String>) -> Self {
        Self {
            done: false,
            status: row.status.clone(),
            summary: summary.into(),
        }
    }

    fn of(row: app_build_checks::Model) -> Self {
        Self {
            done: !stalled::is_open(&row),
            status: row.status,
            summary: row.summary.unwrap_or_default(),
        }
    }
}

async fn functions_row(
    db: &DatabaseConnection,
    key: &AttemptKey,
) -> Result<app_build_checks::Model, ChecksError> {
    store::rows_of(db, key)
        .await?
        .into_iter()
        .find(|row| row.kind == CheckKind::Functions.as_str())
        .ok_or(ChecksError::NotFound)
}

/// How long ago the check started, never negative.
fn elapsed(row: &app_build_checks::Model) -> std::time::Duration {
    let since = row.started_at.unwrap_or(row.created_at);
    (Utc::now() - since.with_timezone(&Utc))
        .to_std()
        .unwrap_or_default()
}

/// Settle the `functions` check of `request`'s attempt of `build` in
/// `environment`.
pub async fn settle(
    db: &DatabaseConnection,
    (app, build): (&apps::Model, &app_builds::Model),
    request: &SettleRequest,
    environment: CheckEnvironment,
) -> Result<SettleAnswer, ChecksError> {
    let key = super::attempt_of(db, build, environment, request.attempt).await?;
    let row = functions_row(db, &key).await?;
    if !stalled::is_open(&row) {
        return Ok(SettleAnswer::of(row));
    }
    let took = elapsed(&row);
    let verdict = match functions::pending_of(row.detail.as_ref()) {
        Some(started) => {
            let readings = functions::readings(db, &started).await;
            match functions::settle(&started, &readings, took, build.id) {
                Settled::Done(verdict) => verdict,
                Settled::Running { done, total } => {
                    return Ok(SettleAnswer::waiting(
                        &row,
                        format!("{done} of {total} done"),
                    ));
                }
            }
        }
        // No claim has started them: a `queued` row waits for its claim. A
        // row that says `running` without them past the deadline never will
        // have them.
        None if row.status == CheckStatus::Queued.as_str() || took < FUNCTIONS_DEADLINE => {
            let why = "the check's functions have not been started; claim the attempt first";
            return Ok(SettleAnswer::waiting(&row, why));
        }
        None => {
            Outcome::error("the build's check functions were never started; run the checks again")
        }
    };
    // Under the attempt's lock: the write that settles the attempt hands it on.
    let finished = store::finish_all(db, &key, &[(CheckKind::Functions, verdict)]).await?;
    if finished.settled {
        settled::on_staging_attempt_settled(db, app, build, &key).await;
    }
    // Whoever wrote it, the row's verdict is the answer.
    Ok(SettleAnswer::of(functions_row(db, &key).await?))
}
