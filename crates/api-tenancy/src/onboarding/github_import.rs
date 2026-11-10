//! Where a GitHub import lands, and the background clone.
//!
//! An org has one workspace. The import connects the repository to it when it
//! has none yet (`workspace_provisioning::adopt`), creates it only for an org
//! that has no workspace at all, and refuses (`409 org_has_workspace`) when the
//! org's workspace already has a repository.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::http::StatusCode;
use entity::workspaces::WorkspaceStatus;
use oxy::github::{GitHubRepository, default_git_client};
use oxy_git::GitClient;
use tracing::{error, info};
use uuid::Uuid;

use super::ops::{register_project, resolve_project_dir, unique_display_name};
use crate::workspace_provisioning::RepositoryOrigin;
use crate::workspace_provisioning::adopt::{self, ImportTarget};

type HandlerError = (StatusCode, String);

/// What the caller asked to import, already validated.
pub(super) struct ImportRequest<'a> {
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub namespace_id: Uuid,
    pub repo: &'a GitHubRepository,
    pub subdir: Option<&'a Path>,
    /// The caller's display name. A new workspace falls back to the
    /// repository's name; an adopted one keeps its own.
    pub name: Option<&'a str>,
}

/// The workspace an import landed on, and where its clone goes.
pub(super) struct PlacedImport {
    pub workspace_id: Uuid,
    pub repo_dir: PathBuf,
    pub oxy_project_dir: PathBuf,
    /// The remote this attempt recorded; the clone's final status is written
    /// against it (`adopt::settle`).
    remote_url: String,
}

/// Record the import on the org's workspace (or create it), and prepare the
/// directory the clone lands in. The row reads Cloning when this returns.
pub(super) async fn place_import(req: ImportRequest<'_>) -> Result<PlacedImport, HandlerError> {
    let db = connect().await?;
    match adopt::import_target(&db, req.org_id)
        .await
        .map_err(logged)?
    {
        ImportTarget::New => place_new(req).await,
        ImportTarget::Adopt(workspace_id) => place_adopted(&db, req, workspace_id).await,
    }
}

async fn place_new(req: ImportRequest<'_>) -> Result<PlacedImport, HandlerError> {
    let name = match req.name {
        Some(n) => n.to_string(),
        None => unique_display_name(&req.repo.name, Some(req.org_id)).await?,
    };
    let workspace_id = Uuid::new_v4();
    let (repo_dir, oxy_project_dir) = prepare_dirs(workspace_id, req.subdir)?;
    let origin = RepositoryOrigin::from_github(req.namespace_id, req.repo, req.subdir);
    // Registered now so it appears in /workspaces immediately.
    let registered = register_project(
        &oxy_project_dir,
        &name,
        workspace_id,
        Some(req.user_id),
        Some(req.org_id),
        WorkspaceStatus::Cloning,
        Some(origin),
    )
    .await;
    match registered {
        Ok(workspace_id) => Ok(PlacedImport {
            workspace_id,
            repo_dir,
            oxy_project_dir,
            remote_url: req.repo.clone_url.clone(),
        }),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&repo_dir);
            Err(logged(e))
        }
    }
}

/// Connect the repository to the org's existing workspace: claim the row
/// first (only one import can), then set its working copy aside and make
/// room for the clone. A failure before the clone puts the working copy back
/// and releases the claim; a clone that fails later leaves the row Failed or
/// NotOxyProject, which the next import may claim again.
async fn place_adopted(
    db: &sea_orm::DatabaseConnection,
    req: ImportRequest<'_>,
    workspace_id: Uuid,
) -> Result<PlacedImport, HandlerError> {
    let repo_dir = oxy::adapters::workspace::workspace_root_path(workspace_id);
    let oxy_project_dir = match req.subdir {
        Some(sub) => repo_dir.join(sub),
        None => repo_dir.clone(),
    };
    let origin = RepositoryOrigin::from_github(req.namespace_id, req.repo, req.subdir);
    let claim = adopt::claim(db, workspace_id, &oxy_project_dir, origin, req.name)
        .await
        .map_err(logged)?;
    let aside = match adopt::set_aside_working_copy(workspace_id) {
        Ok(aside) => aside,
        Err(e) => {
            release(db, claim).await;
            return Err(logged(e));
        }
    };
    match prepare_dirs(workspace_id, req.subdir) {
        Ok((repo_dir, oxy_project_dir)) => Ok(PlacedImport {
            workspace_id: claim.workspace_id(),
            repo_dir,
            oxy_project_dir,
            remote_url: req.repo.clone_url.clone(),
        }),
        Err(e) => {
            adopt::restore_working_copy(workspace_id, aside);
            release(db, claim).await;
            Err(logged(e))
        }
    }
}

/// Give the row back as it was before the claim. Logged, not returned: the
/// caller is already answering with the error that made it give up.
async fn release(db: &sea_orm::DatabaseConnection, claim: adopt::Claim) {
    let workspace_id = claim.workspace_id();
    if let Err(e) = adopt::release(db, claim).await {
        error!(%workspace_id, error = %e, "import: could not release the workspace claim");
    }
}

/// Write the clone's outcome through the fence. `false` when nothing was
/// written: the attempt was superseded, or the write failed (logged).
async fn settle(
    workspace_id: Uuid,
    remote_url: &str,
    status: WorkspaceStatus,
    message: Option<String>,
) -> bool {
    let db = match connect().await {
        Ok(db) => db,
        Err((_, e)) => {
            error!(%workspace_id, "clone status not persisted: {e}");
            return false;
        }
    };
    match adopt::settle(&db, workspace_id, remote_url, status, message).await {
        Ok(true) => true,
        Ok(false) => {
            info!(%workspace_id, "clone finished after a newer import took the workspace; its result is dropped");
            false
        }
        Err(e) => {
            error!(%workspace_id, error = %e, "clone status not persisted");
            false
        }
    }
}

/// `<state_dir>/workspaces/<id>`, and the workspace root inside it — created
/// eagerly so the row can name it before the background clone finishes.
fn prepare_dirs(
    workspace_id: Uuid,
    subdir: Option<&Path>,
) -> Result<(PathBuf, PathBuf), HandlerError> {
    let repo_dir = resolve_project_dir(workspace_id)?;
    let Some(sub) = subdir else {
        return Ok((repo_dir.clone(), repo_dir));
    };
    let dir = repo_dir.join(sub);
    std::fs::create_dir_all(&dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create subdir {dir:?}: {e}"),
        )
    })?;
    Ok((repo_dir, dir))
}

/// Clone the full repository in the background — large repositories can take
/// longer than the global request timeout — then settle the row's status and,
/// once Ready, enqueue its compile.
pub(super) fn spawn_clone(placed: PlacedImport, clone_url: String, branch: String, token: String) {
    tokio::spawn(async move {
        let PlacedImport {
            workspace_id,
            repo_dir,
            oxy_project_dir,
            remote_url,
        } = placed;
        info!("Cloning repository '{clone_url}' branch '{branch}' into {repo_dir:?}");
        let git = default_git_client();
        let clone = git.clone_or_init(&repo_dir, Some(&clone_url), &branch, Some(&token));
        let beat = || touch(workspace_id, &remote_url);
        let clone_result = match bounded(clone, adopt::MAX_CLONE, adopt::HEARTBEAT, beat).await {
            Some(result) => result.map_err(|e| e.to_string()),
            None => Err(format!(
                "timed out after {} minutes",
                adopt::MAX_CLONE.as_secs() / 60
            )),
        };
        let (status, message) = match clone_result {
            Ok(()) => cloned_status(&clone_url, &repo_dir, &oxy_project_dir),
            Err(e) => {
                error!("Background clone failed for workspace {workspace_id}: {e}");
                (
                    WorkspaceStatus::Failed,
                    Some(format!("Background clone failed: {e}")),
                )
            }
        };
        let is_ready = matches!(status, WorkspaceStatus::Ready);
        if settle(workspace_id, &remote_url, status, message).await && is_ready {
            compile_on_import(workspace_id).await;
        }
    });
}

/// Run the clone for at most `max`, calling `beat` every `every` while it
/// runs, so a slow clone never reads as stale and is never taken over. `None`
/// when `max` elapsed: the clone future is dropped (its git child with it) so
/// a hung clone cannot hold the row in `Cloning` past the bound.
async fn bounded<F, B, BF>(
    clone: F,
    max: Duration,
    every: Duration,
    mut beat: B,
) -> Option<F::Output>
where
    F: Future,
    B: FnMut() -> BF,
    BF: Future<Output = ()>,
{
    let heartbeating = async {
        tokio::pin!(clone);
        let mut ticks = tokio::time::interval(every);
        ticks.tick().await; // the first tick is immediate; the claim just touched the row
        loop {
            tokio::select! {
                result = &mut clone => return result,
                _ = ticks.tick() => beat().await,
            }
        }
    };
    tokio::time::timeout(max, heartbeating).await.ok()
}

/// One heartbeat. Best-effort: a missed beat only shortens the margin before
/// the row reads as stale.
async fn touch(workspace_id: Uuid, remote_url: &str) {
    let result = match connect().await {
        Ok(db) => adopt::heartbeat(&db, workspace_id, remote_url)
            .await
            .map_err(|e| e.to_string()),
        Err((_, e)) => Err(e),
    };
    if let Err(e) = result {
        tracing::warn!(%workspace_id, error = %e, "clone heartbeat missed");
    }
}

fn cloned_status(
    clone_url: &str,
    repo_dir: &Path,
    oxy_project_dir: &Path,
) -> (WorkspaceStatus, Option<String>) {
    info!("Repository cloned successfully into {repo_dir:?}");
    if oxy_project_dir.join("config.yml").exists() {
        return (WorkspaceStatus::Ready, None);
    }
    let in_subdir = if oxy_project_dir != repo_dir {
        format!(" in subdirectory '{}'", oxy_project_dir.display())
    } else {
        String::new()
    };
    let msg = format!(
        "Repository '{clone_url}' does not appear to be an Oxygen project — no config.yml found{in_subdir}."
    );
    error!("{msg}");
    (WorkspaceStatus::NotOxyProject, Some(msg))
}

/// Compile-on-import: a freshly-cloned workspace has no compiled revision of
/// its repository, so the serve fleet would fall back to the ide node on first
/// open. Enqueue a promoting compile now (deduped) so it's servable from
/// Postgres ASAP. Best-effort: a connect failure just defers compilation to
/// the lazy self-heal on first serve.
async fn compile_on_import(workspace_id: Uuid) {
    let db = match oxy::database::client::establish_connection().await {
        Ok(db) => db,
        Err(e) => {
            tracing::warn!(?e, %workspace_id, "compile-on-import: db connect failed; deferring to lazy self-heal");
            return;
        }
    };
    oxy_app::surface::enqueue_lazy_compile(&db, workspace_id).await;
    // Tell whoever is waiting on it. Best-effort: the workspace is ready
    // either way, and the list polls while it clones.
    if let Err(e) = oxy_app_core::notifications::platform::workspace_ready::notify_workspace_ready(
        &db,
        workspace_id,
    )
    .await
    {
        tracing::warn!(?e, %workspace_id, "workspace-ready notice not sent");
    }
}

async fn connect() -> Result<sea_orm::DatabaseConnection, HandlerError> {
    oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Database connection failed: {e}"),
            )
        })
}

fn logged(e: impl Into<HandlerError>) -> HandlerError {
    let (status, msg) = e.into();
    error!("{msg}");
    (status, msg)
}

#[cfg(test)]
#[path = "github_import_tests.rs"]
mod tests;
