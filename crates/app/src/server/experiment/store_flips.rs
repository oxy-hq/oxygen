//! The check's write-once settlements: a flip's read-back outcome (with the
//! observations it stands for), and the control pool, group membership and
//! fleet the first check pins. Each is guarded so a second writer is a no-op;
//! `store` re-exports them.

use std::collections::BTreeMap;

use oxy_experiment::compliance::FlipOutcome;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use uuid::Uuid;

use super::panel::GroupMembership;

/// Record a read-back outcome, only from `dispatched` (a failed or already
/// settled flip keeps its outcome).
pub async fn settle_flip(
    db: &DatabaseConnection,
    flip_id: Uuid,
    status: &str,
    confirmed_on: Option<oxy_experiment::Moment>,
    reason: Option<&str>,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE experiment_flips SET status = $1, confirmed_on = $2, reason = $3 \
         WHERE id = $4 AND status = 'dispatched'",
        [
            status.into(),
            confirmed_on.map(|m| m.at()).into(),
            reason.map(str::to_string).into(),
            flip_id.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// The read-back outcomes a settled flip stands for, merged into `observations.<field>`
/// (`period_outcomes` or `member_outcomes`) by key. The check reads only `dispatched` rows,
/// so a flip settled without its outcomes would lose them for good.
pub struct Recorded<'a> {
    pub run_id: Uuid,
    pub field: &'static str,
    pub entries: BTreeMap<&'a str, &'a FlipOutcome>,
}

/// [`settle_flip`] and the merge of `rec` into the run's observations, in one statement:
/// a check that fails after it (a later settle, a warehouse read, a restart) cannot
/// leave the flip settled without the outcomes it was settled on.
pub async fn settle_flip_recorded(
    db: &DatabaseConnection,
    flip_id: Uuid,
    status: &str,
    confirmed_on: Option<oxy_experiment::Moment>,
    reason: Option<&str>,
    rec: &Recorded<'_>,
) -> Result<(), DbErr> {
    let entries = serde_json::to_value(&rec.entries).map_err(|e| DbErr::Custom(e.to_string()))?;
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "WITH settled AS (UPDATE experiment_flips SET status = $1, confirmed_on = $2, reason = $3 \
           WHERE id = $4 AND status = 'dispatched' RETURNING id) \
         UPDATE experiment_runs SET observations = jsonb_set(COALESCE(observations, '{}'::jsonb), \
           ARRAY[$5::text], COALESCE(observations -> $5::text, '{}'::jsonb) || $6::jsonb, true) \
         WHERE id = $7",
        [
            status.into(),
            confirmed_on.map(|m| m.at()).into(),
            reason.map(str::to_string).into(),
            flip_id.into(),
            rec.field.into(),
            entries.into(),
            rec.run_id.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Written once, together: the pool, and under `assign_by` every group's
/// members (C3). One statement, so the two can never disagree.
pub async fn pin_control_pool(
    db: &DatabaseConnection,
    run_id: Uuid,
    pool: &[String],
    membership: Option<&GroupMembership>,
) -> Result<bool, DbErr> {
    let membership = membership.map(|m| serde_json::json!(m));
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE experiment_runs SET control_pool = $1, group_membership = $2 \
             WHERE id = $3 AND control_pool IS NULL",
            [
                serde_json::json!(pool).into(),
                membership.into(),
                run_id.into(),
            ],
        ))
        .await?;
    Ok(result.rows_affected() == 1)
}

/// A switchback's fleet (C3 `fleet`), written once: an existing fleet wins.
pub async fn pin_fleet(
    db: &DatabaseConnection,
    run_id: Uuid,
    fleet: &[String],
) -> Result<bool, DbErr> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE experiment_runs SET fleet = $1 WHERE id = $2 AND fleet IS NULL",
            [serde_json::json!(fleet).into(), run_id.into()],
        ))
        .await?;
    Ok(result.rows_affected() == 1)
}
