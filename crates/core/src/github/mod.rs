pub mod app_auth;
pub mod auth;
pub mod branch_head;
pub mod client;
pub mod tarball;
pub mod types;

pub use app_auth::*;
pub use auth::*;
pub use branch_head::BranchHeadError;
pub use client::*;
pub use tarball::{TarballError, transport_error};
pub use types::*;

use oxy_git::cli::CliGitClient;
use oxy_shared::errors::OxyError;

/// Construct the default git client.
///
/// All call sites that previously used `GitOperations` should migrate to
/// this factory, then to an injected `GitClient` from app state.
pub fn default_git_client() -> CliGitClient {
    CliGitClient::new()
}

/// Resolve a usable GitHub token for a namespace row.
///
/// PAT namespaces return the stored OAuth token directly; GitHub App
/// namespaces mint a short-lived installation token.
pub async fn github_token_for_namespace(
    ns: &entity::git_namespaces::Model,
) -> Result<String, OxyError> {
    if ns.slug == "pat" {
        if ns.oauth_token.is_empty() {
            return Err(OxyError::RuntimeError(
                "PAT namespace has empty token".to_string(),
            ));
        }
        return Ok(ns.oauth_token.clone());
    }
    let app_auth = GitHubAppAuth::from_env()?;
    app_auth
        .get_installation_token(&ns.installation_id.to_string())
        .await
}

/// Resolve an optional GitHub token for a workspace, with no requesting user.
///
/// Equivalent to [`github_token_for_workspace_as_user`] with `user_id = None`:
/// only the workspace's own `git_namespace_id` link is consulted. Use the
/// `_as_user` form from any request handler — it can additionally reach the
/// caller's personal access token, which is the difference between a fetch
/// working and not for a workspace that was never linked at import time.
pub async fn github_token_for_workspace(
    workspace: &entity::workspaces::Model,
) -> Result<Option<String>, OxyError> {
    github_token_for_workspace_as_user(workspace, None).await
}

/// Resolve an optional GitHub token for a workspace on behalf of `user_id`.
///
/// Resolution order:
///
/// 1. The workspace's `git_namespace_id` link — set by the GitHub import flow,
///    and the only source that works for unattended callers (the background
///    fetch sweep, workers).
/// 2. Failing that, the requesting user's **own** PAT namespace in the
///    workspace's org. A PAT the user registered for this org grants exactly
///    the access they already have, so spending it on their own fetch adds no
///    reach — and it is what makes "the org has a PAT configured" behave the
///    way the UI implies for a workspace that predates the link.
///
/// Deliberately **not** in that order: any other member's PAT. PAT namespaces
/// are per-user (`(created_by, slug = "pat")`, filtered to their creator in the
/// namespace list), so borrowing one would run a fetch with someone else's
/// repo access. For the same reason a PAT found via (2) is used for this
/// request only and never written back to `git_namespace_id` — that column is
/// workspace-wide and would silently hand a personal token to every member.
///
/// `Ok(None)` means no connection is reachable. What a caller should do with
/// that differs by operation, and the split matters:
///
/// * **Writes** (push, force-push) can never succeed unauthenticated — take
///   [`require_github_token_for_workspace`] and fail before touching the
///   network.
/// * **Reads** (fetch, pull) still work unauthenticated against a *public*
///   repo, and that is not a host-credential fallback: `oxy-git` resets
///   `credential.helper` on every invocation, so a token-less read borrows
///   nothing from the machine. Attempt it, and on failure wrap the error with
///   [`unlinked_remote_failure`] — the git-level message for a private repo
///   names neither the missing connection nor the cause.
pub async fn github_token_for_workspace_as_user(
    workspace: &entity::workspaces::Model,
    user_id: Option<uuid::Uuid>,
) -> Result<Option<String>, OxyError> {
    use sea_orm::EntityTrait;

    if let Some(namespace_id) = workspace.git_namespace_id {
        let db = crate::database::client::establish_connection().await?;
        let ns = entity::git_namespaces::Entity::find_by_id(namespace_id)
            .one(&db)
            .await
            .map_err(|e| OxyError::RuntimeError(format!("Failed to find git namespace: {e}")))?
            .ok_or_else(|| OxyError::RuntimeError("Git namespace not found".to_string()))?;
        return github_token_for_namespace(&ns).await.map(Some);
    }

    // Before any DB work: the unattended sweep reaches this for every unlinked
    // workspace on every tick, and that path needs no connection at all.
    let (Some(org_id), Some(user_id)) = (workspace.org_id, user_id) else {
        return Ok(None);
    };

    let db = crate::database::client::establish_connection().await?;
    match personal_pat_namespace(&db, org_id, user_id).await? {
        Some(ns) => {
            tracing::debug!(
                workspace_id = %workspace.id,
                namespace_id = %ns.id,
                "workspace has no linked git namespace; using the requesting user's PAT"
            );
            github_token_for_namespace(&ns).await.map(Some)
        }
        None => Ok(None),
    }
}

/// The PAT namespace `user_id` registered for `org_id`, if any.
async fn personal_pat_namespace(
    db: &sea_orm::DatabaseConnection,
    org_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<Option<entity::git_namespaces::Model>, OxyError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    entity::git_namespaces::Entity::find()
        .filter(entity::git_namespaces::Column::OrgId.eq(org_id))
        .filter(entity::git_namespaces::Column::CreatedBy.eq(user_id))
        .filter(entity::git_namespaces::Column::Slug.eq("pat"))
        .one(db)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Failed to find PAT namespace: {e}")))
}

/// Resolve a token for a workspace, or explain that there isn't one.
///
/// Use this for operations that **cannot** succeed unauthenticated — push and
/// force-push. Reads should stay on [`github_token_for_workspace_as_user`] so a
/// public repo still fetches; see that function's `Ok(None)` note.
pub async fn require_github_token_for_workspace(
    workspace: &entity::workspaces::Model,
    user_id: Option<uuid::Uuid>,
) -> Result<String, OxyError> {
    github_token_for_workspace_as_user(workspace, user_id)
        .await?
        .ok_or_else(|| no_git_connection_error(workspace))
}

/// How to get a connection, appended to both errors below.
fn connect_github_advice(workspace: &entity::workspaces::Model) -> String {
    format!(
        "Connect GitHub (a personal access token or the GitHub App) for this organization in \
         Settings, then re-import the repository or link the connection to workspace '{}'. Oxy \
         does not use the host machine's git credentials.",
        workspace.name
    )
}

/// No connection, for an operation that had no chance without one.
pub fn no_git_connection_error(workspace: &entity::workspaces::Model) -> OxyError {
    OxyError::RuntimeError(format!(
        "No GitHub connection is linked to workspace '{}', and this operation cannot run \
         unauthenticated. {}",
        workspace.name,
        connect_github_advice(workspace)
    ))
}

/// A remote read that was attempted without a token and failed.
///
/// Token-less reads are allowed on purpose — a public repo needs no credentials,
/// and `oxy-git` resets `credential.helper` so nothing is borrowed from the host
/// either way. But when such a read fails, git's own message (`could not read
/// Username for 'https://github.com': terminal prompts disabled`) describes the
/// symptom and not the cause, so carry both: what git said, and the fact that
/// there was no connection to try.
pub fn unlinked_remote_failure(
    workspace: &entity::workspaces::Model,
    source: OxyError,
) -> OxyError {
    OxyError::RuntimeError(format!(
        "{source}\n\nThis ran unauthenticated because no GitHub connection is linked to \
         workspace '{}', which only works for a public repository. {}",
        workspace.name,
        connect_github_advice(workspace)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn namespace(slug: &str, installation_id: i64, token: &str) -> entity::git_namespaces::Model {
        entity::git_namespaces::Model {
            id: uuid::Uuid::new_v4(),
            installation_id,
            name: "acme".to_string(),
            oauth_token: token.to_string(),
            owner_type: "Organization".to_string(),
            provider: "github".to_string(),
            slug: slug.to_string(),
            created_by: uuid::Uuid::new_v4(),
            org_id: Some(uuid::Uuid::new_v4()),
        }
    }

    /// A PAT namespace hands back the stored token verbatim.
    #[tokio::test]
    async fn pat_namespace_returns_its_stored_token() {
        let ns = namespace("pat", 0, "ghp_stored");
        assert_eq!(github_token_for_namespace(&ns).await.unwrap(), "ghp_stored");
    }

    /// An empty PAT is a misconfiguration, not a token-less fetch.
    #[tokio::test]
    async fn empty_pat_is_an_error() {
        let ns = namespace("pat", 0, "");
        assert!(github_token_for_namespace(&ns).await.is_err());
    }

    /// GitHub **App** namespaces must keep minting a short-lived installation
    /// token through `GitHubAppAuth` — never returning `oauth_token`, which for
    /// an installation row is blanked on purpose
    /// (`m20260415_000002_null_installation_namespace_tokens`) and would be a
    /// useless credential even when populated.
    ///
    /// Asserted as "never the stored string" rather than a fixed error so the
    /// test holds whether or not `GITHUB_APP_ID` happens to be set in the
    /// environment: with no App configured `from_env` errors, with one it makes
    /// a real call for a bogus installation and errors too. Only a regression
    /// that routed App rows down the PAT branch returns the sentinel.
    #[tokio::test]
    async fn app_namespace_never_returns_the_stored_oauth_token() {
        let ns = namespace("acme-org", 4242, "must-never-be-used-as-a-token");
        if let Ok(token) = github_token_for_namespace(&ns).await {
            assert_ne!(
                token, "must-never-be-used-as-a-token",
                "installation namespace was resolved through the PAT branch"
            );
        }
    }
}
