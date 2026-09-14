use sea_orm_migration::prelude::*;

/// Compliance's two remaining screens: categories, and the review queue.
///
/// # Why review is its own axis and not more values in `status`
///
/// `documents.status` is `draft` | `published`, and that answers "can anybody
/// read this yet". Approval answers something else: "has somebody signed this
/// off". A Compliance document sits in the table, visible, marked **In review**
/// — so it is published AND undecided at the same time, and one column cannot
/// hold both without the Knowledge base inheriting a vocabulary it has no use
/// for.
///
/// Kept nullable rather than defaulted, because most documents are never
/// reviewed at all. NULL means "nobody is asking", which is different from
/// "waiting" and from "approved", and a default would have erased that.
///
/// # Why a category can be deleted and a location cannot
///
/// `documents.location_id` is `NO ACTION`: removing a location would silently
/// widen a document from one store to the whole org. A category carries no
/// visibility, so `SET NULL` costs a label and nothing else — the document
/// simply becomes uncategorised, which is a state the screen already has to
/// render.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            -- Tenant-defined, like `org_roles` and for the same reason: the
            -- eight names one operator uses ("Certificate of Insurance",
            -- "Design Ext/Int", "EIN") are theirs, and an enum would be a
            -- migration every time somebody files a new kind of paperwork.
            CREATE TABLE document_categories (
                id         UUID PRIMARY KEY,
                org_id     UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                name       TEXT NOT NULL,
                created_by UUID REFERENCES users(id) ON DELETE SET NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                UNIQUE (org_id, name)
            );

            ALTER TABLE documents
                ADD COLUMN category_id UUID REFERENCES document_categories(id) ON DELETE SET NULL,
                -- NULL = nobody is asking. Deliberately not defaulted: most
                -- documents are never reviewed, and a default would make the
                -- queue's own emptiness unrepresentable.
                ADD COLUMN review_status TEXT
                    CHECK (review_status IN ('in_review','approved','rejected')),
                ADD COLUMN reviewed_by UUID REFERENCES users(id) ON DELETE SET NULL,
                ADD COLUMN reviewed_at TIMESTAMPTZ,
                ADD COLUMN review_note TEXT,
                -- A decision records when it was made, or it is not a decision.
                -- `in_review` and NULL both mean undecided, so both must have no
                -- timestamp — the same shape as `work_items_completion_is_whole`.
                ADD CONSTRAINT documents_review_decision_is_whole
                    CHECK ((coalesce(review_status,'') IN ('approved','rejected'))
                           = (reviewed_at IS NOT NULL));

            -- The queue is one screen asking one question, and it is small:
            -- partial on the state that puts a row in it.
            CREATE INDEX documents_in_review
                ON documents (org_id, updated_at)
                WHERE deleted_at IS NULL AND review_status = 'in_review';
            CREATE INDEX documents_by_category
                ON documents (category_id)
                WHERE deleted_at IS NULL;

            -- Folders join the search index. A folder name is short and there
            -- are a dozen of them, so this buys little on its own — but a search
            -- that finds the SOP and not the folder it lives in sends the reader
            -- somewhere they cannot navigate back from.
            ALTER TABLE folders
                ADD COLUMN search_tsv tsvector
                GENERATED ALWAYS AS (to_tsvector('english', name)) STORED;
            CREATE INDEX folders_search ON folders USING GIN (search_tsv);
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
            DROP INDEX IF EXISTS folders_search;
            ALTER TABLE folders DROP COLUMN IF EXISTS search_tsv;

            DROP INDEX IF EXISTS documents_by_category;
            DROP INDEX IF EXISTS documents_in_review;
            ALTER TABLE documents
                DROP CONSTRAINT IF EXISTS documents_review_decision_is_whole,
                DROP COLUMN IF EXISTS review_note,
                DROP COLUMN IF EXISTS reviewed_at,
                DROP COLUMN IF EXISTS reviewed_by,
                DROP COLUMN IF EXISTS review_status,
                DROP COLUMN IF EXISTS category_id;

            DROP TABLE IF EXISTS document_categories CASCADE;
        "#,
            )
            .await?;
        Ok(())
    }
}
