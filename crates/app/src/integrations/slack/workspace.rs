//! Building the workspace manager a Slack message runs against.
//!
//! One function, because resolution and execution were each constructing their
//! own. `/slack/events` is `FleetOk`, so both run on stateless replicas with no
//! working copy: a manager built from `config.yml` there carries an empty
//! `Config` — no `databases:`, no `models:` — and the run answers from nothing.
//! Reading the compile boundary first is what makes the reply correct, and doing
//! it in one place is what makes the agent that was *chosen* and the run that
//! *executes* it agree on a revision.
//!
//! A Slack message carries no branch context, so the hint is always `None`: the
//! promoted default-branch revision.

use oxy::adapters::secrets::SecretsManager;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::adapters::workspace::resolve_workspace_path;
use oxy::config::WorkingCopy;
use oxy::service::secret_manager::SecretManagerService;
use oxy_shared::errors::OxyError;
use uuid::Uuid;

use crate::server::api::compiled_reader;

/// The manager for a Slack-initiated read or run, with its `Origin` recorded so
/// every boundary read downstream uses the revision resolved here rather than
/// deriving its own.
///
/// `caller` only labels the boundary-miss log.
pub async fn build_manager(
    workspace_id: Uuid,
    caller: &'static str,
) -> Result<WorkspaceManager<WorkingCopy>, OxyError> {
    let path = resolve_workspace_path(workspace_id).await?;
    let builder = WorkspaceBuilder::new(workspace_id);

    // Resolve which revision this read is pinned to; the manager loads the
    // config from it, or from the working copy when there is none.
    let revision_id = compiled_reader::resolve_request_revision(workspace_id, None).await;
    tracing::debug!(%workspace_id, ?revision_id, caller, "slack workspace manager");

    // The workspace's secrets in oxydb first, then the environment — the same
    // `SecretsManager` the request middleware and the queued-run builders hand
    // their managers. Without it `WorkspaceBuilder::build` falls back to an
    // environment-only manager, and on a replica a warehouse whose credentials
    // are workspace secrets (a ClickHouse `host_var`) resolves to nothing.
    let secrets_manager =
        SecretsManager::from_database_with_env_fallback(SecretManagerService::new(workspace_id))?;
    builder
        .with_working_copy(&path, revision_id, oxy::config::OnMissing::Empty)
        .await?
        .with_secrets_manager(secrets_manager)
        .build()
        .await
}
