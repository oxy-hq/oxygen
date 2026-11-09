use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GitHubRepository {
    pub id: i64,
    pub name: String,
    pub full_name: String,
    pub default_branch: String,
    pub clone_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GitHubBranch {
    pub name: String,
}

/// GitHub synchronization status of a connected repository.
///
/// Formerly a column type of the single-row `settings` table in `crates/entity`;
/// that table was dropped in `m20261108_000300_drop_dead_legacy_tables`, and
/// this is the plain value type the code kept using.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncStatus {
    Idle,
    Syncing,
    Synced,
    Error,
}

#[derive(Debug, Clone)]
pub struct GitHubSettings {
    pub app_installation_id: String,
    pub selected_repo_id: Option<i64>,
    pub revision: Option<String>,
    pub sync_status: SyncStatus,
    pub is_onboarded: bool,
}

impl GitHubSettings {
    pub fn new(app_installation_id: String) -> Self {
        Self {
            app_installation_id,
            selected_repo_id: None,
            revision: None,
            sync_status: SyncStatus::Idle,
            is_onboarded: false,
        }
    }

    pub fn with_repository(mut self, repo_id: i64) -> Self {
        self.selected_repo_id = Some(repo_id);
        self
    }

    pub fn with_revision(mut self, revision: String) -> Self {
        self.revision = Some(revision);
        self
    }

    pub fn with_sync_status(mut self, status: SyncStatus) -> Self {
        self.sync_status = status;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectStatus {
    pub requires_onboarding: bool,
    pub current_repository: Option<GitHubRepository>,
}

/// Current project information for authenticated users
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentProject {
    pub repository: Option<GitHubRepository>,
    pub local_path: Option<String>,
    pub sync_status: ProjectSyncStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub enum ProjectSyncStatus {
    #[serde(rename = "synced")]
    Synced,
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "error")]
    Error(String),
    #[serde(rename = "not_configured")]
    NotConfigured,
}

/// Detailed commit information
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub date: String,
}
