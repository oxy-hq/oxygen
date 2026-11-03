//! Pin a custom-app environment's semantic model to a workspace git branch,
//! and move the pin when the branch's next commit has compiled.
//!
//! An `app_environments` row carries two columns for this
//! (`entity::app_environments`): `semantic_git_sha`, the commit the
//! environment is waiting on or pinned to, and `semantic_revision_id`, the
//! compiled revision that is actually served. The reader
//! (`oxy_custom_apps::staging_pin::environment_pin`) turns them into a state
//! **in the environment's semantic mode**: when the plane names a branch, a
//! revision means *ready*, and a sha without one, or neither, means *not ready*
//! (`503`); when it does not, the columns are ignored (*unset*). This module is
//! the writer side.
//!
//! * [`pin_to_branch`] compiles the branch through `compile_request` as a
//!   `staging` revision (never promoted, so the workspace keeps serving what
//!   it served) and stores the answer.
//! * [`move_after_compile`] is what the compile worker calls when a compile
//!   succeeds: every environment waiting on that commit moves onto the new
//!   revision in one statement.
//!
//! **A pin lands only on a row that still names its branch.** The planes write
//! stores the choice (and clears the previous pin) in one transaction, and the
//! pin is written *after* it, so two writers can interleave: W1 stores
//! `branch:A`, W2 stores `branch:B`, then W1's pin arrives. [`write_pin`]
//! therefore carries the branch in its `WHERE`; W1's write finds the row naming
//! B and changes nothing ([`PinOutcome::Superseded`]), so A's revision is never
//! served under a row that names B.
//!
//! **A new commit of the same branch keeps serving the old model.** An
//! environment already on one revision whose branch has a newer head not
//! compiled yet keeps its old `semantic_revision_id` and takes the new sha, so
//! it serves the old model until the new one is ready rather than going dark.
//! (A planes write that names a *different* branch clears the previous branch's
//! pin first, in the store's transaction, so a revision is never served under a
//! branch it was not compiled from.) One that was never
//! pinned has no old model to keep, so it is refused (`503`) until its first
//! compile completes, which is intended: it must not be routed to another
//! model meanwhile.
//!
//! * [`on_branch_push`] is the GitHub `push` side: a commit on a branch some
//!   environment's `planes` name is compiled and written as the environment's
//!   waiting sha, so it follows the branch without anyone asking.
//!
//! **The workspace is `apps.project_id`.** That column holds the workspace id
//! (it equals `workspaces.id`), which is how the move finds every app of the
//! workspace whose compile just finished.

use entity::workspaces::{self, WorkspaceStatus};
use oxy_compile::RevisionKind;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, Statement,
};
use uuid::Uuid;

use super::push::PushEvent;
use crate::server::compile_git::github_slug_of;
use crate::server::compile_request::{self, Refusal, Target};

/// What [`pin_to_branch`] left the environment on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinOutcome {
    /// The branch's head was already compiled: the environment now serves
    /// `revision_id`, a revision of `git_sha`.
    Pinned { revision_id: Uuid, git_sha: String },
    /// The head is compiling (or queued, or failed). The environment waits on
    /// `git_sha` and keeps any revision it already had; [`move_after_compile`]
    /// moves it when the compile lands.
    Compiling { git_sha: String },
    /// The environment no longer names the branch: a later planes write
    /// replaced the choice while this pin was being set, and that write's own
    /// pin is the one that counts. Nothing was written. Not an error for the
    /// loser of the race.
    Superseded { git_sha: String },
}

/// Point `env_name` of `app_id` at the head of `branch`.
///
/// Compiles the branch as a staging revision, or reuses a ready one of the same
/// commit. A ready revision sets both columns; anything else sets only
/// `semantic_git_sha` and leaves `semantic_revision_id` as it was (see the
/// module docs), after one more look for a revision that landed meanwhile
/// ([`settle_pending_pin`]). Refusals are `compile_request`'s: an unknown branch, a
/// branch GitHub cannot be asked about, and so on.
///
/// A `failed` compile also lands as [`PinOutcome::Compiling`]: nothing moves
/// the pin for a compile that never succeeds, so asking again is the retry.
#[tracing::instrument(skip_all, fields(workspace_id = %workspace.id, %app_id, env = env_name))]
pub async fn pin_to_branch(
    db: &DatabaseConnection,
    workspace: &workspaces::Model,
    app_id: Uuid,
    env_name: &str,
    branch: &str,
) -> Result<PinOutcome, Refusal> {
    let state =
        compile_request::compile(db, workspace, Target::Branch(branch), RevisionKind::Staging)
            .await?;
    let ready = state.revision_id.filter(|_| state.status == "ready");
    match write_pin(db, app_id, env_name, branch, ready, &state.git_sha).await? {
        PinWrite::Written => {}
        PinWrite::Superseded => {
            tracing::info!(
                %app_id, env = env_name, %branch, git_sha = %state.git_sha,
                "semantic pin not written: the environment no longer names this branch"
            );
            return Ok(PinOutcome::Superseded {
                git_sha: state.git_sha,
            });
        }
        PinWrite::NoEnvironment => {
            return Err(Refusal::Conflict(format!(
                "environment {env_name:?} has no semantic pin to set: it does not exist, is \
                 being deleted, or is production"
            )));
        }
    }
    // The compile may have finished between `compile` and the write above, in
    // which case the worker's move found no row waiting. Look again now that
    // the sha is stored.
    let settled = match ready {
        Some(revision_id) => Some(revision_id),
        None => settle_pending_pin(db, workspace.id, &state.git_sha).await?,
    };
    Ok(match settled {
        Some(revision_id) => PinOutcome::Pinned {
            revision_id,
            git_sha: state.git_sha,
        },
        None => PinOutcome::Compiling {
            git_sha: state.git_sha,
        },
    })
}

/// Close the race between storing a waiting sha and the compile that lands it.
///
/// Call this **after** `semantic_git_sha` is written. A compile that finished
/// before the write called [`move_after_compile`] while no row was waiting, so
/// nothing would ever move the pin; this finds that ready revision and moves
/// every environment waiting on the commit onto it. Returns the revision it
/// moved them to, or `None` when no ready revision of the commit exists yet,
/// in which case the worker's own move will land it. Idempotent with that move:
/// both write the same revision.
pub async fn settle_pending_pin(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
) -> Result<Option<Uuid>, DbErr> {
    let Some(revision_id) =
        oxy_compile::find_reusable_revision(db, workspace_id, RevisionKind::Staging, git_sha)
            .await?
    else {
        return Ok(None);
    };
    move_after_compile(db, workspace_id, git_sha, revision_id).await?;
    Ok(Some(revision_id))
}

/// What [`write_pin`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinWrite {
    Written,
    /// The environment is there but its `planes` no longer name the branch.
    Superseded,
    /// No live, non-production environment of that name.
    NoEnvironment,
}

/// `Some(revision)` sets both columns; `None` sets the sha and keeps the
/// revision the row has. Production never takes a pin, and a sandbox being torn
/// down serves nothing. **The row must still name `branch`** (see the module
/// docs): a write for a branch the row has since left changes nothing, which a
/// second lookup tells apart from an environment that is not there at all.
async fn write_pin(
    db: &DatabaseConnection,
    app_id: Uuid,
    env_name: &str,
    branch: &str,
    revision_id: Option<Uuid>,
    git_sha: &str,
) -> Result<PinWrite, DbErr> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "UPDATE app_environments e \
                 SET semantic_revision_id = COALESCE($3, e.semantic_revision_id), \
                     semantic_git_sha = $4 \
                 WHERE e.app_id = $1 AND e.name = $2 AND e.kind <> 'production' \
                   AND e.deleting_at IS NULL AND {}",
                names_branch(5)
            ),
            [
                app_id.into(),
                env_name.into(),
                revision_id.into(),
                git_sha.into(),
                branch.into(),
            ],
        ))
        .await?;
    if result.rows_affected() > 0 {
        return Ok(PinWrite::Written);
    }
    let live = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 AS one FROM app_environments \
             WHERE app_id = $1 AND name = $2 AND kind <> 'production' AND deleting_at IS NULL",
            [app_id.into(), env_name.into()],
        ))
        .await?;
    Ok(if live.is_some() {
        PinWrite::Superseded
    } else {
        PinWrite::NoEnvironment
    })
}

/// Whether `branch` exists in `workspace`: the arm that validates a branch
/// name before anything is written. Answers with `compile_request`'s refusal
/// (`404` for a branch nobody has, `400` for a malformed name) and queues
/// nothing.
pub async fn branch_exists(workspace: &workspaces::Model, branch: &str) -> Result<(), Refusal> {
    compile_request::resolve_branch(workspace, branch)
        .await
        .map(|_| ())
}

/// A compile of `git_sha` in `workspace_id` succeeded as `revision_id`: move
/// every environment waiting on that commit onto it. Returns how many moved.
///
/// One statement, so two environments on one branch move together, and one
/// whose pin was cleared or repointed at another commit (a different
/// `semantic_git_sha`) is left alone. A sandbox being torn down is skipped.
#[tracing::instrument(skip(db))]
pub async fn move_after_compile(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
    revision_id: Uuid,
) -> Result<u64, DbErr> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE app_environments e SET semantic_revision_id = $3 \
             FROM apps a \
             WHERE e.app_id = a.id AND a.project_id = $1 \
               AND e.semantic_git_sha = $2 AND e.deleting_at IS NULL",
            [workspace_id.into(), git_sha.into(), revision_id.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}

/// A `push` to `branch` happened: queue a compile of the branch's head for
/// every workspace that has an environment pinned to the branch, and mark those
/// environments as waiting on it. Returns how many workspaces had a compile
/// queued by this call (one already ready, or already in flight, queues none).
///
/// **The commit compiled is the branch's head as GitHub has it now**
/// (`Target::Branch`), not the push's `after`. A redelivered or out-of-order
/// delivery for an older commit therefore cannot roll a pin back: whatever the
/// delivery says, the waiting sha is the newest one. (The alternative, trusting
/// `push.after` and skipping the write when it is stale, needs the same GitHub
/// lookup plus a second code path.)
///
/// An environment **matches** when it is live (`deleting_at IS NULL`), is not
/// production, belongs to an app of the workspace (`apps.project_id`), and its
/// `planes` name the branch in either wire form: `{"semantic": "branch:<b>"}`
/// or the object form `{"semantic": {"mode": "branch", "branch": "<b>"}}` that
/// a plane carrying settings takes. A workspace is only considered when the
/// push is for its repository and touches its subdirectory, the same match the
/// default-branch path makes, minus the default-branch test: it is the
/// *environment* that names this branch.
///
/// Per workspace, in this order, so no interleaving of the worker and this
/// handler can lose the move:
///
/// 1. compile the branch's head (only queues; reuses a ready revision);
/// 2. store `semantic_git_sha = head` on the matching rows, **leaving
///    `semantic_revision_id` alone**, so they keep serving the old model until
///    the new one is ready;
/// 3. a `ready` answer moves them at once ([`move_after_compile`]); anything
///    else looks once more for a compile that landed before step 2
///    ([`settle_pending_pin`]), and otherwise the worker's own move lands it.
///
/// A push that arrives while an older commit is still compiling just replaces
/// the waiting sha, and the older compile then matches nothing
/// ([`move_after_compile`] matches on the sha). **One workspace's failure does
/// not hide the others'**: a compile that cannot be queued is logged and
/// skipped, and a database error is logged, remembered and the loop goes on;
/// the first such error is returned at the end (the way [`super::push::handle`]
/// treats its own loop). A deleted branch never reaches here: the caller has no
/// head for it.
#[tracing::instrument(skip(db, push))]
pub async fn on_branch_push(
    db: &DatabaseConnection,
    push: &PushEvent,
    branch: &str,
) -> Result<usize, DbErr> {
    let mut queued = 0;
    let mut failed: Option<DbErr> = None;
    for workspace in pinned_workspaces(db, push, branch).await? {
        match follow_push(db, &workspace, branch).await {
            Ok(was_queued) => queued += usize::from(was_queued),
            Err(e) => {
                tracing::warn!(
                    target: "compile_reconcile", workspace_id = %workspace.id, %branch,
                    error = %e,
                    "a push to a pinned branch could not be recorded for a workspace's environments"
                );
                failed.get_or_insert(e);
            }
        }
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(queued),
    }
}

/// One workspace's steps of [`on_branch_push`]. `Ok(true)` when a compile was
/// queued for it; a refusal to compile is logged and is `Ok(false)`.
async fn follow_push(
    db: &DatabaseConnection,
    workspace: &workspaces::Model,
    branch: &str,
) -> Result<bool, DbErr> {
    let target = Target::Branch(branch);
    let state = match compile_request::compile(db, workspace, target, RevisionKind::Staging).await {
        Ok(state) => state,
        Err(refusal) => {
            tracing::warn!(
                target: "compile_reconcile", workspace_id = %workspace.id, %branch,
                error = %refusal,
                "a push to a pinned branch could not be compiled for its environments"
            );
            return Ok(false);
        }
    };
    mark_waiting(db, workspace.id, branch, &state.git_sha).await?;
    match state.revision_id.filter(|_| state.status == "ready") {
        Some(revision_id) => {
            move_after_compile(db, workspace.id, &state.git_sha, revision_id).await?;
        }
        None => {
            settle_pending_pin(db, workspace.id, &state.git_sha).await?;
        }
    }
    Ok(state.task_id.is_some())
}

/// The SQL condition "this environment's semantic plane names the branch bound
/// at `$param`", in both wire forms. Callers add the liveness and kind tests.
fn names_branch(param: u8) -> String {
    format!(
        "((e.planes->>'semantic' = 'branch:' || ${param}) \
          OR (e.planes->'semantic'->>'mode' = 'branch' \
              AND e.planes->'semantic'->>'branch' = ${param}))"
    )
}

/// Ready workspaces with a live environment pinned to `branch`, narrowed to the
/// ones the push is for (repository and subdirectory).
async fn pinned_workspaces(
    db: &DatabaseConnection,
    push: &PushEvent,
    branch: &str,
) -> Result<Vec<workspaces::Model>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "SELECT DISTINCT a.project_id FROM app_environments e \
                 JOIN apps a ON a.id = e.app_id \
                 WHERE e.deleting_at IS NULL AND e.kind <> 'production' AND {}",
                names_branch(1)
            ),
            [branch.into()],
        ))
        .await?;
    let ids = rows
        .iter()
        .map(|r| r.try_get::<Uuid>("", "project_id"))
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let candidates = workspaces::Entity::find()
        .filter(workspaces::Column::Id.is_in(ids))
        .filter(workspaces::Column::Status.eq(WorkspaceStatus::Ready))
        .filter(workspaces::Column::GitRemoteUrl.is_not_null())
        .all(db)
        .await?;
    Ok(candidates
        .into_iter()
        .filter(|w| {
            let slug = w.git_remote_url.as_deref().and_then(github_slug_of);
            slug.is_some_and(|(owner, repo)| push.is_for(&owner, &repo))
                && push.touches(w.repo_subdir.as_deref())
        })
        .collect())
}

/// Store `git_sha` as the commit the matching environments of `workspace_id`
/// wait on. `semantic_revision_id` is deliberately not written.
async fn mark_waiting(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    branch: &str,
    git_sha: &str,
) -> Result<u64, DbErr> {
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            format!(
                "UPDATE app_environments e SET semantic_git_sha = $3 FROM apps a \
                 WHERE e.app_id = a.id AND a.project_id = $1 \
                   AND e.deleting_at IS NULL AND e.kind <> 'production' AND {}",
                names_branch(2)
            ),
            [workspace_id.into(), branch.into(), git_sha.into()],
        ))
        .await?;
    Ok(result.rows_affected())
}
