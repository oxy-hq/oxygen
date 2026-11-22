//! Queueing a compile task: the run row and one durable `TaskSpec::Compile`
//! on the global queue. The staging branch compile (`compile_request`) is the
//! caller. The IDE's Compile button and its status read, which compiled and
//! described the working copy, were removed with the IDE.

use axum::http::StatusCode;
use uuid::Uuid;

/// Materialise the run row and enqueue one durable `TaskSpec::Compile` on the
/// global queue, for the staging branch compile (never promoted). Returns the
/// task id, which is also the run id (compile is 1:1, no fan-out).
pub(crate) async fn enqueue_compile_task(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    git_sha: &str,
    branch: &str,
    kind: oxy_compile::RevisionKind,
    promote: bool,
) -> Result<String, (StatusCode, String)> {
    let task_id = Uuid::new_v4().to_string();
    let spec = agentic_core::delegation::TaskSpec::Compile {
        workspace_id,
        git_sha: Some(git_sha.to_string()),
        branch: Some(branch.to_string()),
        promote,
        kind: Some(kind.as_str().to_string()),
        owner_user_id: None,
        from_git: false,
    };

    // `agentic_task_queue.run_id` has a FK to `agentic_runs.id`; the
    // task insert below fails with `agentic_task_queue_run_id_fkey`
    // unless we materialise the run row first. Task and run share the
    // same UUID for compile tasks (1:1, no fan-out).
    agentic_runtime::crud::insert_run(
        db,
        &task_id,
        &format!("compile {} @ {git_sha}", kind.as_str()),
        None,
        "compile",
        Some(serde_json::json!({
            "workspace_id": workspace_id,
            "git_sha": git_sha,
            "branch": branch,
            "kind": kind.as_str(),
        })),
        workspace_id,
    )
    .await
    .map_err(|e| {
        tracing::error!(?e, "compile: insert_run failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to register compile run".into(),
        )
    })?;

    agentic_runtime::crud::enqueue_task(
        db,
        &task_id,
        &task_id,
        None,
        &spec,
        None,
        agentic_runtime::orchestrator::crud::queue::TaskScope::Global,
    )
    .await
    .map_err(|e| {
        tracing::error!(?e, "compile: enqueue failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to enqueue compile task".into(),
        )
    })?;
    Ok(task_id)
}
