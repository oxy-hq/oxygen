//! Compile a commit with no working copy: fetch it, unpack it, and hand the
//! directory to the unchanged compiler.
//!
//! `oxy_compile::compile_workspace` reads a directory and takes the git SHA
//! only as a label, which is why compile has needed the one pod that holds a
//! checkout. A `compile_git` task names a real commit instead, so any pod can
//! run it: [`CommitSource::resolve`] works out where the commit lives and
//! with which token, and [`fetch`] downloads it as a tarball into a temporary
//! directory that is gone when the returned [`FetchedTree`] is dropped.
//!
//! Every way this can fail has its own [`CompileGitError`] variant and a
//! stable [`CompileGitError::code`], because the reader of a failed compile is
//! an operator deciding what to do next — reconnect GitHub, commit a data
//! file, push again, or just wait.
//!
//! Plan and what is still unbuilt: `internal-docs/factory-retirement.md`,
//! phase 1.

mod download;
mod preflight;
mod remote;
mod task;
mod unpack;

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use oxy::github::{GitHubClient, TarballError, github_token_for_workspace};
use sea_orm::{DatabaseConnection, EntityTrait};
use tempfile::TempDir;
use uuid::Uuid;

pub use download::MAX_ARCHIVE_BYTES;
pub use task::{enqueue, fetch_for_task};
pub use unpack::{Limits, MAX_TREE_BYTES, MAX_TREE_FILES, UnpackError, Unpacked};

use crate::server::workspace_repo_facts::stored_repo_subdir;

#[derive(Debug, thiserror::Error)]
pub enum CompileGitError {
    #[error("workspace {0} is not registered")]
    WorkspaceNotFound(Uuid),
    #[error(
        "workspace {0} has no git remote, so there is no commit to fetch; \
         compile its working copy instead"
    )]
    NoRemote(Uuid),
    #[error(
        "remote {0:?} is not a github.com repository; only GitHub remotes can be \
         compiled from a commit"
    )]
    UnsupportedRemote(String),
    #[error("a commit compile needs a full 40-character commit SHA, got {0:?}")]
    NotACommit(String),
    #[error(
        "workspace {0} has no GitHub connection to fetch with; link it to a GitHub App \
         installation or a personal access token"
    )]
    NoToken(Uuid),
    #[error("could not get a GitHub token for workspace {workspace_id}: {message}")]
    TokenFailed { workspace_id: Uuid, message: String },
    #[error(transparent)]
    GitHub(#[from] TarballError),
    #[error("the commit's archive is larger than the {limit}-byte download limit")]
    ArchiveTooLarge { limit: u64 },
    #[error(transparent)]
    Unpack(#[from] UnpackError),
    #[error(
        "the commit has no directory {0:?}, which is where this workspace is recorded \
         to live in its repository (workspaces.repo_subdir)"
    )]
    SubdirMissing(String),
    #[error(
        "the commit has no config.yml at the workspace root; if the workspace lives in a \
         subdirectory of its repository, workspaces.repo_subdir is not recorded yet"
    )]
    NoConfig,
    #[error(
        "DuckDB database {database:?} reads {path:?}, which is not in the commit. A commit \
         compile can only mirror data that is committed; commit the files, or compile the \
         working copy on the node that has them"
    )]
    DuckDbDataMissing { database: String, path: String },
    #[error(
        "DuckDB database {database:?} reads {path:?}, which is outside the commit. A commit \
         compile only reads paths inside the workspace; make it relative, with no `..`"
    )]
    DuckDbPathOutsideTree { database: String, path: String },
    #[error(
        "DuckDB database {database:?} is a local file and no S3 mirror bucket is configured \
         (OXY_COMPILE_BLOB_S3_BUCKET), so the compiled revision could not read it once the \
         fetched tree is gone"
    )]
    DuckDbMirrorUnconfigured { database: String },
    #[error("database error: {0}")]
    Database(String),
    #[error("could not prepare a directory for the fetched commit: {0}")]
    Io(String),
}

impl CompileGitError {
    pub(crate) fn io(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }

    /// A stable name for the failure, for logs and the task's failure message.
    pub fn code(&self) -> &'static str {
        match self {
            Self::WorkspaceNotFound(_) => "workspace_not_found",
            Self::NoRemote(_) => "no_remote",
            Self::UnsupportedRemote(_) => "unsupported_remote",
            Self::NotACommit(_) => "not_a_commit",
            Self::NoToken(_) => "no_token",
            Self::TokenFailed { .. } => "token_failed",
            Self::GitHub(TarballError::NotFound { .. }) => "commit_not_found",
            Self::GitHub(TarballError::Denied { .. }) => "github_denied",
            Self::GitHub(TarballError::Unavailable(_)) => "github_unavailable",
            Self::ArchiveTooLarge { .. } => "archive_too_large",
            Self::Unpack(UnpackError::TooManyFiles { .. } | UnpackError::TooLarge { .. }) => {
                "tree_too_large"
            }
            Self::Unpack(UnpackError::Io(_)) | Self::Io(_) => "io",
            Self::Unpack(_) => "bad_archive",
            Self::SubdirMissing(_) => "subdir_missing",
            Self::NoConfig => "no_config",
            Self::DuckDbDataMissing { .. } => "duckdb_data_missing",
            Self::DuckDbPathOutsideTree { .. } => "duckdb_path_outside_tree",
            Self::DuckDbMirrorUnconfigured { .. } => "duckdb_mirror_unconfigured",
            Self::Database(_) => "database",
        }
    }

    /// Whether the same task could succeed if it simply ran again: GitHub or
    /// Postgres was briefly unavailable. Everything else needs a person, or a
    /// different commit.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::GitHub(TarballError::Unavailable(_))
                | Self::TokenFailed { .. }
                | Self::Database(_)
        )
    }

    /// The task's failure message: the code first, so it can be grouped on.
    pub fn task_failure(&self) -> String {
        let retry = if self.is_retryable() {
            ", retryable"
        } else {
            ""
        };
        format!("compile from git failed [{}{retry}]: {self}", self.code())
    }
}

/// Everything needed to fetch one commit of one workspace.
#[derive(Clone)]
pub struct CommitSource {
    pub workspace_id: Uuid,
    pub owner: String,
    pub repo: String,
    pub sha: String,
    token: String,
    /// `workspaces.repo_subdir`; `None` is the repository root.
    pub repo_subdir: Option<String>,
}

/// Written by hand so the token cannot be printed: a derived `Debug` would put
/// it in the first log line that formats a `CommitSource` with `?`.
impl std::fmt::Debug for CommitSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommitSource")
            .field("workspace_id", &self.workspace_id)
            .field("owner", &self.owner)
            .field("repo", &self.repo)
            .field("sha", &self.sha)
            .field("token", &"<redacted>")
            .field("repo_subdir", &self.repo_subdir)
            .finish()
    }
}

impl CommitSource {
    /// Read the workspace row and its GitHub connection. No network beyond
    /// minting an installation token.
    pub async fn resolve(
        db: &DatabaseConnection,
        workspace_id: Uuid,
        git_sha: Option<&str>,
    ) -> Result<Self, CompileGitError> {
        let row = workspace_row(db, workspace_id).await?;
        let (owner, repo) = github_repository(&row)?;
        let sha = commit_sha(git_sha)?;
        let token = github_token_for_workspace(&row)
            .await
            .map_err(|e| CompileGitError::TokenFailed {
                workspace_id,
                message: e.to_string(),
            })?
            .ok_or(CompileGitError::NoToken(workspace_id))?;
        Ok(Self {
            workspace_id,
            owner,
            repo,
            sha,
            token,
            repo_subdir: stored_repo_subdir(&row).map(str::to_string),
        })
    }
}

async fn workspace_row(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<entity::workspaces::Model, CompileGitError> {
    entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await
        .map_err(|e| CompileGitError::Database(e.to_string()))?
        .ok_or(CompileGitError::WorkspaceNotFound(workspace_id))
}

/// The `(owner, repo)` on github.com that `row`'s remote names.
pub(crate) fn github_repository(
    row: &entity::workspaces::Model,
) -> Result<(String, String), CompileGitError> {
    let remote = row
        .git_remote_url
        .as_deref()
        .ok_or(CompileGitError::NoRemote(row.id))?;
    remote::github_slug(remote)
        .ok_or_else(|| CompileGitError::UnsupportedRemote(remote.to_string()))
}

/// What can be known about a commit compile before queueing it, without
/// touching GitHub: the workspace has a GitHub remote and `git_sha` is a
/// commit id. Returns the normalised SHA. For a caller with a person waiting
/// on the answer; the worker repeats both checks when it runs the task.
pub async fn validate_request(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    git_sha: Option<&str>,
) -> Result<String, CompileGitError> {
    let row = workspace_row(db, workspace_id).await?;
    github_repository(&row)?;
    commit_sha(git_sha)
}

/// `git_sha` as a full commit id. A branch name or an abbreviation would
/// fetch — GitHub resolves either — and then label the revision with a name
/// that moves, which defeats the `(workspace_id, git_sha)` dedupe.
pub fn commit_sha(git_sha: Option<&str>) -> Result<String, CompileGitError> {
    let sha = git_sha.unwrap_or_default().trim();
    let is_full = sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit());
    if !is_full {
        return Err(CompileGitError::NotACommit(sha.to_string()));
    }
    Ok(sha.to_ascii_lowercase())
}

/// A fetched commit on disk. The directory is removed when this is dropped.
#[derive(Debug)]
pub struct FetchedTree {
    workspace_path: PathBuf,
    pub unpacked: Unpacked,
    _dir: Arc<TempDir>,
}

impl FetchedTree {
    /// The workspace root inside the fetched tree — what the compiler reads.
    ///
    /// Not named `workspace_path`: that name belongs to the managers' accessor
    /// for a working copy, which a source scan
    /// (`tests/platform/workspace_path_escape_hatch.rs`) counts by its text.
    /// This is the opposite thing — a tree that exists because there is none.
    pub fn root(&self) -> &Path {
        &self.workspace_path
    }
}

/// Download `source`'s commit, unpack it, and locate the workspace in it.
pub async fn fetch(source: &CommitSource) -> Result<FetchedTree, CompileGitError> {
    let client = GitHubClient::from_token(source.token.clone()).map_err(|e| {
        CompileGitError::TokenFailed {
            workspace_id: source.workspace_id,
            message: e.to_string(),
        }
    })?;
    let response = client
        .commit_tarball(&source.owner, &source.repo, &source.sha)
        .await?;

    let dir = tempfile::Builder::new()
        .prefix("oxy-compile-git-")
        .tempdir()
        .map_err(CompileGitError::io)?;
    // Shared with the blocking unpack below: if this future is dropped
    // mid-unpack (the task was cancelled), the directory must outlive the
    // thread still writing into it, or that thread recreates it and it leaks.
    let dir = Arc::new(dir);
    let archive = dir.path().join("archive.tar.gz");
    download::save_body(response, &archive, MAX_ARCHIVE_BYTES).await?;

    let tree = dir.path().join("tree");
    let unpacked = unpack_blocking(dir.clone(), archive, tree.clone()).await?;
    let workspace_path = workspace_root_in(&tree, source.repo_subdir.as_deref())?;
    preflight::check(&workspace_path, oxy_compile::blob_store::bucket().is_some())?;
    Ok(FetchedTree {
        workspace_path,
        unpacked,
        _dir: dir,
    })
}

async fn unpack_blocking(
    dir: Arc<TempDir>,
    archive: PathBuf,
    tree: PathBuf,
) -> Result<Unpacked, CompileGitError> {
    tokio::task::spawn_blocking(move || {
        let _keep = dir;
        std::fs::create_dir(&tree).map_err(CompileGitError::io)?;
        let file = std::fs::File::open(&archive).map_err(CompileGitError::io)?;
        let unpacked = unpack::unpack_commit_tarball(file, &tree, Limits::PRODUCTION)?;
        // The archive has served its purpose; give its bytes back before the
        // compile runs.
        let _ = std::fs::remove_file(&archive);
        Ok(unpacked)
    })
    .await
    .map_err(|e| CompileGitError::Io(format!("the unpack task did not finish: {e}")))?
}

/// The workspace root inside an unpacked `tree`: the tree itself, or
/// `repo_subdir` below it. The subdirectory comes from a database column, so
/// it is held to the same rule as an archive path, and must resolve inside
/// the tree on disk — a symlink the commit placed at that name does not get
/// to move the compile somewhere else.
pub(crate) fn workspace_root_in(
    tree: &Path,
    repo_subdir: Option<&str>,
) -> Result<PathBuf, CompileGitError> {
    let Some(subdir) = repo_subdir.filter(|s| !s.is_empty()) else {
        return Ok(tree.to_path_buf());
    };
    let missing = || CompileGitError::SubdirMissing(subdir.to_string());
    let relative = Path::new(subdir);
    if !relative
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(missing());
    }
    let root = tree.canonicalize().map_err(CompileGitError::io)?;
    let resolved = tree.join(relative).canonicalize().map_err(|_| missing())?;
    if !resolved.is_dir() || !resolved.starts_with(&root) {
        return Err(missing());
    }
    Ok(resolved)
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
