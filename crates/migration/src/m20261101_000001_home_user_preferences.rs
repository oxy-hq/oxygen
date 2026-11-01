//! A person's own choices about Oxygen's home: which widget copies they hid and
//! whether Today shows. `internal-docs/home-widgets.md`.
//!
//! Expand-only: one new table. A binary one release back never reads it, so a
//! revert draws everyone's home in full and the choices wait, untouched, for the
//! next release.
//!
//! One row per person per org, written lazily: no row means nothing hidden and
//! Today shown. `hidden` is the array of copy keys (`<app>/<widget>` or
//! `<app>/<widget>#<id>`); a key an app has since dropped is harmless. Deleting
//! the user or the org deletes the row.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS home_user_preferences (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    hidden JSONB NOT NULL DEFAULT '[]'::jsonb,
    today_hidden BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, org_id)
);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS home_user_preferences;
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
