//! OLTP change stream generations and hold kinds (change stream design §5.5).
//!
//! - `oltp_change_generations`: the last generation handed to an (org,
//!   environment)'s store. It lives apart from `oltp_change_relays`, so a
//!   deleted relay row never lets a generation be handed out twice. A bump
//!   that timed out behind a long transaction backs off here
//!   (`bump_attempts`, `bump_retry_at`).
//! - `oltp_change_holds.kind`: why a hold was made (`table`, `ceiling`,
//!   `restore` or `target`), so a restore or resync lifts only its own. Holds
//!   made before the column are classified by their reason.
//!
//! Additive and revert-safe: a binary one release back never reads either.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS oltp_change_generations (
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    environment TEXT NOT NULL,
    generation BIGINT NOT NULL,
    bumped_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    bump_attempts INT NOT NULL DEFAULT 0,
    bump_retry_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, environment)
);

ALTER TABLE oltp_change_holds ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'table';
UPDATE oltp_change_holds SET kind = 'restore'
 WHERE table_name = '' AND reason LIKE 'restore detected (%';
UPDATE oltp_change_holds SET kind = 'ceiling'
 WHERE table_name = '' AND reason LIKE 'a transaction of % rows exceeds%';
UPDATE oltp_change_holds SET kind = 'target' WHERE table_name = '' AND kind = 'table';
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS oltp_change_generations;
ALTER TABLE oltp_change_holds DROP COLUMN IF EXISTS kind;
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
            .execute_unprepared(DOWN_SQL)
            .await?;
        Ok(())
    }
}
