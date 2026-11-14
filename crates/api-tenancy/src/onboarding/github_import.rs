//! Where a GitHub import lands, and its first compile.
//!
//! An org has one workspace. The import connects the repository to it when it
//! has none yet (`workspace_provisioning::adopt`), creates it only for an org
//! that has no workspace at all, and refuses (`409 org_has_workspace`) when the
//! org's workspace already has a repository.
//!
//! **Nothing is cloned.** The repository is recorded on the row, the row
//! settles Ready, and a compile of the default branch's head is queued from
//! git — the same compile a push to that branch queues. So the import runs on
//! any replica, and no pod holds a working copy of the repository
//! (`internal-docs/factory-retirement.md`, phase 6). Whether the repository is
//! an Oxygen project is checked first, through the GitHub API, by the caller.

use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use entity::workspaces::WorkspaceStatus;
use oxy::github::GitHubRepository;
use tracing::{error, info};
use uuid::Uuid;

use super::ops::{register_project, unique_display_name};
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

/// The workspace an import landed on, still reading Cloning.
pub(super) struct PlacedImport {
    pub workspace_id: Uuid,
    /// The remote this attempt recorded; the row's final status is written
    /// against it (`adopt::settle`).
    remote_url: String,
    /// What the claim replaced, for an adopted workspace, so a failed settle
    /// can give the row back (`adopt::release`). `None` for a new workspace.
    claim: Option<adopt::Claim>,
    /// Where the adopted workspace's old files were set aside, if this process
    /// held any, so giving the row back puts them back too.
    aside: Option<std::path::PathBuf>,
    /// The connection the placement used; [`connect`] settles on it.
    db: sea_orm::DatabaseConnection,
}

/// Record the import on the org's workspace (or create it). The row reads
/// Cloning when this returns; [`connect`] settles it.
pub(super) async fn place_import(req: ImportRequest<'_>) -> Result<PlacedImport, HandlerError> {
    let db = db().await?;
    match adopt::import_target(&db, req.org_id)
        .await
        .map_err(logged)?
    {
        ImportTarget::New => place_new(db, req).await,
        ImportTarget::Adopt(workspace_id) => place_adopted(db, req, workspace_id).await,
    }
}

async fn place_new(
    db: sea_orm::DatabaseConnection,
    req: ImportRequest<'_>,
) -> Result<PlacedImport, HandlerError> {
    let name = match req.name {
        Some(n) => n.to_string(),
        None => unique_display_name(&req.repo.name, Some(req.org_id)).await?,
    };
    let workspace_id = Uuid::new_v4();
    let origin = RepositoryOrigin::from_github(req.namespace_id, req.repo, req.subdir);
    let workspace_id = register_project(
        &project_dir(workspace_id, req.subdir),
        &name,
        workspace_id,
        Some(req.user_id),
        Some(req.org_id),
        WorkspaceStatus::Cloning,
        Some(origin),
    )
    .await
    .map_err(logged)?;
    Ok(PlacedImport {
        workspace_id,
        remote_url: req.repo.clone_url.clone(),
        claim: None,
        aside: None,
        db,
    })
}

/// Connect the repository to the org's existing workspace: claim the row (only
/// one import can), then move the blank workspace's files aside where this
/// process holds them (`adopt::set_aside_working_copy`), so a reader that
/// prefers a working copy never serves them as the repository's. A set-aside
/// that fails releases the claim, so the import can be retried.
async fn place_adopted(
    db: sea_orm::DatabaseConnection,
    req: ImportRequest<'_>,
    workspace_id: Uuid,
) -> Result<PlacedImport, HandlerError> {
    let origin = RepositoryOrigin::from_github(req.namespace_id, req.repo, req.subdir);
    let claim = adopt::claim(
        &db,
        workspace_id,
        &project_dir(workspace_id, req.subdir),
        origin,
        req.name,
    )
    .await
    .map_err(logged)?;
    let aside = match adopt::set_aside_working_copy(workspace_id) {
        Ok(aside) => aside,
        Err(e) => {
            if let Err(release) = adopt::release(&db, claim).await {
                error!(%workspace_id, error = %release, "import: could not release the workspace claim");
            }
            return Err(logged(e));
        }
    };
    Ok(PlacedImport {
        workspace_id: claim.workspace_id(),
        remote_url: req.repo.clone_url.clone(),
        claim: Some(claim),
        aside,
        db,
    })
}

/// Settle the placed row Ready and queue the compile of `head` (the default
/// branch's head commit). A settle that fails gives the row back, so the
/// retry is not refused as `org_has_workspace` until the claim goes stale. A
/// compile that cannot be queued now is queued by the branch check, which a
/// request for the workspace brings forward, and by the next push — both of
/// which [`resolve_import_head`] required to be on.
pub(super) async fn connect(
    placed: PlacedImport,
    head: String,
    branch: String,
) -> Result<(), HandlerError> {
    let PlacedImport {
        workspace_id,
        remote_url,
        claim,
        aside,
        db,
    } = placed;
    let written =
        match adopt::settle(&db, workspace_id, &remote_url, WorkspaceStatus::Ready, None).await {
            Ok(written) => written,
            Err(e) => {
                give_back(&db, workspace_id, &remote_url, claim, aside).await;
                return Err(logged(e));
            }
        };
    if !written {
        // Another import took the row between the claim and now.
        info!(%workspace_id, "import superseded before it settled; nothing queued");
        return Ok(());
    }
    oxy_app::surface::enqueue_import_compile(&db, workspace_id, head, branch).await;
    // Tell whoever is waiting on it. Best-effort: the workspace is connected
    // either way, and the setup screen polls until it compiles.
    if let Err(e) = oxy_app_core::notifications::platform::workspace_ready::notify_workspace_ready(
        &db,
        workspace_id,
    )
    .await
    {
        tracing::warn!(?e, %workspace_id, "workspace-ready notice not sent");
    }
    Ok(())
}

/// Undo a placement whose settle failed: an adopted row gets back what the
/// claim replaced, and its old files, if this process set any aside, go back
/// to the root; a new row settles Failed, which the next import may take
/// over. Best-effort and logged: the caller is already answering with the
/// error that made it give up.
async fn give_back(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    remote_url: &str,
    claim: Option<adopt::Claim>,
    aside: Option<std::path::PathBuf>,
) {
    let result = match claim {
        Some(claim) => {
            let released = adopt::release(db, claim).await;
            adopt::restore_working_copy(workspace_id, aside);
            released
        }
        None => adopt::settle(
            db,
            workspace_id,
            remote_url,
            WorkspaceStatus::Failed,
            Some("the import could not be completed; import the repository again".into()),
        )
        .await
        .map(|_| ()),
    };
    if let Err(e) = result {
        error!(%workspace_id, error = %e, "import: could not give the workspace back");
    }
}

/// The commit the import will compile: the head of the repository's default
/// branch, once `config.yml` is found at it (inside `subdir` when given). A
/// repository without one is refused `422` before anything is recorded, and so
/// is every import on a deployment that does not compile from git.
pub(super) async fn resolve_import_head(
    client: &oxy::github::GitHubClient,
    repo_id: i64,
    repo: &GitHubRepository,
    subdir: Option<&Path>,
) -> Result<String, HandlerError> {
    require_compiles_from_git(oxy_app::surface::compiles_from_git())?;
    let head = client
        .get_branch_commit_hash(repo_id, &repo.default_branch)
        .await
        .map_err(|e| {
            logged((
                StatusCode::BAD_GATEWAY,
                format!("Failed to read the head of '{}': {e}", repo.default_branch),
            ))
        })?;
    let config = config_path_in_repo(subdir);
    let is_project = client
        .file_exists(repo_id, &config, &head)
        .await
        .map_err(|e| {
            logged((
                StatusCode::BAD_GATEWAY,
                format!("Failed to look for {config} in the repository: {e}"),
            ))
        })?;
    if !is_project {
        return Err(not_an_oxy_project(&repo.clone_url, subdir));
    }
    Ok(head)
}

/// Refuse an import on a deployment that does not compile from git. The
/// import clones nothing, so its workspace is served only from commit
/// compiles; without the `compile_reconcile` flag none run (not the first, not
/// a push's), and the workspace would sit Ready with nothing ever compiled.
fn require_compiles_from_git(enabled: bool) -> Result<(), HandlerError> {
    if enabled {
        return Ok(());
    }
    Err((
        StatusCode::SERVICE_UNAVAILABLE,
        "This deployment does not compile workspaces from git yet, so a repository cannot be \
         connected. An operator turns it on with `oxyc flags set compile_reconcile --on`."
            .to_string(),
    ))
}

/// Where the workspace root would sit, as the row records it. Nothing is
/// created there: the row's `path` names it for single-process mode and the
/// CLI, which still read a working copy when one exists.
fn project_dir(workspace_id: Uuid, subdir: Option<&Path>) -> PathBuf {
    let root = oxy::adapters::workspace::workspace_root_path(workspace_id);
    match subdir {
        Some(sub) => root.join(sub),
        None => root,
    }
}

/// `config.yml` as the GitHub contents API names it, inside `subdir` when the
/// workspace root is one.
fn config_path_in_repo(subdir: Option<&Path>) -> String {
    match subdir {
        Some(sub) => format!(
            "{}/config.yml",
            sub.to_string_lossy()
                .replace('\\', "/")
                .trim_end_matches('/')
        ),
        None => "config.yml".to_string(),
    }
}

/// The refusal for a repository with no `config.yml` where the workspace root
/// would be — the words the clone used to settle `NotOxyProject` with.
fn not_an_oxy_project(clone_url: &str, subdir: Option<&Path>) -> HandlerError {
    let in_subdir = subdir
        .map(|s| format!(" in subdirectory '{}'", s.display()))
        .unwrap_or_default();
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        format!(
            "Repository '{clone_url}' does not appear to be an Oxygen project — no config.yml found{in_subdir}."
        ),
    )
}

async fn db() -> Result<sea_orm::DatabaseConnection, HandlerError> {
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
