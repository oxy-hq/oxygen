//! `fleet_job_claims` — when each fleet-wide process-lifetime job is next due,
//! and, for a once-per-key job, which key it last ran for.
//!
//! One row per job (`oxy_app_core::fleet_claim`). Every `oxy worker` replica
//! starts the jobs that used to run only on the singleton `ide` pod (the
//! custom-app storage sweeper, the usage-backfill sweep, the asset-bucket
//! lifecycle check, the audit chain-head anchor); the claim is one upsert whose
//! `WHERE` tests the row the winner left, so each job runs once per interval
//! across the fleet without a leader.
//!
//! Control plane only: job names, timestamps, a host name and a build version.
//! Purely additive — a new table nothing older reads or writes, so a reverted
//! binary ignores it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS fleet_job_claims (
    job TEXT PRIMARY KEY,
    next_run_at TIMESTAMPTZ NOT NULL,
    claimed_at TIMESTAMPTZ NOT NULL,
    claimed_by TEXT,
    claim_key TEXT
);
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS fleet_job_claims;")
            .await?;
        Ok(())
    }
}
