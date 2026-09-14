use sea_orm_migration::prelude::*;

/// A record of what somebody asked the library, and what they were told.
///
/// # What is stored, and the one thing that is not
///
/// The question, the prose that came back, the ids the engine was given and
/// the ids the answer cited.
/// **Never the documents' text.** That is the same rule the ask route enforces
/// in memory, written into the schema so a later feature cannot quietly break
/// it: a transcript holding bodies would keep them after the reader stopped
/// being allowed to open them, and the per-turn permission re-check that makes
/// a session safe would be undone by its own history.
///
/// # Why the citations are JSONB and not a join table
///
/// `document_favorites` is a join table with a real foreign key, and that is
/// right for a bookmark: a deleted document should take the bookmark with it.
/// A citation is the opposite kind of fact. It records what an answer used at
/// a moment in time, and `ON DELETE CASCADE` on it would let deleting a
/// document rewrite a transcript that was true when it was written.
///
/// So the ids are data, not a relationship. A citation whose document is gone
/// renders as nothing, because the reader hydrates through
/// `visible_documents` — which is also what makes a citation to a document
/// somebody has since lost access to disappear rather than disclose a title.
///
/// # Why a session is not a chat thread
///
/// `agentic_thread` and `/api/chat/*` both exist and neither fits. The first
/// belongs to an agent run; the second is person-to-person messaging between
/// colleagues, and folding an assistant into it is a conflation that would be
/// expensive to undo. This is a small append-only log with one owner.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            CREATE TABLE document_ask_sessions (
                id         UUID PRIMARY KEY,
                org_id     UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                -- The owner, and the whole authorization model for this table:
                -- a session is nobody's but the person who opened it. There is
                -- no officer override, deliberately — reading what a worker
                -- asked is surveillance, not administration.
                user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                -- Bumped by each turn. The list sorts on this rather than on
                -- `created_at`, so a conversation somebody came back to rises
                -- to the top instead of sinking under newer, shorter ones.
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
            );

            -- The list query, exactly: mine, in this org, most recent first.
            CREATE INDEX document_ask_sessions_mine
                ON document_ask_sessions (user_id, org_id, updated_at DESC);

            CREATE TABLE document_ask_turns (
                id         UUID PRIMARY KEY,
                session_id UUID NOT NULL
                           REFERENCES document_ask_sessions(id) ON DELETE CASCADE,
                -- 1-based, and unique per session. The order of a conversation
                -- is a fact about it, not something to rediscover from
                -- timestamps that can collide inside one millisecond.
                seq        INTEGER NOT NULL,
                question   TEXT NOT NULL,
                -- NULL when no engine wrote one. A turn where the library had
                -- nothing to say is still a turn that happened, and dropping it
                -- would make the transcript disagree with what the reader saw.
                answer     TEXT,
                -- Every document the engine was given, in the order it was
                -- given them. This is what the `[1]`, `[2]` markers inside
                -- `answer` index into, so without it the prose in a stored
                -- transcript carries numbers that point at nothing.
                --
                -- Found by reading one back: a turn whose answer said "[2]"
                -- came back with a single citation, and no way to tell which
                -- slot it was.
                sources    JSONB NOT NULL DEFAULT '[]'::jsonb,
                -- The subset the answer actually used, in the order it first
                -- used them. A different question from `sources` — "what was
                -- available" and "what was used" are both worth keeping, and
                -- deriving either from the other is impossible.
                cited      JSONB NOT NULL DEFAULT '[]'::jsonb,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                UNIQUE (session_id, seq)
            );
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
            DROP TABLE IF EXISTS document_ask_turns;
            DROP INDEX IF EXISTS document_ask_sessions_mine;
            DROP TABLE IF EXISTS document_ask_sessions;
        "#,
            )
            .await?;
        Ok(())
    }
}
