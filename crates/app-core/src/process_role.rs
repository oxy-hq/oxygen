//! This process's fleet role (`OXY_ROLE`, parsed by `crate::config`): what the
//! process owns, read by everything below `oxy-app` that has to behave
//! differently on a stateless serve pod than on the node that holds the
//! workspace files.
//!
//! The role manifest (`oxy_app::server::role_manifest`) re-exports all of this;
//! it owns route classification, this owns the process-wide fact.

use std::sync::OnceLock;

pub use oxy_shared::fleet_role::Role;

static PROCESS_ROLE: OnceLock<Role> = OnceLock::new();

pub fn init_process_role_from_env() -> Role {
    init_process_role_from_env_with_default(Role::All)
}

/// The same read, for a command that already knows what it is.
///
/// `oxy worker` running at all is stronger evidence of the role than an
/// environment variable a deployment chart may never set, and defaulting it to
/// `All` there is not neutral — `All` is the value that claims the workspace
/// filesystem. An explicit `OXY_ROLE` still wins, so an operator can still run
/// the worker binary as an all-in-one.
pub fn init_process_role_from_env_with_default(default: Role) -> Role {
    let role = crate::config::BootConfig::current().role.resolve(default);
    let _ = PROCESS_ROLE.set(role);
    oxy::workspace_fs_probe::set_process_owns_workspace_files(role_owns_workspace_files(role));
    oxy::file_duckdb_limits::set_limits_apply(file_duckdb_limits_apply(role));
    tracing::info!(role = role.as_str(), "role manifest initialised");
    role
}

pub fn current_process_role() -> Role {
    *PROCESS_ROLE.get().unwrap_or(&Role::All)
}

pub fn role_owns_workspace_files(role: Role) -> bool {
    matches!(role, Role::Ide | Role::All)
}

/// Whether a file DuckDB (`dataset:` / `path:`) runs under the cloud limits on
/// `role`: read-only, and read only from the compiler's S3 copy. Every role but
/// the single-process one (`oxy::file_duckdb_limits`).
pub fn file_duckdb_limits_apply(role: Role) -> bool {
    role != Role::All
}
