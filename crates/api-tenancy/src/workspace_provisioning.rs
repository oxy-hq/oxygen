//! Creating a workspace: its working copy on node-local disk, and its row.
//!
//! The one door left is the GitHub import (`POST /orgs/{org_id}/onboarding/github`,
//! [`adopt`]): a workspace exists only once a repository backs it. New orgs get
//! none at creation, and the demo and blank workspaces are gone
//! (`internal-docs/factory-retirement.md` phase 6), so an org shows its setup
//! screen until its repository is connected.
//!
//! **An org has exactly one workspace.** It is the hidden key that binds the
//! org to its configuration repository and keys its revisions, Airhouse tenant
//! and previews; customers never pick between workspaces. [`register_workspace`]
//! refuses a second one ([`ProvisionError::OrgHasWorkspace`], `409`), and the
//! GitHub import connects its repository to the org's workspace instead of
//! creating another ([`adopt`]). Staging and sandboxes are app environments and
//! the data-release staging pointer, not workspaces
//! (`internal-docs/2026-11-10-org-vs-workspace-grain.md`).
//!
//! Lives in `oxy-app` rather than `oxy-project`: registering a workspace seeds
//! its health schedule through `agentic_pipeline`, and a platform crate may
//! never import an agentic one. Both sibling surface crates already depend on
//! `oxy-app`, and `oxy-app` depends on neither.
//!
//! Everything here writes node-local disk, so every route that reaches it is
//! IdeOnly.

use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use entity::prelude::Workspaces;
use entity::workspaces::{self, WorkspaceStatus};
use oxy::adapters::workspace::workspace_root_path;
use oxy::database::client::establish_connection;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, Set,
};
use uuid::Uuid;

pub mod adopt;
mod repository;

pub use repository::RepositoryOrigin;

#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    /// Another workspace in the same org already carries this display name.
    #[error("A workspace named '{0}' already exists. Please choose a different name.")]
    NameTaken(String),
    /// The org already has its one workspace. Connect a repository to it
    /// (`POST /orgs/{org_id}/onboarding/github`) rather than making another;
    /// an internal workspace gets an org of its own.
    #[error(
        "org_has_workspace: this organization already has its workspace. Connect a repository to it instead of creating another."
    )]
    OrgHasWorkspace,
    #[error("{0}")]
    Filesystem(String),
    #[error("{0}")]
    Database(String),
}

impl ProvisionError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::NameTaken(_) | Self::OrgHasWorkspace => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<ProvisionError> for (StatusCode, String) {
    fn from(e: ProvisionError) -> Self {
        (e.status_code(), e.to_string())
    }
}

fn query_failed(e: DbErr) -> ProvisionError {
    ProvisionError::Database(format!("Failed to query workspaces: {e}"))
}

/// `<state_dir>/workspaces/<workspace_id>`, created if needed. The workspace
/// UUID as the directory name guarantees uniqueness with no collision logic.
pub fn resolve_project_dir(workspace_id: Uuid) -> Result<PathBuf, ProvisionError> {
    let dir = workspace_root_path(workspace_id);
    std::fs::create_dir_all(&dir).map_err(|e| {
        ProvisionError::Filesystem(format!(
            "Failed to create workspace directory '{dir:?}': {e}"
        ))
    })?;
    Ok(dir)
}

/// A workspace display name unique within `org_id`: `base`, or the first free
/// "`base` 2" … "`base` 99".
///
/// Builds the whole candidate list and asks once with `WHERE name IN (…)`. `IN`
/// rather than `LIKE`, because `base` can be caller-controlled (a GitHub repo
/// name containing `_` or `%`). Names in other orgs don't count.
pub async fn unique_display_name<C: ConnectionTrait>(
    conn: &C,
    base: &str,
    org_id: Option<Uuid>,
) -> Result<String, ProvisionError> {
    let candidates: Vec<String> = std::iter::once(base.to_string())
        .chain((2u32..=99).map(|i| format!("{base} {i}")))
        .collect();

    let query = Workspaces::find().filter(workspaces::Column::Name.is_in(candidates.clone()));
    let taken: std::collections::HashSet<String> = in_org(query, org_id)
        .all(conn)
        .await
        .map_err(query_failed)?
        .into_iter()
        .map(|w| w.name)
        .collect();

    candidates
        .into_iter()
        .find(|candidate| !taken.contains(candidate))
        .ok_or_else(|| {
            ProvisionError::Database(format!("Could not find a unique name for '{base}'"))
        })
}

fn in_org(query: sea_orm::Select<Workspaces>, org_id: Option<Uuid>) -> sea_orm::Select<Workspaces> {
    match org_id {
        Some(id) => query.filter(workspaces::Column::OrgId.eq(id)),
        None => query.filter(workspaces::Column::OrgId.is_null()),
    }
}

/// The org's workspace, if it has one. Oldest first, so an org that predates
/// the one-per-org rule answers with the workspace it was created with.
pub async fn org_workspace<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
) -> Result<Option<workspaces::Model>, ProvisionError> {
    use sea_orm::QueryOrder;
    Workspaces::find()
        .filter(workspaces::Column::OrgId.eq(org_id))
        .order_by_asc(workspaces::Column::CreatedAt)
        .one(conn)
        .await
        .map_err(query_failed)
}

/// The columns a caller chooses for a new workspace row.
pub struct NewWorkspaceRow<'a> {
    pub name: &'a str,
    pub created_by: Option<Uuid>,
    pub org_id: Option<Uuid>,
    pub status: WorkspaceStatus,
    /// `None` for a workspace with no git remote: its four repository columns
    /// (`git_namespace_id`, `git_remote_url`, `default_branch`, `repo_subdir`)
    /// all stay NULL.
    pub repository: Option<RepositoryOrigin>,
}

/// What [`register_workspace`] found or wrote.
pub struct Registered {
    pub id: Uuid,
    /// `false` when the directory was already registered and nothing was
    /// written — so there is no new row to seed a health schedule for, and
    /// re-seeding one would reset a schedule its compile already enabled.
    pub created: bool,
}

/// Insert the workspace row on `conn`. Idempotent on path: an already
/// registered directory returns its existing id. Does NOT activate the
/// workspace, and does not seed its health schedule — that needs a committed
/// row, so it is the caller's job ([`seed_health_schedule`]).
pub async fn register_workspace<C: ConnectionTrait>(
    conn: &C,
    project_dir: &Path,
    workspace_id: Uuid,
    row: NewWorkspaceRow<'_>,
) -> Result<Registered, ProvisionError> {
    let path = project_dir.to_string_lossy().to_string();
    let existing = Workspaces::find()
        .filter(workspaces::Column::Path.eq(path.clone()))
        .one(conn)
        .await
        .map_err(query_failed)?;
    if let Some(existing) = existing {
        return Ok(Registered {
            id: existing.id,
            created: false,
        });
    }

    // One workspace per org. Checked before the name, so a second create is
    // refused as such rather than as a name clash.
    if let Some(org_id) = row.org_id
        && org_workspace(conn, org_id).await?.is_some()
    {
        return Err(ProvisionError::OrgHasWorkspace);
    }

    // Display names are unique per org; other orgs have their own namespace.
    let clash = in_org(
        Workspaces::find().filter(workspaces::Column::Name.eq(row.name)),
        row.org_id,
    )
    .one(conn)
    .await
    .map_err(query_failed)?;
    if clash.is_some() {
        return Err(ProvisionError::NameTaken(row.name.to_string()));
    }

    let now = chrono::Utc::now();
    let repository = row.repository.map(RepositoryOrigin::into_columns);
    let (git_namespace_id, git_remote_url, default_branch, repo_subdir) =
        repository.unwrap_or_default();
    workspaces::ActiveModel {
        id: Set(workspace_id),
        name: Set(row.name.to_string()),
        git_namespace_id: Set(git_namespace_id),
        git_remote_url: Set(git_remote_url),
        created_at: Set(now.into()),
        updated_at: Set(now.into()),
        path: Set(Some(path.clone())),
        last_opened_at: Set(None),
        created_by: Set(row.created_by),
        org_id: Set(row.org_id),
        status: Set(row.status),
        error: Set(None),
        monthly_vlm_budget_micros: Set(None),
        current_revision_id: Set(None),
        default_branch: Set(default_branch),
        repo_subdir: Set(repo_subdir),
        default_branch_attempted_at: Set(None),
    }
    .insert(conn)
    .await
    .map_err(|e| {
        ProvisionError::Database(format!(
            "Failed to register workspace '{}' in DB: {e}",
            row.name
        ))
    })?;
    tracing::info!("Registered workspace '{}' at '{}'", row.name, path);
    Ok(Registered {
        id: workspace_id,
        created: true,
    })
}

/// How long workspace creation waits for its Airhouse tenant before moving on.
/// Short, because it sits inside the request that creates the workspace and
/// its result is ignored. Only the wait is cut off: the provision itself runs
/// to completion as its own task (see `provision_airhouse_tenant`). If the
/// process dies first, everything that needs the tenant (an upload, the demo)
/// provisions it itself, so it is not a queued task either.
const AIRHOUSE_PROVISION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Give a just-created workspace its Airhouse tenant, where its data lives in a
/// cloud deployment. Best-effort: a failure is logged and never fails the
/// caller, and whatever needs the tenant first (an upload, the demo) provisions
/// it then (`airhouse::ensure_tenant`). A local `OXY_ROLE=all` process
/// provisions too when it has an Airhouse (`oxy start` runs one), and says
/// nothing when it has none.
pub async fn provision_airhouse_tenant(workspace_id: Uuid) {
    use oxy_app_core::process_role::{Role, current_process_role};
    if current_process_role() == Role::All
        && airhouse::AirhouseConfig::cached().as_runtime().is_none()
    {
        return;
    }
    let db = match establish_connection().await {
        Ok(db) => db,
        Err(e) => {
            tracing::warn!(%workspace_id, error = %e, "Airhouse tenant not provisioned at workspace creation: no database connection; it is provisioned on first need");
            return;
        }
    };
    // The provision runs as its own task and only the request's wait is timed
    // out. Dropping the provision future partway could cut it after Airhouse
    // created the tenant and before the local `airhouse_tenants` row is saved,
    // leaving a remote tenant under this workspace's name that no later
    // `ensure_tenant` can adopt (`TenantNameTaken`). A spawned task always runs
    // the create and the insert together.
    let provision = tokio::spawn(async move { airhouse::ensure_tenant(&db, workspace_id).await });
    match tokio::time::timeout(AIRHOUSE_PROVISION_TIMEOUT, provision).await {
        Ok(Err(join_error)) => {
            tracing::warn!(%workspace_id, error = %join_error, "Airhouse tenant provisioning at workspace creation did not finish; it is provisioned on first need");
        }
        Ok(Ok(Ok(outcome))) => {
            tracing::info!(%workspace_id, ?outcome, "Airhouse tenant ready for new workspace");
        }
        Ok(Ok(Err(airhouse::EnsureTenantError::NotConfigured))) => {
            tracing::warn!(%workspace_id, "Airhouse tenant not provisioned at workspace creation: Airhouse is not configured on this deployment");
        }
        Ok(Ok(Err(e))) => {
            tracing::warn!(%workspace_id, error = %e, "Airhouse tenant not provisioned at workspace creation; it is provisioned on first need");
        }
        Err(_) => {
            tracing::warn!(%workspace_id, timeout = ?AIRHOUSE_PROVISION_TIMEOUT, "Airhouse tenant provisioning is still running after workspace creation; the request stopped waiting, the provision finishes in the background");
        }
    }
}

/// Seed the workspace's health schedule row, **disabled**. Best-effort: never
/// fails the caller.
///
/// Health checks are opt-in and a fresh workspace has compiled no `config.yml`,
/// so nothing says it wants them; the compile worker enables the row from
/// `health_check` on the first promoted compile. Seeding it anyway keeps that
/// reconcile a plain update. Both values come from the resolvers the compile
/// worker uses, so there is one definition of "unconfigured" — and the cadence
/// matches, so that compile leaves `next_run_at` alone.
pub async fn seed_health_schedule(workspace_id: Uuid) {
    let db = match establish_connection().await {
        Ok(db) => db,
        Err(e) => {
            tracing::warn!(target: "health_eval", error = %e, %workspace_id, "failed to seed health schedule for new workspace");
            return;
        }
    };
    if let Err(e) = agentic_pipeline::scheduler::reconcile_health_schedule(
        &db,
        workspace_id,
        oxy::config::health_check::resolve_interval(None),
        oxy::config::health_check::resolve_enabled(None),
    )
    .await
    {
        tracing::warn!(
            target: "health_eval",
            error = %e,
            %workspace_id,
            "failed to seed health schedule for new workspace"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Onboarding surfaces this mapping directly: a clash is the one thing a
    /// user fixes.
    #[test]
    fn only_a_name_clash_is_the_callers_to_fix() {
        assert_eq!(
            ProvisionError::NameTaken("x".into()).status_code(),
            StatusCode::CONFLICT
        );
        for e in [
            ProvisionError::Filesystem("x".into()),
            ProvisionError::Database("x".into()),
        ] {
            assert_eq!(e.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
}
