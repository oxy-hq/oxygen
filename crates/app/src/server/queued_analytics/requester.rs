//! Who a chat run runs as when something other than its request drives it.
//!
//! The request drives a chat on the context the workspace middleware built:
//! the caller as its subject, at the workspace role `authorize_workspace`
//! resolved (an API token's ceiling already applied). A driver in another
//! process has none of that. Its own context carries **no subject**, and
//! `airhouse_managed` mints a *system Admin* for a subject-less context where
//! a Member would have minted a *Reader* (`project_ctx::mint_airhouse_managed_creds`).
//!
//! So the request records who it was for on the run's own insert
//! (`agentic_pipeline::platform::Requester`, under `metadata.requester`), and
//! a driver builds the context from that record: the tick's workspace context
//! ([`crate::server::router::recovery::build_requester_base_ctx`]) with the
//! recorded user as subject and the recorded role. The role is the one the
//! request resolved, not one re-read now: that is what a handler-driven run
//! keeps for its whole life, and what a queued one must not exceed.
//!
//! Recovery reads the same record (`agent_ask::caller::CallerRunResolver`), so
//! a chat run its driver died in is resumed as its requester too — where it
//! used to be resumed on the tick's subject-less platform.

use agentic_pipeline::platform::Requester;
use entity::prelude::Workspaces;
use entity::workspace_members::WorkspaceRole;
use sea_orm::{DatabaseConnection, EntityTrait};
use thiserror::Error;
use uuid::Uuid;

use crate::agentic_wiring::OxyProjectContext;

/// Why a recorded requester's context could not be produced. Either way the
/// run is not driven as anyone else.
#[derive(Debug, Error)]
pub enum RequesterContextError {
    #[error("{0}")]
    Unreadable(String),
    #[error("workspace lookup failed: {0}")]
    Lookup(String),
    #[error("the run's workspace no longer exists")]
    WorkspaceGone,
    #[error("the run's workspace has no path to build a context at")]
    NoPath,
    #[error("the workspace is not compiled yet on this node; a compile was queued")]
    NotCompiled,
    #[error("the run's workspace context could not be built: its config does not load here")]
    Build,
    #[error("the run's recorded role `{0}` is not a workspace role")]
    Role(String),
}

impl RequesterContextError {
    /// Might the same build succeed a minute from now? A failed read, and a
    /// workspace with nothing compiled yet (the compile was just queued).
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Lookup(_) | Self::NotCompiled)
    }
}

/// The context `requester` had in `workspace_id`: the driver's workspace
/// context with the requester as subject, at the recorded role.
pub async fn requester_context(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    requester: &Requester,
) -> Result<OxyProjectContext, RequesterContextError> {
    let role = requester
        .role
        .as_deref()
        .map(|r| {
            r.parse::<WorkspaceRole>()
                .map_err(|_| RequesterContextError::Role(r.to_string()))
        })
        .transpose()?;
    let workspace = Workspaces::find_by_id(workspace_id)
        .one(db)
        .await
        .map_err(|e| RequesterContextError::Lookup(e.to_string()))?
        .ok_or(RequesterContextError::WorkspaceGone)?;
    let path = workspace.path.ok_or(RequesterContextError::NoPath)?;
    let base = crate::server::router::recovery::build_requester_base_ctx(workspace_id, &path, db)
        .await
        .map_err(|e| match e {
            crate::server::router::recovery::RequesterBaseError::NotCompiled => {
                RequesterContextError::NotCompiled
            }
            crate::server::router::recovery::RequesterBaseError::Build => {
                RequesterContextError::Build
            }
        })?;
    let context = base.with_subject(requester.user_id);
    Ok(match role {
        Some(role) => context.with_role(role),
        None => context,
    })
}

/// The context recovery drives `root` on when it records a requester; `None`
/// when it records none, or its workspace row is gone — local mode's single
/// workspace has no row, and a deleted workspace's runs are retired by the
/// tick that finds them, so in both the tick's own platform is the right one.
pub async fn requester_platform(
    db: &DatabaseConnection,
    root: &agentic_runtime::entity::run::Model,
) -> Result<Option<OxyProjectContext>, RequesterContextError> {
    let Some(requester) =
        Requester::recorded(root.metadata.as_ref()).map_err(RequesterContextError::Unreadable)?
    else {
        return Ok(None);
    };
    match requester_context(db, root.workspace_id, &requester).await {
        Ok(context) => Ok(Some(context)),
        Err(RequesterContextError::WorkspaceGone) => Ok(None),
        Err(e) => Err(e),
    }
}
