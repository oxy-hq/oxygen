//! `workspace_previews` — which branches of a workspace Oxy staff are
//! previewing (`server::previews`).
//!
//! Deliberately thin. A preview IS a staging revision (`revisions`, `kind =
//! 'staging'`), which already records its branch and SHA; this row holds only
//! what no revision can: that staff asked to preview the branch (a revision may
//! just as well have been compiled for `oxyc publish --semantic-branch`, or be a
//! reused main revision of the same SHA), who asked, the commit last asked for
//! (a queued compile has no revision row yet), and that a DELETE stopped the
//! listing while retention still owns the revisions.
//!
//! Purely additive: a new table nothing older reads or writes.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS workspace_previews (
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    branch TEXT NOT NULL,
    git_sha TEXT NOT NULL,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, branch)
);
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
            .execute_unprepared("DROP TABLE IF EXISTS workspace_previews;")
            .await?;
        Ok(())
    }
}
