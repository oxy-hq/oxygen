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
            -- Compiled `.md` context documents: the markdown an analytics
            -- agent's `context:` globs reach, carried across the compile
            -- boundary so a run on a pod with no working copy reads the same
            -- documents as one on a node that has the files.
            --
            -- The body stays in the row. These are injected whole into an
            -- agent's prompt, so they are small by necessity (single-digit KB
            -- in practice), and the S3 round trip the semantic bodies take
            -- costs more than the row at that size.
            --
            -- Additive only, so a binary rolled back past this migration
            -- neither reads nor writes the table and is unaffected by it.
            CREATE TABLE IF NOT EXISTS context_document_definitions (
                revision_id     UUID NOT NULL
                    REFERENCES revisions(revision_id) ON DELETE CASCADE,
                file_path       TEXT NOT NULL,
                content_sha256  TEXT NOT NULL,
                content         TEXT NOT NULL,
                PRIMARY KEY (revision_id, file_path)
            );
        "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS context_document_definitions CASCADE")
            .await?;
        Ok(())
    }
}
