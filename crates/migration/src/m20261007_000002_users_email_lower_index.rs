//! An index for finding a user by address without regard to case.
//!
//! `oxy_platform::filters` matches `lower(email) = lower($1)`, so that one
//! mailbox is one account whatever capitals it arrives in. Neither existing
//! index can serve that predicate — the unique one and `idx_users_email` are
//! both on the raw column — which left every lookup by address scanning the
//! table.
//!
//! Not unique, on purpose: rows that differ only by case already exist, and a
//! unique index cannot be created over them. Expand-only, and built without
//! `CONCURRENTLY` — `users` is small, and a concurrent build cannot run inside
//! the migration's transaction.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE INDEX IF NOT EXISTS idx_users_email_lower ON users (lower(email));
"#;

const DOWN_SQL: &str = r#"
DROP INDEX IF EXISTS idx_users_email_lower;
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
