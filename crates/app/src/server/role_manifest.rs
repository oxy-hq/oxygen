use std::sync::OnceLock;

use oxy_shared::errors::OxyError;

pub use oxy_app_core::process_role::{
    current_process_role, init_process_role_from_env, init_process_role_from_env_with_default,
    role_owns_workspace_files,
};
pub use oxy_shared::fleet_role::{Role, RouteRole};

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

pub fn ensure_fs_writable(operation: &str) -> Result<(), OxyError> {
    if process_is_fs_writable() {
        return Ok(());
    }
    Err(OxyError::RuntimeError(format!(
        "refused workspace filesystem write ({operation}) on a process that owns \
         no working copy (serve or worker) — writes must run where the workspace \
         files are (the ide, or single-process mode). This indicates a \
         route-classification bug: the route should be IdeOnly."
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

/// [`declared_role`] against a list the caller holds instead of the one this
/// process installed: the most specific declaration matching `method
/// request_path`, or `None` when none does. For a test that composes the
/// declarations of several crates the way the server does, without racing
/// another test in the same binary for the process-wide list.
pub fn declared_role_in(
    declared: &[(&'static str, String, RouteRole)],
    method: &str,
    request_path: &str,
) -> Option<RouteRole> {
    best_declaration_in(declared, method, request_path).map(|(_, role)| role)
}

/// The most specific declaration matching `method request_path`: its pattern
/// and its role.
fn best_declaration_in<'a>(
    declared: &'a [(&'static str, String, RouteRole)],
    method: &str,
    request_path: &str,
) -> Option<(&'a str, RouteRole)> {
    let mut best: Option<(u32, &'a str, RouteRole)> = None;
    for (decl_method, pattern, role) in declared {
        if *decl_method != "*" && *decl_method != method {
            continue;
        }
        if !pattern_matches(pattern, request_path) {
            continue;
        }
        let rank = specificity(decl_method, pattern);
        if best.is_none_or(|(best_rank, _, _)| rank > best_rank) {
            best = Some((rank, pattern.as_str(), *role));
        }
    }
    best.map(|(_, pattern, role)| (pattern, role))
}

/// The declared route pattern `method request_path` classifies under, when a
/// declaration matches: a route template, so it is safe as a metric label
/// where the concrete path (ids and all) is not.
pub fn declared_pattern(method: &str, request_path: &str) -> Option<String> {
    let path = normalise(request_path);
    best_declaration_in(DECLARED.get()?, method, &path).map(|(pattern, _)| pattern.to_string())
}

/// `/external/api/..` reads as `/api/..`, and a trailing slash is dropped.
fn normalise(request_path: &str) -> String {
    let path = match request_path.strip_prefix("/external/api") {
        Some(rest) => format!("/api{rest}"),
        None => request_path.to_string(),
    };
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        path
    } else {
        trimmed.to_string()
    }
}

pub fn classify(method: &str, request_path: &str) -> RouteRole {
    let request_path = &normalise(request_path);
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
