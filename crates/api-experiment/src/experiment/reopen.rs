//! Reopening a design whose retrospective was refused.
//!
//! A retrospective's estimate can refuse — a unit with no rows, a window too
//! sparsely covered — and then there is no result, only the reason. Its design
//! froze when the run was created, so without this every fix would need a
//! duplicate. Instead the refused run (and its results row, flips and interims,
//! which cascade) is deleted and the design unfrozen, inside the caller's
//! transaction, so it can be edited and analysed again. A registered run, or a
//! retrospective that produced a result, is never reopened: those are records.

use entity::experiment_designs;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseBackend, DatabaseTransaction,
    DbErr, IntoActiveModel, Statement,
};
use uuid::Uuid;

/// The run whose results row says it was refused, when it is a retrospective.
const REFUSED_RETROSPECTIVE: &str = "SELECT r.id FROM experiment_runs r \
     JOIN experiment_results x ON x.run_id = r.id \
     WHERE r.experiment_id = $1 AND r.retrospective AND r.state = 'reported' \
       AND x.report->>'refusal' IS NOT NULL \
     LIMIT 1";

/// Whether `experiment`'s design would reopen on an edit or a new analysis: its
/// only run is a retrospective that was refused. The surface offers "Edit design"
/// for it, as for a draft.
pub async fn is_reopenable(db: &impl ConnectionTrait, experiment_id: Uuid) -> Result<bool, DbErr> {
    Ok(db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            REFUSED_RETROSPECTIVE,
            [experiment_id.into()],
        ))
        .await?
        .is_some())
}

/// Reopen the locked, frozen `row` when its only run is a refused retrospective:
/// `Some(row)` unfrozen, or `None` when there is nothing to reopen.
pub(super) async fn reopen_refused(
    txn: &DatabaseTransaction,
    experiment_id: Uuid,
    row: &experiment_designs::Model,
) -> Result<Option<experiment_designs::Model>, DbErr> {
    if row.frozen_at.is_none() {
        return Ok(None);
    }
    let stmt = |sql: &str, id: Uuid| {
        Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, [id.into()])
    };
    let Some(found) = txn
        .query_one_raw(stmt(REFUSED_RETROSPECTIVE, experiment_id))
        .await?
    else {
        return Ok(None);
    };
    let run_id: Uuid = found.try_get("", "id")?;
    txn.execute_raw(stmt(
        "DELETE FROM experiment_results WHERE run_id = $1",
        run_id,
    ))
    .await?;
    txn.execute_raw(stmt("DELETE FROM experiment_runs WHERE id = $1", run_id))
        .await?;
    let mut active = row.clone().into_active_model();
    active.frozen_at = Set(None);
    let row = active.update(txn).await?;
    tracing::info!(
        target: "experiment", %experiment_id, %run_id,
        "refused retrospective reopened: its run is deleted and the design unfrozen"
    );
    Ok(Some(row))
}
