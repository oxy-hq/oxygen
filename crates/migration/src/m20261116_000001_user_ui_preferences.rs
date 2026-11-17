//! `user_ui_preferences`: a person's own choices about how Oxy draws its
//! interface, across every org they belong to. Today one: the light/dark
//! `color_mode` Oxygen and every custom app follow
//! (`entity::user_ui_preferences`).
//!
//! Expand-only: one new table. A binary one release back never reads it, so a
//! revert draws Oxygen from each browser's own copy (`theme-storage`) and apps
//! in their light default, and the choices wait, untouched, for the next
//! release. The `CHECK` sits on a table nothing older writes.
//!
//! One row per person, written lazily: no row, or a null `color_mode`, means
//! the person has not chosen, and Oxygen's default applies. Deleting the user
//! deletes the row.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS user_ui_preferences (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    color_mode TEXT NULL CHECK (color_mode IN ('light', 'dark', 'system')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS user_ui_preferences;
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
