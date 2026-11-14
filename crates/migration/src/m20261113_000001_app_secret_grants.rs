//! `app_secret_grants`: a workspace secret shared with a custom app
//! (`entity::app_secret_grants`). One row per (secret, app); `ctx.env` reads
//! the secret under its own name.
//!
//! `secret_id` and `project_id` are loose UUIDs (domain-boundaries P3);
//! `app_id` cascades with its app, as the rest of the custom-apps aggregate does.
//!
//! Additive and revert-safe: a binary one release back never reads the table,
//! so its apps simply see no shared secrets.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS app_secret_grants (
    id UUID PRIMARY KEY,
    project_id UUID NOT NULL,
    secret_id UUID NOT NULL,
    app_id UUID NOT NULL REFERENCES apps (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by UUID,
    CONSTRAINT app_secret_grants_secret_app_key UNIQUE (secret_id, app_id)
);
CREATE INDEX IF NOT EXISTS app_secret_grants_app_id_idx ON app_secret_grants (app_id);
CREATE INDEX IF NOT EXISTS app_secret_grants_project_id_idx ON app_secret_grants (project_id);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS app_secret_grants;
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
