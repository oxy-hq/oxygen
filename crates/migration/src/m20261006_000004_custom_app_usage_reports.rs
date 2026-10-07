use sea_orm_migration::prelude::*;

/// The weekly custom-app usage report: what was written, who it was sent to,
/// and who asked not to be emailed.
///
/// # Tables
///
/// `custom_app_usage_reports` — one row per reported week. `period_start` is
/// unique, and that is what makes the report exactly-once: every node that
/// runs the weekly loop inserts with `ON CONFLICT DO NOTHING`, and the row
/// that lands is the one everyone delivers. `snapshot` holds counts and the
/// names of the orgs and apps they belong to — never what anyone did in an
/// app.
///
/// `custom_app_usage_report_deliveries` — one row per report per address. The
/// primary key is the claim: an address that already has a row for a report
/// is never emailed that report again, whichever node tries. A send that
/// fails deletes its row, so the next pass may try again.
///
/// `staff_notification_preferences` — a staff member's answer to "email me
/// this?", keyed by address because a Global Owner has no `app_admins` row to
/// hang it on. No row means the default, which is to send. `updated_by` is the
/// address that last changed it: an admin may switch the email off for
/// someone else, and that person should be able to see who did.
///
/// # Locking
///
/// Three new tables and nothing else; no existing table is read or altered.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS custom_app_usage_reports (\
               id UUID PRIMARY KEY, \
               period_start TIMESTAMPTZ NOT NULL UNIQUE, \
               period_end TIMESTAMPTZ NOT NULL, \
               snapshot JSONB NOT NULL, \
               created_at TIMESTAMPTZ NOT NULL DEFAULT now()\
             )",
        )
        .await?;
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS custom_app_usage_report_deliveries (\
               report_id UUID NOT NULL \
                 REFERENCES custom_app_usage_reports(id) ON DELETE CASCADE, \
               email TEXT NOT NULL, \
               claimed_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
               sent_at TIMESTAMPTZ, \
               PRIMARY KEY (report_id, email)\
             )",
        )
        .await?;
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS staff_notification_preferences (\
               email TEXT NOT NULL, \
               notification TEXT NOT NULL, \
               enabled BOOLEAN NOT NULL, \
               updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
               updated_by TEXT, \
               PRIMARY KEY (email, notification)\
             )",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS staff_notification_preferences")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS custom_app_usage_report_deliveries")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS custom_app_usage_reports")
            .await?;
        Ok(())
    }
}
