use std::sync::OnceLock;

use oxy_shared::errors::OxyError;

pub use oxy_shared::fleet_role::{Role, RouteRole};

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
    let role = match std::env::var("OXY_ROLE").ok().as_deref() {
        Some("ide") => Role::Ide,
        Some("serve") => Role::Serve,
        Some("worker") => Role::Worker,
        Some("all") => Role::All,
        None => default,
        Some(other) => {
            tracing::warn!(
                value = other,
                default = default.as_str(),
                "OXY_ROLE: unrecognised value; using this command's default"
            );
            default
        }
    };
    let _ = PROCESS_ROLE.set(role);
    oxy::workspace_fs_probe::set_process_owns_workspace_files(role_owns_workspace_files(role));
    tracing::info!(role = role.as_str(), "role manifest initialised");
    role
}

pub fn current_process_role() -> Role {
    *PROCESS_ROLE.get().unwrap_or(&Role::All)
}

fn role_owns_workspace_files(role: Role) -> bool {
    matches!(role, Role::Ide | Role::All)
}

pub fn process_can_compile() -> bool {
    role_owns_workspace_files(current_process_role())
}

pub fn role_runs_inprocess_workers(role: Role) -> bool {
    !matches!(role, Role::Serve)
}

pub fn process_is_fs_writable() -> bool {
    current_process_role() != Role::Serve
}

pub fn ensure_fs_writable(operation: &str) -> Result<(), OxyError> {
    if process_is_fs_writable() {
        return Ok(());
    }
    Err(OxyError::RuntimeError(format!(
        "refused workspace filesystem write ({operation}) on a stateless serve \
         replica — writes must run on the filesystem-owning environment (the ide). \
         This indicates a route-classification bug: the route should be IdeOnly."
    )))
}

static DECLARED: OnceLock<Vec<(&'static str, String, RouteRole)>> = OnceLock::new();

/// The SERVER path: `apply_middleware` hands over what the router declared, so
/// every request `classify` sees is answered from a real build.
pub fn install_declarations(decls: Vec<(&'static str, String, RouteRole)>) {
    let _ = DECLARED.set(decls);
}

/// The TEST path: builds the protected router purely to read its declarations
/// back. Nothing in `src/` calls this — a test that classifies must install
/// first, or `classify` answers `FleetOk` for everything.
pub fn install_route_declarations_for_tests() {
    install_route_declarations_for_tests_with(Vec::new());
}

/// The same, plus declarations a surface crate supplies at the composition
/// seam. `oxy-app` cannot depend on those crates, so a test that needs their
/// routes classified has to hand them in — otherwise the route is simply absent
/// and `classify` answers with the FleetOk default, which is the failure this
/// whole file exists to make impossible.
pub fn install_route_declarations_for_tests_with(extra: Vec<(&'static str, String, RouteRole)>) {
    let mut decls = crate::server::router::route_declarations();
    decls.extend(api_prefixed(extra));
    let _ = DECLARED.set(decls);
}

pub fn api_prefixed(
    decls: Vec<(&'static str, String, RouteRole)>,
) -> Vec<(&'static str, String, RouteRole)> {
    decls
        .into_iter()
        .map(|(method, path, role)| (method, format!("/api{path}"), role))
        .collect()
}

pub fn declared_role(method: &str, request_path: &str) -> Option<RouteRole> {
    declared_role_in(DECLARED.get()?, method, request_path)
}

fn declared_role_in(
    declared: &[(&'static str, String, RouteRole)],
    method: &str,
    request_path: &str,
) -> Option<RouteRole> {
    let mut best: Option<(u32, RouteRole)> = None;
    for (decl_method, pattern, role) in declared {
        if *decl_method != "*" && *decl_method != method {
            continue;
        }
        if !pattern_matches(pattern, request_path) {
            continue;
        }
        if best.is_none_or(|(best_rank, _)| specificity(decl_method, pattern) > best_rank) {
            best = Some((specificity(decl_method, pattern), *role));
        }
    }
    best.map(|(_, role)| role)
}

pub fn classify(method: &str, request_path: &str) -> RouteRole {
    let normalised;
    let request_path = match request_path.strip_prefix("/external/api") {
        Some(rest) => {
            normalised = format!("/api{rest}");
            normalised.as_str()
        }
        None => request_path,
    };

    let trimmed = request_path.trim_end_matches('/');
    let request_path = if trimmed.is_empty() {
        request_path
    } else {
        trimmed
    };

    if let Some(role) = declared_role(method, request_path) {
        return role;
    }
    if DECLARED.get().is_none() {
        tracing::error!(
            path = request_path,
            "route roles were never installed; every route classifies FleetOk"
        );
    }
    RouteRole::FleetOk
}

pub fn dump_manifest() -> Vec<(&'static str, String, &'static str)> {
    DECLARED
        .get()
        .map(|d| {
            d.iter()
                .map(|(m, p, r)| (*m, p.clone(), r.as_str()))
                .collect()
        })
        .unwrap_or_default()
}

/// How specific a declaration is, for picking between two that both match.
///
/// Literal segments are the measure. `/secrets/env` and `/secrets/{id}` both
/// match `/secrets/env`, and the rank this replaced could not tell them apart:
/// it scored only "has no `{*}`" and "names a method", so the two tied and
/// `>` kept whichever was mounted first. Both real collisions in the tree
/// happened to be mounted literal-first, so classification was correct by
/// accident — reordering a mount would have flipped `/secrets/env` to FleetOk
/// and let a replica with no working copy answer it.
///
/// `{*rest}` still loses to everything, and an exact method still breaks a tie
/// between two equally literal patterns.
pub(crate) fn specificity(decl_method: &str, pattern: &str) -> u32 {
    let literal_segments = pattern
        .split('/')
        .filter(|seg| !seg.is_empty() && !seg.starts_with('{'))
        .count() as u32;
    (u32::from(!pattern.contains("{*")) << 16)
        | (literal_segments << 1)
        | u32::from(decl_method != "*")
}

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

#[cfg(test)]
#[path = "role_manifest_tests.rs"]
mod tests;
