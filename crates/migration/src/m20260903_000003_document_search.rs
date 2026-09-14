use sea_orm_migration::prelude::*;

/// Full-text search over the document library.
///
/// # Why a working default rather than a no-op seam
///
/// The notification work shipped a `Push` trait whose default implementation
/// logged and did nothing, which was right there: nobody expects a log line to
/// ring a phone. Search is the opposite. A search box that returns nothing
/// looks identical to a library with nothing in it, and the person who
/// concludes the second is the person who stops using it. So the seam ships
/// with an engine behind it: Postgres full text, over titles and over the
/// markdown of authored chapters.
///
/// # Why two columns and not one
///
/// A title lives on `documents`; a chapter's text lives on the version that is
/// current. There is no single row holding both, and materialising one would
/// mean a trigger that has to fire on two tables and stay correct through every
/// publish. Two generated columns need no trigger at all — Postgres maintains
/// them — and the search joins them at query time through
/// `current_version_id`, which is already a primary-key lookup.
///
/// The regconfig is spelled explicitly (`'english'`) rather than left to the
/// session default, because the one-argument form of `to_tsvector` is STABLE
/// rather than IMMUTABLE and a generated column cannot be built from it.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            ALTER TABLE documents
                ADD COLUMN search_tsv tsvector
                GENERATED ALWAYS AS (to_tsvector('english', title)) STORED;
            CREATE INDEX documents_search ON documents USING GIN (search_tsv);

            -- Every version, not only the current one. Indexing the whole
            -- history costs one GIN entry per version and buys the question
            -- somebody actually asks during an audit: which version said this.
            ALTER TABLE document_versions
                ADD COLUMN body_tsv tsvector
                GENERATED ALWAYS AS (to_tsvector('english', coalesce(body, ''))) STORED;
            CREATE INDEX document_versions_search ON document_versions USING GIN (body_tsv);
        "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            DROP INDEX IF EXISTS document_versions_search;
            ALTER TABLE document_versions DROP COLUMN IF EXISTS body_tsv;
            DROP INDEX IF EXISTS documents_search;
            ALTER TABLE documents DROP COLUMN IF EXISTS search_tsv;
        "#,
            )
            .await?;
        Ok(())
    }
}
