//! Count every request an API key or token made, and name the token on the
//! request span (API-tokens design §3.7: usage, and the `token_id` attribute).
//!
//! Sits **inside** the auth gate, so it sees the `CredentialContext` auth
//! attached; a session request passes straight through, and a request auth
//! refused never arrives (it named no token to count against). It wraps the
//! timeout layer, so a timed-out request is counted with its real status.
//!
//! What it records: the token **id**, the response status, the client IP and
//! user agent, and the **matched route template** (`/api/{workspace_id}/…`) —
//! never the raw path, which can carry ids and secrets, and never the token.
//! The IP is the address the load balancer saw (`forwarded::client_ip`), not
//! the `X-Forwarded-For` hop the caller wrote.
//! Recording is one map update under a mutex; the write happens once a minute
//! in `server::token_usage_flush`.

use axum::body::Body;
use axum::extract::MatchedPath;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use chrono::Utc;
use oxy_app_core::audit::user_agent;
use oxy_app_core::forwarded::client_ip;
use oxy_auth::token::CredentialContext;
use oxy_auth::token::usage::{self, UsageSample};
use oxy_telemetry::http_trace::RequestTokenId;

pub async fn token_usage_middleware(request: Request<Body>, next: Next) -> Response {
    let Some(token_id) = request
        .extensions()
        .get::<CredentialContext>()
        .map(|c| c.token_id)
    else {
        return next.run(request).await;
    };
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string());
    let ip = client_ip(request.headers());
    let user_agent = user_agent(request.headers());

    let mut response = next.run(request).await;

    // The request span was made before auth ran; hand it the id on the way out.
    response
        .extensions_mut()
        .insert(RequestTokenId(token_id.to_string()));
    usage::record(UsageSample {
        token_id,
        status: response.status().as_u16(),
        ip,
        user_agent,
        route,
        at: Utc::now(),
    });
    response
}
