//! The `experiment_runs` writes the tick and the check race on: the claim, the
//! state transition and the saved observations. Each is a compare-and-swap (or
//! a sticky `COALESCE`), stated here once; `store` re-exports them.

use chrono::{DateTime, Utc};
use entity::experiment_runs;
use oxy_experiment::run::{AbortLevers, RunState};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

use super::store::{Observations, observations, run_by_id};

/// Claim `run` until `until`. Only the replica whose UPDATE still sees the
/// `next_check_at` it read advances the run — `tick_schedules`' CAS on
/// `next_run_at`, and like it, no leader election.
pub async fn claim(
    db: &DatabaseConnection,
    run: &experiment_runs::Model,
    until: DateTime<Utc>,
) -> bool {
    match db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE experiment_runs SET next_check_at = $1 WHERE id = $2 AND next_check_at = $3",
            [
                until.fixed_offset().into(),
                run.id.into(),
                run.next_check_at.into(),
            ],
        ))
        .await
    {
        Ok(r) => r.rows_affected() == 1,
        Err(e) => {
            tracing::warn!(
                target: "experiment", run_id = %run.id, error = %e,
                "tick: claim failed; the run is retried on the next tick"
            );
            false
        }
    }
}

pub struct Persist {
    pub prev: RunState,
    pub next: RunState,
    /// Set only on the step into `aborted`: what the abort does with the levers.
    pub abort_levers: Option<AbortLevers>,
    /// Now in `design.timezone`, floored to the hour: `closing_since`, and (its
    /// date) `aborted_on`.
    pub local: oxy_experiment::Moment,
    pub next_check_at: DateTime<Utc>,
    pub claimed_until: DateTime<Utc>,
    pub now: DateTime<Utc>,
}

/// Write the transition, guarded on the state it was computed from. A
/// `next_check_at` someone pulled EARLIER than this tick's claim (an abort, a
/// breach) is kept; otherwise the next check is scheduled.
pub async fn persist(db: &DatabaseConnection, run_id: Uuid, p: &Persist) -> Result<bool, DbErr> {
    let sql = "UPDATE experiment_runs SET state = $1, \
         closing_since = CASE WHEN $1 = 'closing' THEN COALESCE(closing_since, $9) \
                              ELSE closing_since END, \
         aborted_on = CASE WHEN $1 = 'aborted' THEN COALESCE(aborted_on, $2) \
                           ELSE aborted_on END, \
         abort_levers = COALESCE($8, abort_levers), \
         next_check_at = CASE WHEN next_check_at < $3 THEN next_check_at ELSE $4 END, \
         updated_at = $5 \
       WHERE id = $6 AND state = $7 \
         AND ($1 <> 'closing' OR $7 <> 'live' OR abort_reason IS NULL)";
    let values: [sea_orm::Value; 9] = [
        p.next.as_str().into(),
        p.local.date().into(),
        p.claimed_until.fixed_offset().into(),
        p.next_check_at.fixed_offset().into(),
        p.now.fixed_offset().into(),
        run_id.into(),
        p.prev.as_str().into(),
        p.abort_levers.map(|l| l.as_str().to_string()).into(),
        p.local.at().into(),
    ];
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await?;
    Ok(result.rows_affected() == 1)
}

/// Prefix of the problem a check that could not run leaves on the run; the
/// next one replaces it, and a check that completes clears it.
pub const CHECK_FAILED: &str = "the last check failed: ";

/// A check that failed before it could save observations: say so in
/// `observations.problems`. Everything else recorded stays, a recorded breach
/// included, and `checked_at` does not move — a failure is not a check.
pub async fn record_check_failure(
    db: &DatabaseConnection,
    run_id: Uuid,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), DbErr> {
    let Some(run) = run_by_id(db, run_id).await? else {
        return Ok(());
    };
    let mut obs = observations(&run);
    obs.problems.retain(|p| !p.starts_with(CHECK_FAILED));
    obs.problems.push(format!("{CHECK_FAILED}{reason}"));
    save_observations(db, run_id, &obs, None, now).await
}

/// Save what a check saw. A breach is sticky (`COALESCE`) and pulls the next
/// tick to `now`, so the abort actuates within one driver interval.
pub async fn save_observations(
    db: &DatabaseConnection,
    run_id: Uuid,
    obs: &Observations,
    breach: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), DbErr> {
    let observations = serde_json::to_value(obs).map_err(|e| DbErr::Custom(e.to_string()))?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE experiment_runs SET observations = $1, \
           guardrail_breach = COALESCE(guardrail_breach, $2), \
           next_check_at = CASE WHEN $2::text IS NULL THEN next_check_at \
                                ELSE LEAST(next_check_at, $3) END, \
           updated_at = $3 \
         WHERE id = $4",
        [
            observations.into(),
            breach.map(str::to_string).into(),
            now.fixed_offset().into(),
            run_id.into(),
        ],
    ))
    .await?;
    Ok(())
}
