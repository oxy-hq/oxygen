//! Adds two columns to `customer_app_automation_runs`: `execution_started_at`,
//! the moment a driver began executing the run's first step, and
//! `execution_heartbeat_at`, that driver's proof it is still executing.
//!
//! A custom-app procedure run is a queued task, and a queued task whose driver
//! dies is handed to another driver. The inline automation runner keeps no
//! checkpoint, so a second attempt would start from the first step and repeat
//! every side effect the first one finished (`http_request`, a write, an
//! email). `execution_started_at` is the durable fact that lets the second
//! attempt refuse: it is stamped by one atomic `UPDATE … WHERE
//! execution_started_at IS NULL` immediately before the runner starts, and an
//! attempt that finds it already set does not run the automation again.
//!
//! The stamp alone cannot say whether the attempt that wrote it is dead or
//! still running — a claim can be handed on under a live driver. So the
//! executing attempt re-stamps `execution_heartbeat_at` on an interval, and a
//! later attempt closes the run as interrupted only once that has gone stale;
//! while it is fresh it steps aside and leaves the run to its driver.
//!
//! Why columns, and not the queue's `claim_count`: a graceful release
//! (`release_claims_for_worker`) *decrements* the count, so a run that began
//! on a pod the rollout replaced comes back looking like a first attempt; and
//! the count moves at claim time, before anything ran, so a claim that failed
//! in `prepare` would count as "began". Nor the `procedure_run_started` event:
//! it travels a channel and is persisted after the runner is already going.
//! Nor the queue row's own heartbeat for liveness: handing the claim on clears
//! it, which is exactly the case that needs answering.
//!
//! Internal: the poll endpoint never serves either (`progress_*`, `result_*`
//! and `error_*` are what the bundle sees). Both nullable with no default, so
//! a binary from before this migration, which names its columns, keeps
//! inserting. Guarded with `has_column` like the neighbouring additive
//! migrations.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum AutomationRuns {
    #[sea_orm(iden = "customer_app_automation_runs")]
    Table,
    ExecutionStartedAt,
    ExecutionHeartbeatAt,
}

const TABLE: &str = "customer_app_automation_runs";
const STARTED: &str = "execution_started_at";
const HEARTBEAT: &str = "execution_heartbeat_at";

/// Both columns, as `(name, iden)`: each is added and dropped the same way.
fn columns() -> [(&'static str, AutomationRuns); 2] {
    [
        (STARTED, AutomationRuns::ExecutionStartedAt),
        (HEARTBEAT, AutomationRuns::ExecutionHeartbeatAt),
    ]
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (name, column) in columns() {
            if !manager.has_column(TABLE, name).await? {
                manager
                    .alter_table(
                        Table::alter()
                            .table(AutomationRuns::Table)
                            .add_column(ColumnDef::new(column).timestamp_with_time_zone().null())
                            .to_owned(),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (name, column) in columns() {
            if manager.has_column(TABLE, name).await? {
                manager
                    .alter_table(
                        Table::alter()
                            .table(AutomationRuns::Table)
                            .drop_column(column)
                            .to_owned(),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}
