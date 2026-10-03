use sea_orm_migration::prelude::*;

/// `app_environments.oltp_schema`: what a sandbox's own schema on the org's
/// OLTP staging branch is, as the platform last left it
/// (`internal-docs/per-org-oltp-postgres.md` → Sandbox schemas on the staging
/// branch).
///
/// A sandbox's `ctx.oltp` runs in a schema of its own inside the branch
/// database. The schema is created and seeded by a queued task, on a branch
/// that can be reset under it, so two readers need to know its state without
/// connecting to the tenant: a function's admission (ready, on the branch's
/// current cut — or refused) and `oxyc env show`. The column holds that state
/// as JSON: the schema's name, `seeding` / `ready` / `failed`, the branch cut
/// it was seeded on, and which tables the seed left empty.
///
/// Control-plane state about a schema Oxy made — no row of an org's data.
///
/// `NULL` for every existing row, for `production` and `staging` always, and
/// for a sandbox never published to in an org with a branch.
///
/// **Rollback-safe**: additive and nullable. The previous deploy never reads
/// or writes the column; its sandboxes keep sharing staging's schema.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE app_environments ADD COLUMN IF NOT EXISTS oltp_schema JSONB",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE app_environments DROP COLUMN IF EXISTS oltp_schema")
            .await?;
        Ok(())
    }
}
