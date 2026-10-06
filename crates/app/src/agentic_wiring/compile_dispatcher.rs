//! Host impl of [`agentic_pipeline::platform::CompileDispatcher`].
//!
//! Owns the bridge between the runtime executor's `TaskSpec::Compile` arm
//! (which lives in agentic-pipeline, no entity dep) and the actual compile
//! worker (which lives in the host, calls `oxy_compile::*` + `entity`).
//!
//! The worker resolves the workspace path from the DB rather than from the
//! pipeline's bound `PlatformContext`: compile tasks are claimed `Global` by
//! any worker, so the bound platform's workspace would silently override the
//! compile's intended target.

use std::sync::Arc;

use agentic_pipeline::platform::CompileDispatcher;
use agentic_runtime::worker::ExecutingTask;
use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::server::compile_worker;

pub struct OxyCompileDispatcher {
    db: Arc<DatabaseConnection>,
}

impl OxyCompileDispatcher {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }

    /// The worktree path of `branch` (subdirectory-aware, via
    /// `effective_workspace_path`). Errors when the branch has no worktree:
    /// `effective_workspace_path` falls back to the main working copy there,
    /// which would compile main's content under the branch's SHA.
    async fn branch_worktree(
        &self,
        workspace_id: Uuid,
        branch: &str,
    ) -> Result<std::path::PathBuf, String> {
        use oxy_git::GitClient;
        use sea_orm::EntityTrait;
        let row = entity::workspaces::Entity::find_by_id(workspace_id)
            .one(self.db.as_ref())
            .await
            .map_err(|e| format!("compile: {e}"))?
            .ok_or_else(|| format!("compile: workspace {workspace_id} not found"))?;
        let root = row
            .path
            .as_deref()
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let git = oxy::github::default_git_client();
        if branch != git.get_default_branch(&root).await
            && git.get_worktree_path(&root, branch).is_none()
        {
            return Err(format!(
                "compile: branch {branch:?} has no worktree on this node — re-run the staging compile"
            ));
        }
        oxy::adapters::workspace::effective_workspace_path(&row, Some(branch))
            .await
            .map_err(|e| format!("compile: {e}"))
    }
}

#[async_trait]
impl CompileDispatcher for OxyCompileDispatcher {
    async fn dispatch(
        &self,
        workspace_id: Uuid,
        git_sha: Option<String>,
        branch: Option<String>,
        promote: bool,
        kind: Option<String>,
        owner_user_id: Option<Uuid>,
        from_git: bool,
    ) -> Result<ExecutingTask, String> {
        if from_git {
            // A commit compile reads no working copy, so none of the checks
            // below apply: there is no path to resolve and no directory that
            // has to be on this node. The worker fetches the commit itself,
            // once the task is executing and can be heartbeated and cancelled.
            let spec = compile_worker::spec_from_taskspec(
                workspace_id,
                std::path::PathBuf::new(),
                git_sha,
                branch,
                promote,
                kind.as_deref(),
                owner_user_id,
            )?;
            let spec = compile_worker::CompileSpec {
                from_git: true,
                ..spec
            };
            return Ok(compile_worker::CompileWorker::new(self.db.clone()).execute(spec));
        }
        let workspace_path = oxy_compile::resolve_workspace_path(&self.db, workspace_id)
            .await
            .map_err(|e| format!("compile: {e}"))?;
        // A staging compile reads the BRANCH's worktree, not the main working
        // copy. The enqueue route (`compile_staging`) already created it and
        // checked it is clean; resolve it the way the IDE does.
        let workspace_path = match (kind.as_deref(), branch.as_deref()) {
            (Some("staging"), Some(b)) => self.branch_worktree(workspace_id, b).await?,
            _ => workspace_path,
        };
        if !workspace_path.is_dir() {
            return Err(format!(
                "compile: workspace {workspace_id} path {} does not exist on this worker — \
                 a working-copy compile runs only on a node that already holds the files \
                 (the IDE/build singleton with OXY_INPROC_GLOBAL_WORKER=1). A pod without \
                 them can compile a pushed commit instead (a `compile_git` task; see \
                 internal-docs/factory-retirement.md).",
                workspace_path.display()
            ));
        }

        let spec = compile_worker::spec_from_taskspec(
            workspace_id,
            workspace_path,
            git_sha,
            branch,
            promote,
            kind.as_deref(),
            owner_user_id,
        )?;
        let worker = compile_worker::CompileWorker::new(self.db.clone());
        Ok(worker.execute(spec))
    }
}
