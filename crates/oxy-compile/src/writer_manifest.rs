//! The content-addressed rows of a revision ([`crate::manifest`]): written in
//! the finalise transaction beside the per-kind rows, plus the one read the
//! semantic gate needs and the sweep that keeps `compiled_sources` bounded.
//!
//! A submodule of `writer` so the crate keeps its rule that `writer` is the only
//! code that talks to the database.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use sea_orm::ActiveValue::Set;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Statement};
use uuid::Uuid;

use super::RevisionContext;
use crate::errors::CompileError;
use crate::manifest::{Fingerprint, SourceEntry};

/// Rows per `INSERT`. Five or six columns each, so far below Postgres's 65,535
/// bind-parameter ceiling, which a workspace near the 20,000-file archive cap
/// would otherwise reach in one statement.
const INSERT_CHUNK: usize = 1_000;

/// Write `sources` and `fingerprints` for the revision in `ctx`, inside the
/// caller's transaction.
///
/// Bodies first, then the manifest that names them: inserting or touching a
/// body takes its row lock, which is what makes
/// [`prune_unreferenced_sources`] skip a body this transaction is about to
/// reference (see there).
pub(super) async fn write_manifest(
    txn: &impl ConnectionTrait,
    ctx: &RevisionContext,
    sources: &[SourceEntry],
    fingerprints: &[Fingerprint],
) -> Result<(), CompileError> {
    write_bodies(txn, ctx.workspace_id, sources).await?;
    write_entries(txn, ctx, sources).await?;
    write_fingerprints(txn, ctx.revision_id, fingerprints).await
}

async fn write_bodies(
    txn: &impl ConnectionTrait,
    workspace_id: Uuid,
    sources: &[SourceEntry],
) -> Result<(), CompileError> {
    use entity::compiled_sources::{ActiveModel, Column, Entity};
    // One row per distinct hash: two files with the same text share a body,
    // and an `ON CONFLICT DO UPDATE` may not touch one row twice in a statement.
    let mut bodies: BTreeMap<&str, &str> = BTreeMap::new();
    for entry in sources {
        if let Some(text) = entry.text.as_deref() {
            bodies.entry(entry.content_sha256.as_str()).or_insert(text);
        }
    }
    let now = chrono::Utc::now().fixed_offset();
    let rows: Vec<ActiveModel> = bodies
        .into_iter()
        .map(|(hash, text)| ActiveModel {
            workspace_id: Set(workspace_id),
            content_sha256: Set(hash.to_string()),
            content: Set(text.to_string()),
            byte_size: Set(i32::try_from(text.len()).unwrap_or(i32::MAX)),
            last_seen_at: Set(now),
        })
        .collect();
    for chunk in rows.chunks(INSERT_CHUNK) {
        Entity::insert_many(chunk.to_vec())
            .on_conflict(
                OnConflict::columns([Column::WorkspaceId, Column::ContentSha256])
                    .update_column(Column::LastSeenAt)
                    .to_owned(),
            )
            .exec_without_returning(txn)
            .await?;
    }
    Ok(())
}

async fn write_entries(
    txn: &impl ConnectionTrait,
    ctx: &RevisionContext,
    sources: &[SourceEntry],
) -> Result<(), CompileError> {
    use entity::revision_manifest::{ActiveModel, Entity};
    // Discovery lists each path once; deduplicated anyway, because a repeated
    // path would fail the primary key and with it the whole revision.
    let mut seen = HashSet::new();
    let rows: Vec<ActiveModel> = sources
        .iter()
        .filter(|entry| seen.insert(entry.file_path.as_str()))
        .map(|entry| ActiveModel {
            revision_id: Set(ctx.revision_id),
            file_path: Set(entry.file_path.clone()),
            kind: Set(entry.kind.to_string()),
            workspace_id: Set(ctx.workspace_id),
            content_sha256: Set(entry.content_sha256.clone()),
        })
        .collect();
    for chunk in rows.chunks(INSERT_CHUNK) {
        Entity::insert_many(chunk.to_vec())
            .exec_without_returning(txn)
            .await?;
    }
    Ok(())
}

async fn write_fingerprints(
    txn: &impl ConnectionTrait,
    revision_id: Uuid,
    fingerprints: &[Fingerprint],
) -> Result<(), CompileError> {
    use entity::revision_fingerprints::{ActiveModel, Entity};
    let rows: Vec<ActiveModel> = fingerprints
        .iter()
        .map(|f| ActiveModel {
            revision_id: Set(revision_id),
            kind: Set(f.kind.clone()),
            fingerprint: Set(f.fingerprint.clone()),
            file_count: Set(i32::try_from(f.file_count).unwrap_or(i32::MAX)),
            validated: Set(f.validated),
        })
        .collect();
    if !rows.is_empty() {
        Entity::insert_many(rows)
            .exec_without_returning(txn)
            .await?;
    }
    Ok(())
}

/// The semantic gate's recorded verdict on the revision `workspace_id` serves
/// now: `Some(true)` its model built, `Some(false)` it did not, `None` nothing
/// is served or nothing checked it (a revision compiled before the gate).
///
/// A read that fails answers `None`, which never refuses a compile: the gate
/// then behaves as it did before it had a baseline, and the warning says why.
pub(crate) async fn served_semantic_verdict(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Option<bool> {
    let served = match entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await
    {
        Ok(row) => row?.current_revision_id?,
        Err(e) => {
            tracing::warn!(?e, %workspace_id, "compile: could not read the served revision for the semantic gate");
            return None;
        }
    };
    match entity::revision_fingerprints::Entity::find_by_id((served, "semantic".to_string()))
        .one(db)
        .await
    {
        Ok(row) => row?.validated,
        Err(e) => {
            tracing::warn!(?e, %served, "compile: could not read the served revision's semantic verdict");
            None
        }
    }
}

/// Rows the sweep deletes per statement.
const PRUNE_BATCH: i64 = 1_000;

/// Delete `compiled_sources` rows no manifest names, that no compile has
/// touched for `grace`. Returns how many went.
///
/// Revision retention deletes manifests (by cascade); bodies are shared across
/// revisions, so they go here, like `git gc` after a branch is deleted.
///
/// **Why it cannot delete a body a compile is about to reference.** A compile
/// upserts every body it carries — touching `last_seen_at` — before inserting
/// the manifest rows that name them, in one transaction. If this `DELETE`
/// reaches such a row first, the compile's upsert waits for it and then inserts
/// the body afresh. If the compile reaches it first, this statement waits on the
/// compile's row lock and then re-checks `last_seen_at` on the version the
/// compile committed (Postgres re-evaluates a `DELETE`'s own conditions on a
/// row that changed under it), which is now inside the grace window, so the row
/// is kept. The age condition is therefore on the outer statement, not only in
/// the subquery that picks the batch.
pub async fn prune_unreferenced_sources(
    db: &DatabaseConnection,
    grace: Duration,
) -> Result<u64, CompileError> {
    let grace_secs = i64::try_from(grace.as_secs()).unwrap_or(i64::MAX);
    let sql = "DELETE FROM compiled_sources s \
               WHERE (s.workspace_id, s.content_sha256) IN ( \
                   SELECT c.workspace_id, c.content_sha256 FROM compiled_sources c \
                   WHERE c.last_seen_at < now() - ($1::bigint * interval '1 second') \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM revision_manifest m \
                         WHERE m.workspace_id = c.workspace_id \
                           AND m.content_sha256 = c.content_sha256) \
                   LIMIT $2) \
                 AND s.last_seen_at < now() - ($1::bigint * interval '1 second') \
                 AND NOT EXISTS ( \
                     SELECT 1 FROM revision_manifest m \
                     WHERE m.workspace_id = s.workspace_id \
                       AND m.content_sha256 = s.content_sha256)";
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [grace_secs.into(), PRUNE_BATCH.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}
