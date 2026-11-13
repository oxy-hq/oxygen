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

/// The prefix the external API surface is nested under in `serve.rs`.
pub const EXTERNAL_API_PREFIX: &str = "/external/api";

pub async fn enforce_role(req: Request, next: Next) -> Response {
    enforce_role_under("", req, next).await
}

/// [`enforce_role`] for the `/external/api` surface, which layers it INSIDE its
/// `nest`. There axum has already stripped the prefix, so the request reads
/// `/{ws}/sql/query`: no declaration matches that, so an IdeOnly agentic route
/// would default to FleetOk and run here, and a forward would hand the ide the
/// stripped path — its static SPA fallback, `405 Allow: GET,HEAD` for every
/// POST, which is what broke every custom app whose SDK sent `?branch=main`
/// (Poke House's Command Center and Time to Serve on dev, 2026-11-09). The
/// prefix is put back for both.
pub async fn enforce_role_external(req: Request, next: Next) -> Response {
    enforce_role_under(EXTERNAL_API_PREFIX, req, next).await
}

/// `prefix` is what a `nest` above this layer stripped; it is put back for the
/// classification and for the forward, and left off for `next`, which routes on
/// the stripped remainder.
async fn enforce_role_under(prefix: &str, mut req: Request, next: Next) -> Response {
    let role = current_process_role();
    if matches!(role, Role::All) {
        return stamp(next.run(req).await, role);
    }

    let method = req.method().as_str().to_string();
    let path = format!("{prefix}{}", req.uri().path());
    let classified = classify(&method, &path);
    // `?branch=` escalates nothing: there is no working copy to read, and the
    // request is served from the compiled revision on this pod.
    let route_role = classified;
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
            tracing::debug!(
                method = %method,
                path = %path,
                "serve replica: forwarding IdeOnly route to ide upstream"
            );
            restore_prefix(prefix, &mut req);
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

/// Put back the prefix a `nest` stripped, query included, so the ide routes the
/// request as the client sent it.
fn restore_prefix(prefix: &str, req: &mut Request) {
    if prefix.is_empty() {
        return;
    }
    let rest = req
        .uri()
        .path_and_query()
        .map_or_else(|| req.uri().path(), |pq| pq.as_str())
        .to_string();
    match format!("{prefix}{rest}").parse() {
        Ok(uri) => *req.uri_mut() = uri,
        Err(e) => tracing::error!(%prefix, %rest, "cannot restore nested prefix: {e}"),
    }
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
pub fn stamp_forwarded_via(mut resp: Response<Body>, role: Role) -> Response<Body> {
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
