//! [`MemoryStore`] over Oxy's Postgres (`agent_memory_notes`): one user's
//! notes in one workspace. Served runs use it; the eval uses
//! [`super::FileMemoryStore`].
//!
//! Runs and the merge task write the same notes concurrently, so every write
//! takes the owner's transaction-scoped advisory lock: a merge's apply
//! ([`MemoryStore::apply_merge`]) checks and writes with no run write in
//! between, and holds the lock only for that, never across a model call.

use async_trait::async_trait;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use uuid::Uuid;

use super::{MemoryError, MemoryStore, MergePlan, NoteFact, NoteOp, latest};

/// Whose notes: the user and the workspace they were learned in. A run with
/// no requesting user has no owner, and so no memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteOwner {
    pub workspace_id: Uuid,
    pub user_id: Uuid,
}

impl NoteOwner {
    /// The advisory lock key every write to these notes takes.
    fn lock_key(&self) -> String {
        format!("agent_memory_notes:{}:{}", self.workspace_id, self.user_id)
    }
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

    async fn read(&self, conn: &impl ConnectionTrait) -> Result<Vec<NoteFact>, MemoryError> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT note_id, op, text, run_id, recorded_at FROM agent_memory_notes \
             WHERE workspace_id = $1 AND user_id = $2 ORDER BY id",
            [self.owner.workspace_id.into(), self.owner.user_id.into()],
        );
        let rows = conn.query_all_raw(stmt).await.map_err(db_err)?;
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

    async fn insert(&self, conn: &impl ConnectionTrait, fact: NoteFact) -> Result<(), MemoryError> {
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
        conn.execute_raw(stmt).await.map_err(db_err)?;
        Ok(())
    }

    /// Takes the owner's lock for the rest of `conn`'s transaction.
    async fn lock(&self, conn: &impl ConnectionTrait) -> Result<(), MemoryError> {
        let stmt = Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
            [self.owner.lock_key().into()],
        );
        conn.execute_raw(stmt).await.map_err(db_err)?;
        Ok(())
    }
}

fn db_err(e: sea_orm::DbErr) -> MemoryError {
    MemoryError::Db(e.to_string())
}

#[async_trait]
impl MemoryStore for DbMemoryStore {
    /// In append order (`id`), which [`super::latest`] breaks ties on.
    async fn facts(&self) -> Result<Vec<NoteFact>, MemoryError> {
        self.read(&self.db).await
    }

    async fn append(&self, fact: NoteFact) -> Result<(), MemoryError> {
        let tx = self.db.begin().await.map_err(db_err)?;
        self.lock(&tx).await?;
        self.insert(&tx, fact).await?;
        tx.commit().await.map_err(db_err)
    }

    async fn apply_merge(&self, plan: &MergePlan, run_id: &str) -> Result<bool, MemoryError> {
        let tx = self.db.begin().await.map_err(db_err)?;
        self.lock(&tx).await?;
        if !plan.still_applies(&latest(self.read(&tx).await?)) {
            return Ok(false);
        }
        for fact in plan.facts(run_id) {
            self.insert(&tx, fact).await?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(true)
    }
}
