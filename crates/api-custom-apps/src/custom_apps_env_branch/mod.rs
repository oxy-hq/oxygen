//! A custom-app environment's own OLTP branch: the task that cuts, resets and
//! migrates it on the worker fleet
//! (`internal-docs/2026-11-02-custom-app-branches-design.md`).
//!
//! An environment whose `planes.oltp` is `branch` or `empty` runs `ctx.oltp`
//! on a Neon branch of its own, not in a schema of the org's shared staging
//! branch. The provisioner (`oxy_oltp::OltpProvisioner`) owns the provider
//! calls and the row's status; this module owns the rest of a cut — applying
//! the environment's build's OLTP migrations and calling the branch `ready` —
//! and the queue entry that runs it ([`task`]).
//!
//! **A branch is `ready` only with its build's tables.** A cut or reset leaves
//! the row `migrating`; the task applies the files and only then says `ready`,
//! and a failed apply says `failed`, so a branch that misses its tables never
//! serves.
//!
//! **Nothing here runs on a request path.** A cut copies a database, which
//! takes seconds to minutes.

pub mod diff;
pub mod home;
pub(crate) mod planes;
pub(crate) mod publish;
pub mod queue;
pub mod task;

use oxy_custom_apps::planes::{OltpMode, PlaneSpec};
use oxy_oltp::OltpProvisioner;
use oxy_oltp::entity::env_branches::EnvBranchMode;
use sea_orm::DatabaseConnection;

/// The provisioner for the process's `OXY_OLTP_*` environment, as `oxy oltp`
/// and the admin console build it. `Err` says why there is none (the
/// integration is off, or misconfigured).
pub async fn provisioner(db: &DatabaseConnection) -> Result<OltpProvisioner, String> {
    oxy_oltp::provisioner::from_env(db.clone())
        .await
        .map_err(|e| format!("the OLTP provisioner is not available: {e}"))
}

/// The branch `row`'s environment asks for: its `planes.oltp`, when that is
/// `branch` or `empty`. A `planes` column that does not parse asks for none —
/// the same environment is served from its schema, as before.
pub fn wants_branch(row: &entity::app_environments::Model) -> Option<EnvBranchMode> {
    wants_branch_in(&row.planes)
}

/// [`wants_branch`] for a `planes` column value.
pub fn wants_branch_in(planes: &serde_json::Value) -> Option<EnvBranchMode> {
    match PlaneSpec::parse(planes).ok()?.oltp? {
        OltpMode::Branch => Some(EnvBranchMode::Branch),
        OltpMode::Empty => Some(EnvBranchMode::Empty),
        OltpMode::Schema => None,
    }
}
