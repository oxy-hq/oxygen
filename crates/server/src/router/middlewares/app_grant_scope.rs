//! The `/api` half of the route allow-list for a credential whose grants name
//! **apps** (sandbox agent credential design §3.1).
//!
//! [`app_grant_scope_middleware`] sits in `api_auth_layers`, after
//! authentication, and reads the request's `CredentialContext`. A sandbox
//! agent token (`oxy_sbx_`) is answered `404` on every route the `app_sandbox`
//! table does not list, the answer an out-of-scope App Operator gets, so it
//! cannot learn which ids exist.
//!
//! The tables (`SANDBOX_ALLOWED`, `PUBLIC_ROUTES`), the matcher
//! (`sandbox_admits`) and the serve tree's half (`serve_tree_refuses`, asked
//! first in `serve_dispatch`, before authentication) are
//! `oxy_custom_apps::sandbox_reach`, where the design's whole story is told.
//! This half only refuses.

use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use oxy_auth::token::CredentialContext;
use oxy_custom_apps::sandbox_reach::sandbox_admits;

/// The `/api` half. A no-op for every credential but a sandbox agent token.
pub async fn app_grant_scope_middleware(
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(credential) = request
        .extensions()
        .get::<CredentialContext>()
        .filter(|credential| credential.is_sandbox_agent())
    else {
        return Ok(next.run(request).await);
    };
    if !sandbox_admits(request.method(), request.uri().path(), |app| {
        credential.sandboxes_app(app)
    }) {
        tracing::warn!(
            token_id = %credential.token_id,
            method = %request.method(),
            path = %request.uri().path(),
            "sandbox agent token reached outside the sandbox loop — 404"
        );
        return Err(StatusCode::NOT_FOUND);
    }
    // One caller for the whole request: the guards after this share its one
    // uncached read of the minter's platform grant (design §4, "Cost").
    oxy_server_authz::Caller::share_with(request.extensions_mut());
    Ok(next.run(request).await)
}

#[cfg(test)]
#[path = "app_grant_scope_tests.rs"]
mod tests;
