//! Which pod a process is, and which pods a route may run on.
//!
//! Two pure enums, here rather than in `oxy-app` so that crates BELOW it can
//! name them. `agentic-http` is the reason: it owns ~40 routes under
//! `/analytics` and `/agentic-*`, cannot import `oxy-app` (the layering forbids
//! it), and therefore could not state whether its own routes need the ide. That
//! left `oxy-app` declaring them from the outside, as four wildcard entries in a
//! hand-written table, with the per-route carve-outs that a wildcard forces.
//!
//! The process-wide fact lives here too: the one `OnceLock` holding this
//! process's role, its getter and the pure predicates over a role, plus the
//! route-pattern matcher ([`pattern_matches`]). They moved down from
//! `oxy-app-core` (`process_role`, `pods::role_manifest`) so that a crate which
//! only asks "which pod am I" or "does this path match this pattern" does not
//! need `oxy-app-core`, and `oxy-app-core` re-exports them at those paths.
//!
//! The env plumbing stays in `oxy-app-core`: reading `OXY_ROLE`
//! (`process_role::init_process_role_from_env*`, which writes the cell through
//! [`set_process_role`]), and the route declarations and `classify`.

use std::sync::OnceLock;

/// Process-role for this server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Singleton owning the workspace FS + git working tree.
    Ide,
    /// Stateless fleet replica.
    Serve,
    /// Queue drainer. Today has only `/healthz` over HTTP.
    Worker,
    /// Single-process all-in-one. Accepts every route.
    All,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Ide => "ide",
            Role::Serve => "serve",
            Role::Worker => "worker",
            Role::All => "all",
        }
    }
}

/// Which pods may serve a route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteRole {
    IdeOnly,
    FleetOk,
    WorkerOnly,
}

impl RouteRole {
    pub fn as_str(self) -> &'static str {
        match self {
            RouteRole::IdeOnly => "ide-only",
            RouteRole::FleetOk => "fleet-ok",
            RouteRole::WorkerOnly => "worker-only",
        }
    }

    /// Whether a process of `process_role` may serve a route of `self`.
    /// Notably FleetOk is accepted by Worker too: kubelet health probes
    /// must succeed on every role.
    pub fn accepted_by(self, process_role: Role) -> bool {
        match (self, process_role) {
            (_, Role::All) => true,
            (RouteRole::IdeOnly, Role::Ide) => true,
            (RouteRole::FleetOk, Role::Ide | Role::Serve | Role::Worker) => true,
            (RouteRole::WorkerOnly, Role::Worker) => true,
            _ => false,
        }
    }
}

/// One route's declared role, as a crate states it about its OWN routes.
///
/// `path` is relative to wherever the router is nested — the mounting crate
/// prepends its prefix. That is what lets a sub-router declare per-route
/// instead of being covered by a wildcard from outside.
#[derive(Clone, Copy, Debug)]
pub struct RouteRoleDecl {
    /// HTTP method, or `"*"` for any.
    pub method: &'static str,
    /// Path relative to the mount point, e.g. `"/runs/{run_id}"`.
    pub path: &'static str,
    pub role: RouteRole,
}

/// This process's role. Exactly one cell in the process: written once, by
/// `oxy_app_core::process_role::init_process_role_from_env*`, through
/// [`set_process_role`].
static PROCESS_ROLE: OnceLock<Role> = OnceLock::new();

/// Record this process's role. The first write wins and a later one is
/// refused (returned as `Err(role)`), as `OnceLock::set` does: a process has
/// exactly one role.
pub fn set_process_role(role: Role) -> Result<(), Role> {
    PROCESS_ROLE.set(role)
}

/// This process's role, or `All` when none was recorded (a test, a CLI
/// command that never initialised it).
pub fn current_process_role() -> Role {
    *PROCESS_ROLE.get().unwrap_or(&Role::All)
}

pub fn role_owns_workspace_files(role: Role) -> bool {
    matches!(role, Role::Ide | Role::All)
}

pub fn process_can_compile() -> bool {
    role_owns_workspace_files(current_process_role())
}

pub fn role_runs_inprocess_workers(role: Role) -> bool {
    !matches!(role, Role::Serve)
}

/// Whether this process may write the workspace filesystem: only one that owns
/// the workspace files (`Ide`, or `All` in single-process mode). This was
/// `role != Serve`, which let a `worker` through, so a write a worker reached
/// went to a checkout that process does not have.
pub fn process_is_fs_writable() -> bool {
    role_owns_workspace_files(current_process_role())
}

/// Whether `path` matches the route `pattern`, segment by segment: a literal
/// segment matches itself, `{param}` matches any one non-empty segment, and a
/// trailing `{*rest}` matches one or more remaining segments. The route
/// classifier (`oxy_app_core::pods::role_manifest`), the previews read-only
/// table and the factory-access table all match with this one function.
pub fn pattern_matches(pattern: &str, path: &str) -> bool {
    let mut pat = pattern.trim_start_matches('/').split('/');
    let mut req = path.trim_start_matches('/').split('/');
    loop {
        match (pat.next(), req.next()) {
            (None, None) => return true,
            (Some(_), None) | (None, Some(_)) => return false,
            (Some(p), Some(r)) => {
                if is_rest_wildcard(p) {
                    return !r.is_empty();
                }
                if is_param(p) {
                    if r.is_empty() {
                        return false;
                    }
                    continue;
                }
                if p != r {
                    return false;
                }
            }
        }
    }
}

fn is_param(seg: &str) -> bool {
    seg.starts_with('{') && seg.ends_with('}') && !seg.starts_with("{*")
}

fn is_rest_wildcard(seg: &str) -> bool {
    seg.starts_with("{*") && seg.ends_with('}')
}
