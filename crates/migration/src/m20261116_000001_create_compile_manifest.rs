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
            -- The content-addressed half of the compile boundary
            -- (internal-docs/compile-performance.md § 6).
            --
            -- `compiled_sources` holds a file's text ONCE per workspace, keyed
            -- by the SHA-256 of that text, however many revisions carry it.
            -- Only the kinds whose text a reader needs are stored: the semantic
            -- views and topics, which the semantic scan parses straight from
            -- the pushed text instead of from a per-row S3 GET. `config.yml` is
            -- never stored here: the compile strips its inline secrets, and the
            -- raw text would put them back.
            --
            -- `last_seen_at` is touched by every compile that carries the text,
            -- so the sweep that deletes unreferenced text (`oxy_compile::writer
            -- ::prune_unreferenced_sources`) cannot race a compile that is about
            -- to reference it.
            CREATE TABLE IF NOT EXISTS compiled_sources (
                workspace_id    UUID NOT NULL
                    REFERENCES workspaces(id) ON DELETE CASCADE,
                content_sha256  TEXT NOT NULL,
                content         TEXT NOT NULL,
                byte_size       INTEGER NOT NULL,
                last_seen_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (workspace_id, content_sha256)
            );

            -- What a revision is made of: one row per compiled file, naming
            -- its kind and the hash of its text. The git tree of a revision.
            -- Every compiled file is listed, whether or not its text is kept in
            -- `compiled_sources`.
            CREATE TABLE IF NOT EXISTS revision_manifest (
                revision_id     UUID NOT NULL
                    REFERENCES revisions(revision_id) ON DELETE CASCADE,
                file_path       TEXT NOT NULL,
                kind            TEXT NOT NULL,
                workspace_id    UUID NOT NULL,
                content_sha256  TEXT NOT NULL,
                PRIMARY KEY (revision_id, file_path)
            );
            -- The sweep's anti-join.
            CREATE INDEX IF NOT EXISTS idx_revision_manifest_content
                ON revision_manifest (workspace_id, content_sha256);

            -- One hash per kind per revision, over its sorted
            -- (file_path, content_sha256) entries: "did this kind change"
            -- without reading a body. `validated` is the compile's verdict on
            -- the semantic model (`kind = 'semantic'` only; NULL elsewhere, and
            -- NULL when nothing checked it). It is the baseline the next
            -- compile's semantic gate ratchets against.
            CREATE TABLE IF NOT EXISTS revision_fingerprints (
                revision_id     UUID NOT NULL
                    REFERENCES revisions(revision_id) ON DELETE CASCADE,
                kind            TEXT NOT NULL,
                fingerprint     TEXT NOT NULL,
                file_count      INTEGER NOT NULL,
                validated       BOOLEAN,
                PRIMARY KEY (revision_id, kind)
            );

            -- Additive only: a binary rolled back past this migration neither
            -- reads nor writes these tables and is unaffected by them.
        "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "DROP TABLE IF EXISTS revision_fingerprints CASCADE;
                 DROP TABLE IF EXISTS revision_manifest CASCADE;
                 DROP TABLE IF EXISTS compiled_sources CASCADE;",
            )
            .await?;
        Ok(())
    }
}
