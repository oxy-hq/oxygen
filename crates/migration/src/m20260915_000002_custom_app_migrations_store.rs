//! The migration ledger records which store a file ran against.
//!
//! `custom_app_migrations` was keyed `(app_id, filename)` while an app's OLTP
//! schema was the only store it could migrate. Airhouse migrations
//! (`airhouseMigrations` in `oxy-app.json`) follow the same ledger rules, and an
//! app may ship `0001_init.sql` for both stores, so the store joins the key.
//! Every existing row ran against OLTP.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            ALTER TABLE custom_app_migrations
                ADD COLUMN IF NOT EXISTS store TEXT NOT NULL DEFAULT 'oltp';
            ALTER TABLE custom_app_migrations
                DROP CONSTRAINT IF EXISTS custom_app_migrations_store_check;
            ALTER TABLE custom_app_migrations
                ADD CONSTRAINT custom_app_migrations_store_check
                CHECK (store IN ('oltp', 'airhouse'));
            ALTER TABLE custom_app_migrations DROP CONSTRAINT IF EXISTS custom_app_migrations_pkey;
            ALTER TABLE custom_app_migrations ADD PRIMARY KEY (app_id, store, filename);
        "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Airhouse rows cannot survive the narrower key: their filenames may
        // collide with OLTP ones. Dropping them means the next promote re-runs
        // those files against Airhouse, which fails loudly on `already exists`
        // rather than silently.
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            DELETE FROM custom_app_migrations WHERE store <> 'oltp';
            ALTER TABLE custom_app_migrations DROP CONSTRAINT IF EXISTS custom_app_migrations_pkey;
            ALTER TABLE custom_app_migrations ADD PRIMARY KEY (app_id, filename);
            ALTER TABLE custom_app_migrations
                DROP CONSTRAINT IF EXISTS custom_app_migrations_store_check;
            ALTER TABLE custom_app_migrations DROP COLUMN IF EXISTS store;
        "#,
            )
            .await?;
        Ok(())
    }
}
