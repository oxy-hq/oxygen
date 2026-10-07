//! The route allow-list for a credential whose grants name **apps**.
//!
//! One module, a table per grant kind (sandbox agent credential design §3.1).
//! It holds the `app_sandbox` table, [`SANDBOX_ALLOWED`]: the sandbox loop as
//! exact `(method, path shape)` pairs. A sandbox agent token (`oxy_sbx_`) is
//! answered `404` on every other route of the trees this is mounted on, the
//! answer an out-of-scope App Operator gets, so it cannot learn which ids
//! exist.
//!
//! Two halves, because the loop's routes live on two trees:
//!
//! - [`app_grant_scope_middleware`] sits in `api_auth_layers`, after
//!   authentication, and reads the request's `CredentialContext`.
//! - [`serve_tree_refuses`] is asked first in `serve_dispatch`, BEFORE
//!   authentication, so it keys on the presented `oxy_sbx_` prefix.
//!
//! Both only refuse. A shape is matched segment for segment, never by prefix,
//! so `/customer-apps/{id}/secrets/{key}/value` cannot ride on the shape of
//! the delete beside it. Whether the environment a route names is one the
//! token created is the handler's check (§3.3), which needs the database.
//!
//! **Staging adds no route.** A token granted an app's staging uses the rows
//! of [`SANDBOX_ALLOWED`] with `staging` where it named a sandbox, and F1 with
//! `X-Oxy-App-Env: staging`. The serve tree's half is asked before the token
//! is known, so it admits the header by shape — a sandbox's name, or
//! `staging` — and whether **this** token was granted that app's staging is
//! decided once it is: the credential's own grant, then oxy-authz
//! (`custom_apps_agent::holds_staging`, `custom_apps_functions::agent_gate`).
//! A token minted without staging is answered the same `404`, by that gate.
//!
//! `/logs` is on the public router, where no fence is mounted: it admits the
//! token at authentication and every other public route refuses it there
//! ([`PUBLIC_ROUTES`]).

use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::token::CredentialContext;
use uuid::Uuid;

/// One row of an allow-list: a method and an exact path shape, as the `/api`
/// tree sees it (no `/api` prefix).
///
/// `{id}` must be an app the token is granted. Any other `{…}` segment is one
/// non-empty segment.
pub struct Allowed {
    /// The row's name in the design's table (§1).
    pub row: &'static str,
    pub method: &'static str,
    pub shape: &'static str,
}

const fn row(row: &'static str, method: &'static str, shape: &'static str) -> Allowed {
    Allowed { row, method, shape }
}

/// What an `app_sandbox` grant reaches on the `/api` tree: the sandbox loop.
/// §1's table as data, minus F1 (the serve tree) and L1 (the public router).
pub const SANDBOX_ALLOWED: &[Allowed] = &[
    row("A1", "GET", "/auth/token"),
    row("A2", "DELETE", "/auth/token"),
    row("E1", "GET", "/customer-apps/{id}/environments"),
    row("E2", "POST", "/customer-apps/{id}/environments"),
    row("E3", "GET", "/customer-apps/{id}/environments/{name}"),
    row("E3", "DELETE", "/customer-apps/{id}/environments/{name}"),
    row("P1", "POST", "/customer-apps/publish"),
    row("C1", "GET", "/customer-apps/{id}/functions"),
    row("C2", "POST", "/customer-apps/{id}/functions/{name}/runs"),
    row("C3", "GET", "/customer-apps/{id}/function-runs/{run_id}"),
    row(
        "R1",
        "GET",
        "/customer-apps/{id}/functions/{name}/invocations",
    ),
    row("R2", "GET", "/customer-apps/{id}/invocations"),
    row(
        "R3",
        "GET",
        "/customer-apps/{id}/invocations/{invocation_id}/held",
    ),
    row("S1", "POST", "/customer-apps/{id}/secrets"),
    row("S2", "DELETE", "/customer-apps/{id}/secrets/{key}"),
    row("S3", "GET", "/customer-apps/{id}/secrets"),
];

/// Every `/customer-apps/…` route of the **public** router, and whether it
/// admits a sandbox agent token. No fence is mounted there, so each states its
/// answer where it authenticates; a test holds this table to the routes that
/// exist, and another pins the call sites that say `Admit`.
pub const PUBLIC_ROUTES: &[(&str, bool)] = &[
    ("/customer-apps/{org_slug}/{app_slug}/logs", true),
    ("/customer-apps/{org_slug}/{app_slug}/errors", false),
    ("/customer-apps/{org_slug}/{app_slug}/debug", false),
    ("/customer-apps/{org_slug}/{app_slug}/health", false),
    ("/customer-apps/{org_slug}/{app_slug}/availability", false),
    ("/customer-apps/health", false),
    ("/customer-apps/publish/oidc-exchange", false),
    ("/customer-apps/{project_id}/events", false),
];

/// Whether `path` is exactly `shape`. `Some(app)` carries the id the path
/// names where the shape has `{id}`.
fn match_shape(shape: &str, path: &str) -> Option<Option<Uuid>> {
    let mut app = None;
    let mut want = shape.split('/');
    let mut got = path.split('/');
    loop {
        match (want.next(), got.next()) {
            (None, None) => return Some(app),
            (Some("{id}"), Some(segment)) => app = Some(Uuid::parse_str(segment).ok()?),
            (Some(wanted), Some(segment)) if wanted.starts_with('{') => {
                if segment.is_empty() {
                    return None;
                }
            }
            (Some(wanted), Some(segment)) if wanted == segment => {}
            _ => return None,
        }
    }
}

/// Whether the sandbox table admits `method path`, for a token granted the
/// apps `granted` answers true for.
pub fn sandbox_admits(method: &Method, path: &str, granted: impl Fn(Uuid) -> bool) -> bool {
    SANDBOX_ALLOWED.iter().any(|allowed| {
        allowed.method == method.as_str()
            && match match_shape(allowed.shape, path) {
                Some(Some(app)) => granted(app),
                Some(None) => true,
                None => false,
            }
    })
}

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
    crate::server::authz::Caller::share_with(request.extensions_mut());
    Ok(next.run(request).await)
}

/// The serve tree's half: whether a request presenting a sandbox agent token
/// is refused there. `path` is `serve_dispatch`'s `{*path}` capture,
/// `<org>/<app>/<rest>`.
///
/// Asked before authentication, so it reads the presented prefix and nothing
/// from the database. `false` for every other credential.
pub(crate) fn serve_tree_refuses(method: &Method, headers: &HeaderMap, path: &str) -> bool {
    oxy_auth::token::presents_sandbox_agent(headers) && !serve_tree_admits(method, headers, path)
}

/// F1, and nothing else: `POST <org>/<app>/fn/<name>` on the product host,
/// naming a `dev-*` sandbox, or `staging`, in `X-Oxy-App-Env`.
///
/// An app or org subdomain is refused whatever it asks: its host label would
/// decide the environment before the header does. Whether the sandbox is the
/// token's own — and whether the token was granted the app's staging at all —
/// is `environment_gate`'s check, made once the token is known.
fn serve_tree_admits(method: &Method, headers: &HeaderMap, path: &str) -> bool {
    if method != Method::POST || on_tenant_host(headers) {
        return false;
    }
    let mut segments = path.trim_start_matches('/').split('/');
    let shape = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    );
    let (Some(org), Some(app), Some("fn"), Some(name), None) = shape else {
        return false;
    };
    !org.is_empty() && !app.is_empty() && !name.is_empty() && names_an_agent_environment(headers)
}

/// Whether the request arrived on an app subdomain or an org subdomain.
fn on_tenant_host(headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    oxy_app_core::custom_apps_host_dispatch::parse_app_host(host).is_some()
        || oxy_app_core::org_host_dispatch::parse_org_subdomain(host).is_some()
}

/// Whether `X-Oxy-App-Env` is present and names an environment a sandbox
/// agent token can hold: a `dev-*` sandbox, or `staging`. Never production —
/// named, or meant by naming nothing.
fn names_an_agent_environment(headers: &HeaderMap) -> bool {
    headers
        .get(oxy_app_core::custom_app_env_request::ENV_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|name| AppEnvironment::parse(name.trim()))
        .is_some_and(|environment| match environment {
            AppEnvironment::Dev { .. } | AppEnvironment::Staging => true,
            AppEnvironment::Production => false,
        })
}

#[cfg(test)]
#[path = "app_grant_scope_tests.rs"]
mod tests;
