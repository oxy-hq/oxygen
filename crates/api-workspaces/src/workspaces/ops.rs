use axum::response::Json as ResponseJson;
use reqwest::StatusCode;
use tracing::error;
use uuid::Uuid;

use oxy::adapters::workspace::effective_workspace_path;

use super::dto::*;

/// Returns the value to put in `WorkspaceDetailsResponse.storage_key`.
/// `None` (cloud) yields the workspace UUID; `Some(path)` (local) yields
/// `local:{hash}` over the canonical, absolute, OS-encoded path bytes so
/// two `--local` sessions in different directories on the same dev port
/// don't collide and a dev alternating `--local` and cloud on the same
/// origin keeps separate keyspaces. Falls back through raw / current_dir
/// joins when `canonicalize` fails so the helper is total.
pub fn compute_workspace_storage_key(
    workspace_id: Uuid,
    local_path: Option<&std::path::Path>,
) -> String {
    match local_path {
        Some(path) => {
            use sha2::{Digest, Sha256};
            let normalized = std::fs::canonicalize(path).unwrap_or_else(|_| {
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    std::env::current_dir()
                        .map(|cwd| cwd.join(path))
                        .unwrap_or_else(|_| path.to_path_buf())
                }
            });
            let digest = Sha256::digest(normalized.as_os_str().as_encoded_bytes());
            format!("local:{}", &hex::encode(digest)[..16])
        }
        None => workspace_id.to_string(),
    }
}

pub(super) async fn workspace_root(
    ws: &entity::workspaces::Model,
) -> Result<std::path::PathBuf, StatusCode> {
    effective_workspace_path(ws, None).await.map_err(|e| {
        error!("Failed to resolve workspace path for {}: {}", ws.id, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// Build a `WorkspaceDetailsResponse` in one of the two "no git visible"
/// shapes. Shared between the missing-directory branch and the
/// local-mode short-circuit so a future field addition only lands in
/// one place.
pub(super) fn no_git_response(
    workspace_id: Uuid,
    name: &str,
    now: String,
    workspace_error: Option<String>,
    requires_local_setup: bool,
    current_user_role: String,
    storage_key: String,
) -> ResponseJson<WorkspaceDetailsResponse> {
    let mode = GitMode::None;
    ResponseJson(WorkspaceDetailsResponse {
        id: workspace_id,
        name: name.to_string(),
        workspace_id: Uuid::nil(),
        created_at: now.clone(),
        updated_at: now,
        active_branch: None,
        detached_head: None,
        workspace_error,
        git_mode: mode,
        capabilities: mode.into(),
        default_branch: "main".to_string(),
        protected_branches: vec!["main".to_string()],
        requires_local_setup,
        current_user_role,
        storage_key,
    })
}

/// Response builder for the local-mode "no config.yml yet" case. Exposed
/// publicly so integration tests can assert the shape without spinning up
/// the full router + DB.
pub fn build_workspace_details_response_for_uninitialized_local(
    workspace_id: Uuid,
    name: &str,
    current_user_role: String,
    storage_key: String,
) -> ResponseJson<WorkspaceDetailsResponse> {
    let now = chrono::Utc::now().to_string();
    no_git_response(
        workspace_id,
        name,
        now,
        None,
        true,
        current_user_role,
        storage_key,
    )
}

/// The workspace card's agent count: the agents each workspace's promoted
/// revision compiled, keyed by workspace id. A workspace with no promoted
/// revision is absent (`agent_count: None`, "did not look"), and so is every
/// workspace when the count could not be read — a database blip must not
/// render as "0 agents". The same on every role: no working copy is walked.
pub(super) async fn compiled_agent_counts(
    workspaces: &[entity::workspaces::Model],
) -> std::collections::HashMap<Uuid, usize> {
    let revisions: Vec<Uuid> = workspaces
        .iter()
        .filter_map(|w| w.current_revision_id)
        .collect();
    let by_revision = match oxy::config::compiled_agent_counts(&revisions).await {
        Ok(counts) => counts,
        Err(e) => {
            tracing::warn!(error = %e, retryable = e.retryable(), "workspace agent counts unavailable");
            return std::collections::HashMap::new();
        }
    };
    workspaces
        .iter()
        .filter_map(|w| {
            let revision = w.current_revision_id?;
            Some((w.id, by_revision.get(&revision).copied().unwrap_or(0)))
        })
        .collect()
}

/// Count files whose name ends with `.<suffix>.yml` under `dir` (recursive, skips hidden dirs).
pub(super) fn count_yml_suffix(dir: &std::path::Path, suffix: &str) -> usize {
    let pattern = format!(".{suffix}.yml");
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                let hidden = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with('.'))
                    .unwrap_or(false);
                if !hidden {
                    count += count_yml_suffix(&path, suffix);
                }
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(&pattern))
                .unwrap_or(false)
            {
                count += 1;
            }
        }
    }
    count
}

/// The workspace card's automation count: the two spellings
/// `ConfigManager::list_workflows` and the compile walker recognise, and no
/// other. `.workflow.yml` is not a kind (product-context.md), so counting it
/// here made the card disagree with the list it summarises.
pub(super) fn count_automation_files(dir: &std::path::Path) -> usize {
    count_yml_suffix(dir, "automation") + count_yml_suffix(dir, "procedure")
}

#[cfg(test)]
mod tests {
    use super::count_automation_files;

    #[test]
    fn the_summary_counts_the_two_recognised_automation_spellings_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("workflows")).unwrap();
        std::fs::write(root.join("nightly.automation.yml"), "tasks: []\n").unwrap();
        std::fs::write(root.join("workflows/legacy.procedure.yml"), "tasks: []\n").unwrap();
        std::fs::write(root.join("workflows/ancient.workflow.yml"), "tasks: []\n").unwrap();

        assert_eq!(
            count_automation_files(root),
            2,
            "`.workflow.yml` is not a recognised kind, so the card must not count it"
        );
    }
}
