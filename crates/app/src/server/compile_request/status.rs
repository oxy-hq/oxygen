//! Where the compile of one commit stands: the status read beside
//! [`super::compile`]. Postgres only, so any pod answers it.

use oxy_compile::RevisionKind;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, Statement,
};
use serde::Serialize;
use uuid::Uuid;

/// The answer of [`super::compile`] and of [`status`], and the body of the
/// two `/compile/staging` routes. `revision_id` is set exactly when `status`
/// is `ready`: that id is what a caller pins.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct CompileState {
    pub workspace_id: Uuid,
    pub git_sha: String,
    /// `ready` | `compiling` | `pending` (queued, no revision row yet) |
    /// `stale` (a ready revision this server won't reuse; ask again once) |
    /// `failed`.
    pub status: String,
    pub revision_id: Option<Uuid>,
    /// The enqueued compile task, when this call enqueued one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CompileState {
    pub(super) fn ready(workspace_id: Uuid, git_sha: String, revision_id: Uuid) -> Self {
        Self {
            revision_id: Some(revision_id),
            ..Self::bare(workspace_id, git_sha, "ready")
        }
    }

    pub(super) fn pending(workspace_id: Uuid, git_sha: String, task_id: Option<String>) -> Self {
        Self {
            task_id,
            ..Self::bare(workspace_id, git_sha, "pending")
        }
    }

    fn failed(workspace_id: Uuid, git_sha: String, error: String) -> Self {
        Self {
            error: Some(error),
            ..Self::bare(workspace_id, git_sha, "failed")
        }
    }

    fn bare(workspace_id: Uuid, git_sha: String, status: &str) -> Self {
        Self {
            workspace_id,
            git_sha,
            status: status.into(),
            revision_id: None,
            task_id: None,
            error: None,
        }
    }
}

/// Where the compile of `git_sha` as `kind` stands.
///
/// A ready revision the caller may reuse answers first. Otherwise the newest
/// revision row of the commit says what happened to it. With no row at all the
/// queue is asked: a compile that failed before it wrote a revision — the
/// commit could not be fetched, the tree was refused — is `failed` with the
/// task's reason, and anything else is `pending`.
pub async fn status(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
    kind: RevisionKind,
) -> Result<CompileState, DbErr> {
    let git_sha = git_sha.to_string();
    if let Some(revision_id) =
        oxy_compile::find_reusable_revision(db, workspace_id, kind, &git_sha).await?
    {
        return Ok(CompileState::ready(workspace_id, git_sha, revision_id));
    }
    if let Some(row) = latest_for_sha(db, workspace_id, &git_sha, kind).await? {
        return Ok(from_row(workspace_id, git_sha, row));
    }
    Ok(
        match failed_before_a_revision(db, workspace_id, &git_sha, kind).await? {
            Some(error) => CompileState::failed(workspace_id, git_sha, error),
            None => CompileState::pending(workspace_id, git_sha, None),
        },
    )
}

/// Whether a compile of `git_sha` for this workspace is queued or running on the
/// task queue — the one state `revisions` cannot show, because a compile writes
/// its revision row only once a worker has claimed it. Any kind counts: a main
/// compile of the commit in flight will leave a revision a staging one reuses.
pub async fn compile_task_in_flight(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
) -> Result<bool, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 FROM agentic_task_queue \
             WHERE queue_status IN ('queued', 'claimed') \
               AND spec->>'type' = 'compile' \
               AND spec->>'workspace_id' = $1 \
               AND spec->>'git_sha' = $2 \
             LIMIT 1",
            [workspace_id.to_string().into(), git_sha.into()],
        ))
        .await?;
    Ok(row.is_some())
}

/// The reason the newest compile task of this commit failed, when it did and
/// nothing is queued after it. Only asked when the commit has no revision row,
/// which is what a compile that never started compiling leaves: fetching the
/// commit happens before the row is written.
///
/// Without it such a failure reads as `pending` for good, and a caller polling
/// for `ready` waits out its whole timeout on a compile that ended in seconds.
///
/// It looks at every kind a compile of `kind` may reuse, as the row lookup
/// does, not at `kind` alone: [`super::compile`] joins a compile of the commit
/// that is in flight whatever its kind, so a staging request can be waiting on
/// a main compile, and has to hear that it failed.
async fn failed_before_a_revision(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
    kind: RevisionKind,
) -> Result<Option<String>, DbErr> {
    let kinds: Vec<String> = kind
        .reusable_kinds()
        .iter()
        .map(|k| k.to_string())
        .collect();
    if kinds.is_empty() {
        return Ok(None);
    }
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT q.queue_status, r.error_message FROM agentic_task_queue q \
             JOIN agentic_runs r ON r.id = q.run_id \
             WHERE q.spec->>'type' = 'compile' \
               AND q.spec->>'workspace_id' = $1 \
               AND q.spec->>'git_sha' = $2 \
               AND COALESCE(q.spec->>'kind', 'main') = ANY($3) \
             ORDER BY q.created_at DESC \
             LIMIT 1",
            [
                workspace_id.to_string().into(),
                git_sha.into(),
                kinds.into(),
            ],
        ))
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let queue_status: String = row.try_get("", "queue_status")?;
    if !matches!(queue_status.as_str(), "failed" | "dead") {
        return Ok(None);
    }
    let reason: Option<String> = row.try_get("", "error_message")?;
    Ok(Some(
        reason
            .filter(|r| !r.trim().is_empty())
            .unwrap_or_else(|| unexplained(&queue_status).to_string()),
    ))
}

/// What to say for a task that ended without a reason of its own.
fn unexplained(queue_status: &str) -> &'static str {
    match queue_status {
        // Dead-lettered: its wait or its claims ran out, so no worker ever
        // reported an outcome for it.
        "dead" => {
            "the compile was dead-lettered before it produced a revision: no worker ran it \
             to an outcome"
        }
        _ => "the compile failed before it produced a revision",
    }
}

/// Newest revision row of this SHA among the kinds a compile of `kind` may
/// reuse, whatever its status.
pub(super) async fn latest_for_sha(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
    kind: RevisionKind,
) -> Result<Option<entity::revisions::Model>, DbErr> {
    entity::revisions::Entity::find()
        .filter(entity::revisions::Column::WorkspaceId.eq(workspace_id))
        .filter(entity::revisions::Column::GitSha.eq(git_sha))
        .filter(entity::revisions::Column::Kind.is_in(kind.reusable_kinds().iter().copied()))
        .order_by_desc(entity::revisions::Column::StartedAt)
        .one(db)
        .await
}

/// A non-reusable row: in flight, failed, or superseded. A `ready` row that
/// [`oxy_compile::find_reusable_revision`] skipped (another compiler or schema
/// version, e.g. a deploy landed mid-poll) reports `stale`, its own status and
/// not `pending`: `pending` also means "queued, no row yet", where asking
/// again would enqueue a second compile. On `stale` the caller asks once more,
/// which enqueues a fresh compile rather than pinning a revision this build
/// can't read.
pub(super) fn from_row(
    workspace_id: Uuid,
    git_sha: String,
    row: entity::revisions::Model,
) -> CompileState {
    match row.status.as_str() {
        "compiling" => CompileState::bare(workspace_id, git_sha, "compiling"),
        "failed" => CompileState::failed(
            workspace_id,
            git_sha,
            row.error_summary
                .map(|v| v.to_string())
                .unwrap_or_else(|| "compile failed".into()),
        ),
        "ready" => CompileState::bare(workspace_id, git_sha, "stale"),
        _ => CompileState::bare(workspace_id, git_sha, "pending"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str) -> entity::revisions::Model {
        let now = chrono::Utc::now().fixed_offset();
        entity::revisions::Model {
            revision_id: Uuid::new_v4(),
            workspace_id: Uuid::nil(),
            git_sha: "abc".into(),
            branch: Some("feature".into()),
            schema_version: 1,
            status: status.into(),
            kind: "staging".into(),
            owner_user_id: None,
            compiler_version: "x".into(),
            started_at: now,
            finished_at: None,
            file_count_seen: 0,
            file_count_compiled: 0,
            file_count_failed: 0,
            error_summary: None,
        }
    }

    #[test]
    fn a_non_reusable_row_never_hands_out_a_revision_id() {
        for s in ["compiling", "failed", "ready", "superseded"] {
            let r = from_row(Uuid::nil(), "abc".into(), row(s));
            assert_eq!(r.revision_id, None, "status {s}");
        }
        assert_eq!(
            from_row(Uuid::nil(), "abc".into(), row("failed")).status,
            "failed"
        );
        // A ready row this server won't reuse is `stale` (the CLI asks again);
        // `pending` stays "queued, nothing to recompile", which it must not.
        assert_eq!(
            from_row(Uuid::nil(), "abc".into(), row("ready")).status,
            "stale"
        );
        assert_eq!(
            from_row(Uuid::nil(), "abc".into(), row("superseded")).status,
            "pending"
        );
    }
}
