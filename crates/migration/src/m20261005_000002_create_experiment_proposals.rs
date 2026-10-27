//! Creates `experiment_proposals`, the pricing proposal log.
//!
//! Additive and idempotent (`IF NOT EXISTS`), so it is revert-safe under
//! `internal-docs/revert-safe-migrations.md`: the previous binary never reads
//! the table, and a roll-forward that finds no ledger row re-runs it harmlessly.
//! No foreign keys: a proposal is evidence and must outlive the definition or
//! workspace revision it priced.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS experiment_proposals (\
                id UUID PRIMARY KEY, \
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
                workspace_id UUID NOT NULL, \
                name TEXT NOT NULL, \
                revision_id UUID NOT NULL, \
                request_id UUID NOT NULL, \
                status TEXT NOT NULL, \
                request JSONB NOT NULL, \
                alpha DOUBLE PRECISION NOT NULL, \
                seed BIGINT NULL, \
                waves JSONB NULL, \
                periods JSONB NULL, \
                strata JSONB NULL, \
                assign_by TEXT NULL, \
                power JSONB NULL, \
                mde_unit_day DOUBLE PRECISION NULL, \
                prior_coefficient JSONB NULL, \
                warnings JSONB NOT NULL DEFAULT '[]'::jsonb, \
                refusal TEXT NULL, \
                requested_by UUID NOT NULL, \
                finished_at TIMESTAMPTZ NULL\
            )",
        )
        .await?;
        // Every read is "this workspace's proposals for this experiment, newest
        // first" — and the workspace filter is a correctness invariant.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_experiment_proposals_workspace_name \
             ON experiment_proposals (workspace_id, name, created_at DESC)",
        )
        .await?;
        // The candidates of one request are read and counted together.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_experiment_proposals_request \
             ON experiment_proposals (request_id)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_experiment_proposals_request")
            .await?;
        db.execute_unprepared("DROP INDEX IF EXISTS idx_experiment_proposals_workspace_name")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS experiment_proposals")
            .await?;
        Ok(())
    }
}
