//! Per-org targeting and A/B splits for feature flags: one nullable JSONB column,
//! `targeting`, holding the flag's rules (kill switch, user and org overrides, and
//! what everyone else gets: one variant or a weighted split).
//!
//! Expand-only, and safe for a binary one release back. That binary never reads the
//! column and reads `enabled` as the flag's value for every org. So while a flag has
//! rules, the new binary writes `enabled` as the flag's registry default, which is
//! what the old binary would read with no row at all: a revert sees a targeted flag
//! as untouched, never as on for everyone. `server::feature_flags::store` owns that
//! mapping.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
ALTER TABLE feature_flags ADD COLUMN IF NOT EXISTS targeting JSONB;
"#;

const DOWN_SQL: &str = r#"
ALTER TABLE feature_flags DROP COLUMN IF EXISTS targeting;
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
