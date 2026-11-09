//! The owner's abort request (`POST /experiments/{name}/abort`). It records the
//! fact and nothing else: the tick actuates it. `status` re-exports it.

use chrono::{DateTime, Utc};
use oxy_experiment::run::AbortLevers;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

use super::store;

#[derive(Debug, PartialEq, Eq)]
pub enum AbortOutcome {
    Requested,
    NoRun,
    NotAbortable { state: String },
}

/// The owner's abort: why, and what to do with the levers.
pub struct OwnerAbort<'a> {
    pub reason: &'a str,
    pub levers: AbortLevers,
}

/// Record the owner's abort. The tick actuates it: this only sets the fact
/// (the first request wins, its lever choice with its reason) and pulls the next
/// check to now.
pub async fn request_abort(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
    req: OwnerAbort<'_>,
    now: DateTime<Utc>,
) -> Result<AbortOutcome, DbErr> {
    let changed = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE experiment_runs SET \
               abort_levers = CASE WHEN abort_reason IS NULL THEN $5 ELSE abort_levers END, \
               abort_reason = COALESCE(abort_reason, $1), \
               next_check_at = LEAST(next_check_at, $2), updated_at = $2 \
             WHERE workspace_id = $3 AND state IN ('registered', 'live') \
               AND experiment_id = (SELECT id FROM experiments WHERE workspace_id = $3 AND name = $4)",
            [
                req.reason.into(),
                now.fixed_offset().into(),
                workspace_id.into(),
                name.into(),
                req.levers.as_str().into(),
            ],
        ))
        .await?
        .rows_affected();
    if changed == 1 {
        return Ok(AbortOutcome::Requested);
    }
    Ok(match store::run_by_name(db, workspace_id, name).await? {
        None => AbortOutcome::NoRun,
        Some(run) => AbortOutcome::NotAbortable { state: run.state },
    })
}
