//! The org's home layout: which sections the Oxygen home draws, in what order,
//! and which app widgets at what size. `internal-docs/home-widgets.md`.
//!
//! Expand-only: one new table. A binary one release back never reads it, so a
//! revert draws the default home and the stored layout waits, untouched, for
//! the next release.
//!
//! One row per org (`org_id` is the key). `revision` counts writes, for the
//! optimistic check every write makes. Deleting the org deletes its layout;
//! deleting the user who wrote it only forgets who did (`updated_by` is audit).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS org_home_layouts (
    org_id UUID PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
    layout JSONB NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1,
    updated_by UUID REFERENCES users(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS org_home_layouts;
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
