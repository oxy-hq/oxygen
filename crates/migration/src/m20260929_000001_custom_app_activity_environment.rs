//! The app environment on every custom-app activity row
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §3.4).
//!
//! Staging hosts now serve staging HTML, so a staff member opening one records
//! views (`custom_app_view_event`) and platform beacons (`custom_app_event`),
//! and later phases write audit rows (`audit_events`) from non-production
//! functions. Each table gains `environment`, and the Activity tab counts
//! production only. `app_function_invocations` took the same column in
//! `m20260928_000003`.
//!
//! **Additive and cheap.** A constant `DEFAULT 'production'` is recorded in the
//! catalog, not written into each row, so none of these tables is rewritten;
//! every existing row reads `production`, which it was. It is DDL, so
//! `audit_events`' append-only trigger (`m20260908_000001`) does not fire. The
//! previous deploy never names the column and its inserts take the default.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const TABLES: [&str; 3] = ["custom_app_event", "custom_app_view_event", "audit_events"];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for table in TABLES {
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} \
                 ADD COLUMN IF NOT EXISTS environment TEXT NOT NULL DEFAULT 'production'"
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        for table in TABLES {
            db.execute_unprepared(&format!(
                "ALTER TABLE {table} DROP COLUMN IF EXISTS environment"
            ))
            .await?;
        }
        Ok(())
    }
}
