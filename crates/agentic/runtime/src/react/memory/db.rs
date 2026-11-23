//! [`MemoryStore`] over Oxy's Postgres (`agent_memory_notes`): one user's
//! notes in one workspace. Served runs use it; the eval uses
//! [`super::FileMemoryStore`].

use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

use super::{MemoryError, MemoryStore, NoteFact, NoteOp};

/// Whose notes: the user and the workspace they were learned in. A run with
/// no requesting user has no owner, and so no memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteOwner {
    pub workspace_id: Uuid,
    pub user_id: Uuid,
}

pub struct DbMemoryStore {
    db: DatabaseConnection,
    owner: NoteOwner,
    /// The compiled revision the writing run read, kept on each row so a
    /// later reader can tell a note about a view that is gone.
    revision_id: Option<Uuid>,
}

impl DbMemoryStore {
    pub fn new(db: DatabaseConnection, owner: NoteOwner, revision_id: Option<Uuid>) -> Self {
        Self {
            db,
            owner,
            revision_id,
        }
    }
}

fn db_err(e: sea_orm::DbErr) -> MemoryError {
    MemoryError::Db(e.to_string())
}

#[async_trait]
impl MemoryStore for DbMemoryStore {
    /// In append order (`id`), which [`super::latest`] breaks ties on.
    async fn facts(&self) -> Result<Vec<NoteFact>, MemoryError> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT note_id, op, text, run_id, recorded_at FROM agent_memory_notes \
             WHERE workspace_id = $1 AND user_id = $2 ORDER BY id",
            [self.owner.workspace_id.into(), self.owner.user_id.into()],
        );
        let rows = self.db.query_all_raw(stmt).await.map_err(db_err)?;
        rows.iter()
            .map(|r| {
                let op: String = r.try_get("", "op").map_err(db_err)?;
                Ok(NoteFact {
                    note_id: r.try_get("", "note_id").map_err(db_err)?,
                    op: match op.as_str() {
                        "delete" => NoteOp::Delete,
                        _ => NoteOp::Put,
                    },
                    text: r.try_get("", "text").map_err(db_err)?,
                    run_id: r.try_get("", "run_id").map_err(db_err)?,
                    recorded_at: r.try_get("", "recorded_at").map_err(db_err)?,
                })
            })
            .collect()
    }

    async fn append(&self, fact: NoteFact) -> Result<(), MemoryError> {
        let op = match fact.op {
            NoteOp::Put => "put",
            NoteOp::Delete => "delete",
        };
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO agent_memory_notes \
             (workspace_id, user_id, note_id, op, text, run_id, revision_id, recorded_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            [
                self.owner.workspace_id.into(),
                self.owner.user_id.into(),
                fact.note_id.into(),
                op.into(),
                fact.text.into(),
                fact.run_id.into(),
                self.revision_id.into(),
                fact.recorded_at.into(),
            ],
        );
        self.db.execute_raw(stmt).await.map_err(db_err)?;
        Ok(())
    }
}
