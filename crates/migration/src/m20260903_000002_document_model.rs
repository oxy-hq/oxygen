use sea_orm_migration::prelude::*;

/// The document model — folders, documents, and immutable versions.
///
/// The third and last blocking primitive named by the "Landing the Platform"
/// plan, and the one two modules are stuck behind: Knowledge base and
/// Compliance are both a file list with a visibility rule, and neither can be
/// built out of `ctx.storage` alone because a per-app silo cannot express "HQ
/// only" or "this store only".
///
/// # Why this is platform state and not an app's own tables
///
/// A custom app could model documents in its `ctx.oltp` schema in an afternoon,
/// and for a store's own operational rows that is the right answer. Documents
/// are not those rows. Three things force them up here:
///
/// * The read filter joins `locations` and `org_frontline_members`, which live
///   here. An app-side copy would re-derive "which stores is this person at"
///   from its own tables and drift from the assignment graph the day one moves.
/// * Search has to reach them. An `app_*` schema is private to one writer by
///   design, so a document indexed there is invisible to the analytics agent
///   and to every other app.
/// * Compliance is plausibly a second app. Two apps reading one library is the
///   case a silo cannot serve at all.
///
/// # Not the compile boundary either
///
/// These are user content, not workspace artifacts: no `revision_id`, no
/// `.yml`, no git. The 25 Aug feasibility doc flagged this as the expensive
/// call to get wrong early, and it is the same call `work_items` made.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            -- The tree. A folder carries a DEFAULT visibility rather than a
            -- binding one: Delightree's twelve folders are authored per team
            -- ("Back of House", "Marketing Essentials SOP") and the exceptions
            -- live at the item, so a folder that dictated visibility would be
            -- wrong for exactly the documents anyone thinks about.
            CREATE TABLE folders (
                id          UUID PRIMARY KEY,
                org_id      UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                parent_id   UUID REFERENCES folders(id) ON DELETE CASCADE,
                name        TEXT NOT NULL,
                -- 'org'  — every member and every frontline worker in the org.
                -- 'hq'   — org members only; a frontline worker never sees it.
                visibility  TEXT NOT NULL DEFAULT 'org'
                            CHECK (visibility IN ('org','hq')),
                created_by  UUID REFERENCES users(id) ON DELETE SET NULL,
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                -- Soft delete IS the product's "Deleted" tab, not a hedge
                -- against a hard delete. Restoring is the feature.
                deleted_at  TIMESTAMPTZ
            );
            -- Every listing is scoped to an org and hides the trash, so the
            -- partial index is the shape the hot path actually asks for.
            CREATE INDEX folders_live_by_org ON folders (org_id) WHERE deleted_at IS NULL;
            CREATE INDEX folders_by_parent ON folders (parent_id) WHERE deleted_at IS NULL;

            -- One table for BOTH document kinds.
            --
            -- Delightree ships two: authored chapters (Knowledge base) and
            -- uploaded files (Compliance). They differ in where the bytes live
            -- and in nothing else — same folder, same visibility rule, same
            -- versions, same trash, same search. Two tables would be the same
            -- system built twice, and the second copy is where the visibility
            -- filter drifts.
            CREATE TABLE documents (
                id          UUID PRIMARY KEY,
                org_id      UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                folder_id   UUID REFERENCES folders(id) ON DELETE SET NULL,
                title       TEXT NOT NULL,
                -- 'chapter' — markdown authored in the app, body in Postgres.
                -- 'file'    — an upload, bytes in S3 under org-documents/.
                kind        TEXT NOT NULL CHECK (kind IN ('file','chapter')),
                -- 'draft' is the "Drafts" tab: visible to its author and to
                -- document managers, to nobody else.
                status      TEXT NOT NULL DEFAULT 'draft'
                            CHECK (status IN ('draft','published')),
                visibility  TEXT NOT NULL DEFAULT 'org'
                            CHECK (visibility IN ('org','hq')),

                -- NULL = every location. Set = this store only.
                --
                -- NO ACTION on purpose, which is neither of the two obvious
                -- choices and is why this comment exists. ON DELETE SET NULL
                -- would WIDEN a document from one store to the whole org the
                -- moment somebody removed a location — a visibility change
                -- nobody asked for, arriving silently. CASCADE would delete the
                -- health permit for a store that closed, which is a record the
                -- tenant may be required to keep. So deleting a location that
                -- still has documents fails, and the remedy is the lifecycle
                -- `locations.status` already carries ('archived', 'terminated').
                --
                -- NO ACTION rather than RESTRICT because the check must be
                -- deferred to end-of-statement: deleting an ORG cascades to
                -- both tables, and RESTRICT would fire before the cascade had
                -- removed these rows, making org deletion fail.
                location_id UUID REFERENCES locations(id),

                -- The Compliance hook, two columns in v1 because that is all
                -- its DATA shape is. `work_items` already lists "document
                -- expiry" as an anticipated source_kind; the sweeper that
                -- opens the work item is later, and needs no schema change.
                expires_at  TIMESTAMPTZ,

                -- Set once a version is confirmed. FK added after
                -- document_versions exists — the two tables reference each
                -- other, and one of the two constraints has to come second.
                current_version_id UUID,

                created_by  UUID REFERENCES users(id) ON DELETE SET NULL,
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                deleted_at  TIMESTAMPTZ,

                -- A published document must have something to read. Enforced
                -- here rather than in the handler because "published with no
                -- version" is not a state anything can render, and the publish
                -- path is not the only writer that will ever exist.
                CONSTRAINT documents_published_has_a_version
                    CHECK (status <> 'published' OR current_version_id IS NOT NULL)
            );
            CREATE INDEX documents_live_by_org ON documents (org_id) WHERE deleted_at IS NULL;
            CREATE INDEX documents_by_folder ON documents (folder_id) WHERE deleted_at IS NULL;
            -- The frontline read: published, org-visible, at my store or
            -- everywhere. Ordered to match the filter's own selectivity.
            CREATE INDEX documents_by_location
                ON documents (org_id, location_id)
                WHERE deleted_at IS NULL AND status = 'published';
            -- Compliance's only scan: what expires soon. Partial because most
            -- of the library has no expiry at all.
            CREATE INDEX documents_expiring
                ON documents (org_id, expires_at)
                WHERE deleted_at IS NULL AND expires_at IS NOT NULL;

            -- Immutable. Never UPDATEd, never DELETEd on its own — a version is
            -- the record of what a document said when somebody signed off on
            -- it, and Compliance is the reason that has to stay true.
            CREATE TABLE document_versions (
                id           UUID PRIMARY KEY,
                document_id  UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
                version_no   INTEGER NOT NULL CHECK (version_no > 0),
                -- NULL when the author's user row is gone. The version stays:
                -- a document that loses its history when somebody leaves is
                -- unreadable exactly when it is being audited.
                author_id    UUID REFERENCES users(id) ON DELETE SET NULL,

                -- Exactly one of these two. A chapter's markdown lives in the
                -- row; a file's bytes live in S3 and the row holds the key.
                body         TEXT,
                object_key   TEXT,

                content_type TEXT,
                size_bytes   BIGINT,
                created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

                CONSTRAINT document_versions_body_xor_object_key
                    CHECK ((body IS NULL) <> (object_key IS NULL)),
                -- Version numbers are per document and dense. The unique index
                -- is what makes "next version" a safe read-then-insert: a
                -- concurrent second writer loses on the constraint rather than
                -- silently overwriting the first one's number.
                CONSTRAINT document_versions_are_numbered_once
                    UNIQUE (document_id, version_no)
            );
            CREATE INDEX document_versions_by_document
                ON document_versions (document_id, version_no DESC);

            ALTER TABLE documents
                ADD CONSTRAINT documents_current_version_fkey
                FOREIGN KEY (current_version_id)
                REFERENCES document_versions(id) ON DELETE SET NULL;
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
            -- documents first: it holds the FK into document_versions that was
            -- added last, and CASCADE on the DROP takes the constraint with it.
            DROP TABLE IF EXISTS document_versions CASCADE;
            DROP TABLE IF EXISTS documents CASCADE;
            DROP TABLE IF EXISTS folders CASCADE;
        "#,
            )
            .await?;
        Ok(())
    }
}
