use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::server::role_manifest::{Role, RouteRole, classify, current_process_role};

const HEADER_SERVED_BY: &str = "x-oxy-served-by";
const HEADER_REQUIRED_ROLE: &str = "x-oxy-required-role";
const HEADER_FORWARDED_VIA: &str = "x-oxy-forwarded-via";

pub async fn enforce_role(mut req: Request, next: Next) -> Response {
    let role = current_process_role();
    if matches!(role, Role::All) {
        return stamp(next.run(req).await, role);
    }

    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let classified = classify(&method, &path);
    let mut route_role = if branch_names_a_resource(&path) {
        classified
    } else {
        escalate_for_branch(classified, req.uri().query())
    };
    // A preview request keeps a fleet route on the fleet: its `?branch=` names a
    // compiled preview, not the ide's working copy. Whether the preview applies
    // depends on who is asking, which is not known yet — so the workspace
    // middleware finishes the decision, forwarding to the ide after all when it
    // does not (see `previews::pin::DeferredBranchEscalation`).
    if matches!(role, Role::Serve)
        && classified == RouteRole::FleetOk
        && route_role == RouteRole::IdeOnly
        && crate::server::previews::pin::has_preview_header(req.headers())
    {
        route_role = RouteRole::FleetOk;
        req.extensions_mut()
            .insert(crate::server::previews::pin::DeferredBranchEscalation);
    }
    if route_role.accepted_by(role) {
        return stamp(next.run(req).await, role);
    }

    if matches!(role, Role::Serve)
        && matches!(route_role, RouteRole::IdeOnly)
        && let Some(upstream) = crate::server::ide_proxy::ide_upstream()
    {
        if crate::server::ide_proxy::already_forwarded(&req) {
            tracing::error!(
                method = %method,
                path = %path,
                "ide_proxy loop guard: re-forwarded request reached a serve replica — \
                 OXY_IDE_UPSTREAM must target ide-only pods; rejecting"
            );
        } else {
            if crate::server::serve_safety::analytics_fleet_unpin_enabled()
                && let Some(ws) = crate::server::serve_safety::analytics_workspace_id(&path)
                && crate::server::serve_safety::workspace_is_serve_safe(ws).await
            {
                tracing::debug!(
                    workspace_id = %ws,
                    path = %path,
                    "serve replica: serve-safe /analytics handled locally (fleet un-pin)"
                );
                return stamp(next.run(req).await, role);
            }
            tracing::debug!(
                method = %method,
                path = %path,
                "serve replica: forwarding IdeOnly route to ide upstream"
            );
            return stamp_forwarded_via(
                crate::server::ide_proxy::forward_to_ide(upstream, req).await,
                role,
            );
        }
    }

    let required = required_role_for(route_role);
    tracing::warn!(
        method = %method,
        path = %path,
        process_role = role.as_str(),
        required_role = required,
        "misroute: process role does not accept this route (no ide upstream to forward to)"
    );
    let body = format!(
        "this oxy server runs as role '{}'; route '{} {}' is classified '{}' and must be served by role '{}'",
        role.as_str(),
        method,
        path,
        route_role.as_str(),
        required,
    );
    let mut resp = (StatusCode::MISDIRECTED_REQUEST, body).into_response();
    if let Ok(v) = HeaderValue::from_str(required) {
        resp.headers_mut().insert(HEADER_REQUIRED_ROLE, v);
    }
    stamp(resp, role)
}

fn escalate_for_branch(role: RouteRole, query: Option<&str>) -> RouteRole {
    if matches!(role, RouteRole::IdeOnly) {
        return role;
    }
    let has_branch = query.is_some_and(|q| {
        q.split('&')
            .filter_map(|pair| pair.split_once('='))
            .any(|(k, v)| k == "branch" && !v.is_empty())
    });
    if has_branch { RouteRole::IdeOnly } else { role }
}

/// Routes whose `?branch=` names the resource being acted on rather than a
/// working copy to read — the previews API (`DELETE /previews?branch=X`,
/// `POST /previews/refresh?branch=X`, `GET /previews/checks?branch=X`,
/// `GET /previews/runs?branch=X`) and the staging compile
/// (`POST /compile/staging?branch=X`, the branch to compile). The escalation
/// above would otherwise send a request no working copy answers to the ide.
///
/// The workspace middleware asks the same question, for the one of these that
/// is mounted under it: there `?branch=` also picks the revision a request
/// reads and the working copy a manager is built on.
pub(crate) fn branch_names_a_resource(path: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "/api/{workspace_id}/previews",
        "/api/{workspace_id}/previews/refresh",
        "/api/{workspace_id}/previews/checks",
        "/api/{workspace_id}/previews/runs",
        "/api/{workspace_id}/compile/staging",
    ];
    PATTERNS
        .iter()
        .any(|p| crate::server::role_manifest::pattern_matches(p, path))
}

fn required_role_for(route_role: RouteRole) -> &'static str {
    match route_role {
        RouteRole::IdeOnly => "ide",
        RouteRole::FleetOk => "serve",
        RouteRole::WorkerOnly => "worker",
    }
}

fn stamp(mut resp: Response<Body>, role: Role) -> Response<Body> {
    // A handler that relayed this answer from the Factory from inside a
    // `FleetOk` route (`invocation_placement`) has already stamped it with
    // [`stamp_forwarded_via`]. Its `served-by` is the Factory's and stays:
    // this replica did not serve it, and overwriting would make the hop
    // invisible to anyone reading the headers.
    if resp.headers().contains_key(HEADER_FORWARDED_VIA) {
        return resp;
    }
    let header = format!("{}@{}", role.as_str(), worker_id());
    if let Ok(v) = HeaderValue::from_str(&header) {
        resp.headers_mut().insert(HEADER_SERVED_BY, v);
    }
    resp
}

/// The headers that say a replica relayed the Factory's answer: this process
/// in `x-oxy-forwarded-via`, and — only when the Factory stamped none of its
/// own — in `x-oxy-served-by` too. [`enforce_role`] applies it when it
/// forwards an `IdeOnly` route; a handler that forwards from inside a
/// `FleetOk` route applies the same, so the pair an operator reads a hop from
/// (`forwarded-via: serve@…`, `served-by: ide@…`) holds on both paths.
pub(crate) fn stamp_forwarded_via(mut resp: Response<Body>, role: Role) -> Response<Body> {
    let Ok(v) = HeaderValue::from_str(&format!("{}@{}", role.as_str(), worker_id())) else {
        return resp;
    };
    resp.headers_mut().insert(HEADER_FORWARDED_VIA, v.clone());
    if !resp.headers().contains_key(HEADER_SERVED_BY) {
        resp.headers_mut().insert(HEADER_SERVED_BY, v);
    }
    resp
}

fn worker_id() -> String {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string());
    format!("{host}#{}", std::process::id())
}

#[cfg(test)]
#[path = "role_middleware_tests.rs"]
mod tests;
