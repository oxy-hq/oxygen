//! The migration ledger records which **target** a file ran against.
//!
//! `custom_app_migrations` is keyed `(app_id, store, filename)`. Custom-app
//! environments (`internal-docs/2026-09-10-custom-app-environments-design.md`
//! §4.3) apply an app's migrations to more than one database per store: the
//! production schema, and a staging OLTP branch or Airhouse schema. Keyed
//! without the target, staging's applied `0002_add_column.sql` would read as
//! applied everywhere, and the promote that followed would skip production's
//! DDL. So `target` joins the key: `production`, `branch:<id>` for `oltp`, or
//! `schema:<name>` for `airhouse`. Every existing row ran against production.
//!
//! **Rollback-safe.** The previous deploy never writes `target`, so its rows
//! take `DEFAULT 'production'`; among production rows the wider key rejects
//! exactly the `(app_id, store, filename)` duplicates the old one did, and that
//! deploy writes the ledger with plain `INSERT`s (no `ON CONFLICT` naming the
//! old key). Its ledger reads, which do not filter on `target`, see the same
//! rows as before because nothing yet writes another target.
//! `migration_rollback_safety` flags the redefined primary key and carries this
//! argument as its declaration.
//!
//! No `CHECK` on the values: adding one to a table the previous deploy writes
//! is a contraction by that guard's rule, and the one writer builds the value
//! from a typed target (`custom_apps_migrations::MigrationTarget`).

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
                ADD COLUMN IF NOT EXISTS target TEXT NOT NULL DEFAULT 'production';
            ALTER TABLE custom_app_migrations DROP CONSTRAINT IF EXISTS custom_app_migrations_pkey;
            ALTER TABLE custom_app_migrations ADD PRIMARY KEY (app_id, store, target, filename);
        "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Rows for another target cannot survive the narrower key: their
        // filenames collide with production's. Dropping them means the next
        // apply to that target re-runs those files, which fails loudly on
        // `already exists` rather than silently.
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            DELETE FROM custom_app_migrations WHERE target <> 'production';
            ALTER TABLE custom_app_migrations DROP CONSTRAINT IF EXISTS custom_app_migrations_pkey;
            ALTER TABLE custom_app_migrations ADD PRIMARY KEY (app_id, store, filename);
            ALTER TABLE custom_app_migrations DROP COLUMN IF EXISTS target;
        "#,
            )
            .await?;
        Ok(())
    }
}
