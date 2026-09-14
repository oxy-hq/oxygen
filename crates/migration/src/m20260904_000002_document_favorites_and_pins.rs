use sea_orm_migration::prelude::*;

/// The two shelves a Knowledge base puts above the folder tree.
///
/// # Why these are two different mechanisms and not one
///
/// Delightree ships a **Favorites** tab and a **Pinned** tab side by side. If
/// both were per-user they would be the same feature twice and one of the tabs
/// would have no reason to exist. The only reading where both earn their place
/// is the one modelled here: a favorite is a personal bookmark, and a pin is an
/// officer saying "everybody read this".
///
/// That difference decides the storage. A favorite is per-viewer, so it is a
/// join table keyed by the pair. A pin is a property of the document itself —
/// singular, org-wide — so it is two columns on the row, and asking "is this
/// pinned" costs no join.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            CREATE TABLE document_favorites (
                user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                document_id UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (user_id, document_id)
            );
            -- The PK already serves "what has this person favorited". This one
            -- serves the other direction, which the delete path uses.
            CREATE INDEX document_favorites_by_document
                ON document_favorites (document_id);

            ALTER TABLE documents
                ADD COLUMN pinned_at TIMESTAMPTZ,
                -- `pinned_at` alone decides whether a document is pinned.
                --
                -- There was a `documents_pin_is_whole` CHECK here asserting the
                -- two columns are null together, by analogy with
                -- `documents_review_decision_is_whole`. The analogy does not
                -- hold: that constraint spans `review_status` and `reviewed_at`,
                -- neither of which any foreign key touches, while this one spans
                -- a column that is `ON DELETE SET NULL`. Deleting an officer who
                -- had pinned anything would null `pinned_by`, leave `pinned_at`
                -- set, and fail the CHECK — which does not reject the pin, it
                -- rejects the DELETE. Removing a departed employee would error
                -- naming a table nobody was touching.
                --
                -- This repo has already been here: see the comment in
                -- `m20260901_000002_assignment_graph.rs` refusing the same CHECK
                -- over `completed_by` for the same reason. A pin whose author
                -- has left is a real state and the shelf still renders — the
                -- card shows the pin and no name, which is the honest reading of
                -- "pinned by somebody who is gone".
                ADD COLUMN pinned_by UUID REFERENCES users(id) ON DELETE SET NULL;

            -- The Pinned tab is one small ordered read. Partial on the state
            -- that puts a row in it, so the index stays the size of the shelf
            -- rather than the size of the library.
            CREATE INDEX documents_pinned
                ON documents (org_id, pinned_at DESC)
                WHERE deleted_at IS NULL AND pinned_at IS NOT NULL;
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
            DROP INDEX IF EXISTS documents_pinned;
            ALTER TABLE documents
                DROP CONSTRAINT IF EXISTS documents_pin_is_whole,
                DROP COLUMN IF EXISTS pinned_by,
                DROP COLUMN IF EXISTS pinned_at;
            DROP TABLE IF EXISTS document_favorites CASCADE;
        "#,
            )
            .await?;
        Ok(())
    }
}
