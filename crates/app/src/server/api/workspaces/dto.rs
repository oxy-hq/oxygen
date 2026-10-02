use chrono::{DateTime, Utc};
use entity::settings::SyncStatus;
use oxy::api_types::{CommitEntry, ProjectBranch};
use oxy_git::DirtyEntry;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// API wrapper for SyncStatus that implements ToSchema
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApiSyncStatus {
    Idle,
    Syncing,
    Synced,
    Error,
}

impl From<SyncStatus> for ApiSyncStatus {
    fn from(status: SyncStatus) -> Self {
        match status {
            SyncStatus::Idle => ApiSyncStatus::Idle,
            SyncStatus::Syncing => ApiSyncStatus::Syncing,
            SyncStatus::Synced => ApiSyncStatus::Synced,
            SyncStatus::Error => ApiSyncStatus::Error,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SwitchBranchRequest {
    pub branch: String,
    /// Optional fork point when creating a new branch.  Ignored when `branch`
    /// already exists.  Defaults to git's `HEAD` of the main worktree.
    #[serde(default)]
    pub base_branch: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub enum PullState {
    /// Local branch matches origin exactly — nothing left to reconcile.
    Synced,
    /// The pull succeeded, but local commits that are not on origin remain on
    /// top. Distinct from `Synced` because it is not a resting state: those
    /// commits block the next fast-forward and make restore refuse, so the
    /// caller has something left to do (push them, or discard them).
    AheadOfRemote,
    Conflict,
    Error,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PullChangesResponse {
    pub success: bool,
    pub message: String,
    pub state: PullState,
}

/// The single source of truth for the workspace's git state. Only three
/// shapes are valid; representing them as one enum (rather than two booleans)
/// makes the impossible state `(no .git, but has remote)` unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitMode {
    /// No `.git` directory on disk. Pure local mode — no git UI.
    None,
    /// `.git` exists but no remote configured. Commits are local-only.
    Local,
    /// `.git` exists and a remote is configured (or `GIT_REPOSITORY_URL` is set).
    Connected,
}

/// What the workspace's git mode allows. Derived from `GitMode` via
/// `GitCapabilities::from(mode)` — never set ad-hoc. Adding a new git
/// operation = one row here, no scattered conditionals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GitCapabilities {
    pub can_commit: bool,
    pub can_browse_history: bool,
    pub can_reset_to_commit: bool,
    pub can_switch_branch: bool,
    pub can_diff: bool,
    pub can_push: bool,
    pub can_pull: bool,
    pub can_fetch: bool,
    pub can_force_push: bool,
    pub can_rebase: bool,
    pub can_open_pr: bool,
    pub auto_feature_branch_on_protected: bool,
}

impl From<GitMode> for GitCapabilities {
    fn from(mode: GitMode) -> Self {
        let local = matches!(mode, GitMode::Local | GitMode::Connected);
        let connected = matches!(mode, GitMode::Connected);
        Self {
            can_commit: local,
            can_browse_history: local,
            can_reset_to_commit: local,
            can_switch_branch: local,
            can_diff: local,
            can_push: connected,
            can_pull: connected,
            can_fetch: connected,
            can_force_push: connected,
            can_rebase: connected,
            can_open_pr: connected,
            auto_feature_branch_on_protected: connected,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceDetailsResponse {
    pub id: Uuid,
    pub name: String,
    pub workspace_id: Uuid,
    pub active_branch: Option<ProjectBranch>,
    /// Set only when the working copy is on a detached HEAD: the short sha it
    /// is checked out at. `active_branch.name` is then the `HEAD@<sha>` label,
    /// which is not a branch — reads and saves address the working copy as it
    /// is, and operations that need a branch answer `409 detached_head`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detached_head: Option<String>,
    pub created_at: String,
    pub updated_at: String,

    /// True when this workspace is registered but its directory does not exist
    /// on disk (e.g. deleted externally). Frontend should show a toast.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_error: Option<String>,

    /// Single source of truth for the workspace's git state.
    pub git_mode: GitMode,

    /// What the current `git_mode` allows. Derived from `git_mode`; the
    /// frontend should branch on these flags rather than on `git_mode`
    /// directly so that adding a new operation only requires one change.
    pub capabilities: GitCapabilities,

    /// Default branch (e.g. "main"). Only meaningful when `git_mode != None`.
    pub default_branch: String,

    /// Branches where saving a file auto-creates a feature branch. Configured
    /// via `protected_branches` in config.yml; defaults to `[default_branch]`.
    pub protected_branches: Vec<String>,

    /// True when this workspace is in local mode and no `config.yml` is
    /// resolvable. The frontend should render the setup dialog instead of
    /// the main app. Always `false` in cloud mode.
    #[serde(default)]
    pub requires_local_setup: bool,

    /// The authenticated user's effective role in this workspace
    /// (`"owner" | "admin" | "member" | "viewer"`). Lets the UI gate
    /// destructive actions without a 403 roundtrip.
    pub current_user_role: String,

    /// See `compute_workspace_storage_key`.
    pub storage_key: String,
}

/// The half of [`WorkspaceDetailsResponse`] that comes from Postgres.
///
/// Split out because `/details` does two jobs, and the git half is the only
/// reason the route needs the ide. Served from any replica.
///
/// `active_branch` is deliberately NOT here — it comes from
/// `git.get_current_branch()`, not a column. There is no branch field on the
/// workspaces entity.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceMetaResponse {
    pub id: Uuid,
    pub name: String,
    pub workspace_id: Uuid,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_error: Option<String>,
    /// See `compute_workspace_storage_key`.
    pub storage_key: String,
    /// The authenticated user's effective role in this workspace.
    pub current_user_role: String,
    /// True when this workspace is in local mode and no `config.yml` is
    /// resolvable — the frontend renders the setup dialog instead of the app.
    #[serde(default)]
    pub requires_local_setup: bool,
}

/// The half of [`WorkspaceDetailsResponse`] that needs `.git`.
///
/// Live state — branch, mode, capabilities — so it cannot be compiled and the
/// route stays IdeOnly. When the ide is unreachable this 502s, and the frontend
/// treats a missing git half exactly as it treats today's degraded
/// `git_mode: None`: no branch, so the queries gated on one stay closed.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceGitStateResponse {
    pub active_branch: Option<ProjectBranch>,
    /// See [`WorkspaceDetailsResponse::detached_head`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detached_head: Option<String>,
    pub git_mode: GitMode,
    pub capabilities: GitCapabilities,
    pub default_branch: String,
    pub protected_branches: Vec<String>,
}

// BranchType and ProjectBranch imported from oxy::api_types

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceBranchesResponse {
    pub branches: Vec<ProjectBranch>,
}

#[derive(Deserialize)]
pub struct ResolveConflictQuery {
    pub branch: Option<String>,
    pub file: String,
    /// `"mine"` = keep your local version; `"theirs"` = accept the remote version
    pub side: String,
}

#[derive(Deserialize)]
pub struct ResolveConflictWithContentQuery {
    pub branch: Option<String>,
    pub file: String,
}

#[derive(Deserialize)]
pub struct UnresolveConflictQuery {
    pub branch: Option<String>,
    pub file: String,
}

#[derive(Deserialize)]
pub struct RecentCommitsQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Deserialize)]
pub struct ResetToCommitQuery {
    pub branch: Option<String>,
    pub commit: String,
    /// When `true`, discard all working-tree changes (tracked + untracked) and
    /// restore. When `false` (default), the call refuses if the tree is dirty
    /// and returns the file list so the UI can confirm.
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize)]
pub struct ResetToCommitResponse {
    pub success: bool,
    pub message: String,
    /// Populated only when `success=false` and the working tree was dirty.
    /// The UI should show these files in a confirmation dialog and re-call
    /// the endpoint with `force=true` if the user confirms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty: Option<Vec<DirtyEntry>>,
    /// Populated only when `success=false` because the restore would drop
    /// commits. Same contract as `dirty`: show them, then re-call with
    /// `force=true` on confirm. Each entry's `on_remote` says whether losing it
    /// is cheap (never pushed) or expensive (already on origin).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discarded_commits: Option<Vec<CommitEntry>>,
}

#[derive(Deserialize)]
pub struct ResolveConflictWithContentBody {
    pub content: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct PushChangesRequest {
    pub commit_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProjectStatus {
    pub required_secrets: Option<Vec<String>>,
    pub is_config_valid: bool,
    pub error: Option<String>,
}

/// Summary of a registered workspace returned by `GET /orgs/{org_id}/workspaces`.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub name: String,
    pub path: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_opened_at: Option<DateTime<Utc>>,
    /// Display name of the user who created this workspace, if known.
    pub created_by_name: Option<String>,
    /// Number of `.agent.yml` files found (recursive), or `None` when this
    /// instance holds no working copy for the workspace and therefore did not
    /// look. Zero means "counted, and there are none" — a replica reporting the
    /// two as the same number is how a healthy workspace came to render
    /// "0 agents · 0 automations · 0 apps".
    pub agent_count: Option<usize>,
    /// Number of automation files found (recursive) — `.automation.yml` plus the
    /// `.procedure.yml` / `.workflow.yml` legacy spellings, matching
    /// `ConfigManager::list_workflows` and the "automations" the UI labels them.
    /// `None` as above.
    pub workflow_count: Option<usize>,
    /// Number of `.app.yml` files found (recursive). `None` as above.
    pub app_count: Option<usize>,
    /// Git remote URL (e.g. `https://github.com/org/repo`), if set.
    pub git_remote: Option<String>,
    /// Short commit hash + message of HEAD, if available.
    pub git_commit: Option<String>,
    /// Human-readable relative date of the last commit (e.g. "3 hours ago").
    pub git_updated_at: Option<String>,
    /// Serialised as a string. Typed opaquely here rather than deriving
    /// `ToSchema` on the entity enum: that would pull utoipa into `entity`,
    /// which every consumer of the crate then carries for one doc string.
    #[schema(value_type = String)]
    pub status: entity::workspaces::WorkspaceStatus,
    pub error: Option<String>,
}

/// PATCH /workspaces/{id}/rename — change the display name of a workspace.
#[derive(Deserialize)]
pub struct RenameWorkspaceRequest {
    pub name: String,
}

/// DELETE /workspaces/{id} — remove a workspace record from the database.
///
/// Pass `?delete_files=true` to also remove the workspace directory from disk.
/// Without that flag only the DB record is removed, leaving files intact.
/// Requires Admin or Owner role.
#[derive(Deserialize)]
pub struct DeleteProjectQuery {
    #[serde(default)]
    pub delete_files: bool,
}
