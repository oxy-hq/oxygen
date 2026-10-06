//! The queue-facing side of a commit compile: how one is enqueued, and what
//! the compile worker calls to get its tree.

use std::time::Instant;

use agentic_core::delegation::TaskSpec;
use agentic_runtime::coordinator::COMPILE_GIT_SOURCE_TYPE;
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use oxy_compile::RevisionKind;
use oxy_telemetry::metrics::record;
use sea_orm::{DatabaseConnection, DbErr};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{CommitSource, CompileGitError, FetchedTree, fetch};

/// Queue a compile of commit `git_sha` as a `kind` revision, for any pod to
/// run. Returns the task id, which is also the run id.
///
/// The run is stamped [`COMPILE_GIT_SOURCE_TYPE`], which is the whole reason a
/// worker will take it: selection matches on `source_type`, and a worker
/// declines `compile`. The task payload stays a `TaskSpec::Compile`, so the
/// queries that find "a compile for this workspace" by `spec->>'type'` see
/// both kinds and the index that serves them still applies.
///
/// Does not validate `git_sha` or the workspace's remote: the worker does, and
/// fails the task with a typed message. A caller with a person on the other
/// end should check first ([`super::commit_sha`]) and answer them directly.
///
/// `promote` is only a request: the compiler promotes a `main` revision and no
/// other kind, whatever is asked here.
pub async fn enqueue(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
    branch: Option<&str>,
    kind: RevisionKind,
    promote: bool,
) -> Result<String, DbErr> {
    let task_id = Uuid::new_v4().to_string();
    // `agentic_task_queue.run_id` references `agentic_runs.id`, so the run row
    // comes first. One task per run: a compile does not fan out.
    agentic_runtime::crud::insert_run(
        db,
        &task_id,
        &format!("compile {} ({git_sha}) from git", kind.as_str()),
        None,
        COMPILE_GIT_SOURCE_TYPE,
        Some(serde_json::json!({
            "workspace_id": workspace_id,
            "git_sha": git_sha,
            "branch": branch,
            "kind": kind.as_str(),
        })),
        workspace_id,
    )
    .await?;
    let spec = TaskSpec::Compile {
        workspace_id,
        git_sha: Some(git_sha.to_string()),
        branch: branch.map(str::to_string),
        promote,
        kind: Some(kind.as_str().to_string()),
        owner_user_id: None,
        from_git: true,
    };
    agentic_runtime::crud::enqueue_task(
        db,
        &task_id,
        &task_id,
        None,
        &spec,
        None,
        TaskScope::Global,
    )
    .await?;
    Ok(task_id)
}

/// Fetch the commit a claimed `compile_git` task names. `Err` is the task's
/// failure message ([`CompileGitError::task_failure`]).
///
/// Cancelling `cancel` abandons the fetch; the half-written directory goes
/// with it. Records `oxy_compile_fetch_duration_seconds` either way.
pub async fn fetch_for_task(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: Option<&str>,
    cancel: &CancellationToken,
) -> Result<FetchedTree, String> {
    let started = Instant::now();
    let fetched = tokio::select! {
        result = resolve_and_fetch(db, workspace_id, git_sha) => result.map_err(|e| {
            tracing::warn!(
                %workspace_id,
                code = e.code(),
                retryable = e.is_retryable(),
                error = %e,
                "compile from git: could not fetch the commit"
            );
            e.task_failure()
        }),
        () = cancel.cancelled() => {
            Err("compile from git cancelled while fetching the commit".to_string())
        }
    };
    let seconds = started.elapsed().as_secs_f64();
    record::compile_fetch_duration(fetched.is_ok(), seconds);
    if let Ok(tree) = &fetched {
        tracing::info!(
            %workspace_id,
            files = tree.unpacked.files,
            bytes = tree.unpacked.bytes,
            links_skipped = tree.unpacked.links_skipped,
            seconds,
            "compile from git: fetched the commit"
        );
    }
    fetched
}

async fn resolve_and_fetch(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: Option<&str>,
) -> Result<FetchedTree, CompileGitError> {
    let source = CommitSource::resolve(db, workspace_id, git_sha).await?;
    fetch(&source).await
}
