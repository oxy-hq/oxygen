//! The HTTP half of the custom-domain sign-in (the rules, and why the cookie
//! is a narrowed token session, are in `oxy_custom_apps::custom_domains::session`).
//!
//! | Step | Route | Host |
//! | ---- | ----- | ---- |
//! | 1 | the serve path's refusal → [`start`] | custom domain |
//! | 2 | `GET /api/custom-domains/authorize` → [`authorize`] | product host |
//! | 3 | `GET /api/custom-domains/session` → [`session`] | custom domain |
//!
//! Plus `GET /api/custom-domains/tls-ask` ([`tls_ask`]), the TLS edge's
//! question before it requests a certificate for a hostname.
//!
//! **Login CSRF.** A ticket is a bearer of a session, so a page that could make
//! a visitor's browser redeem the page author's ticket would sign the visitor
//! in as the author. Step 1 sets a random `state` as a host-only cookie on the
//! custom domain and carries it through step 2; step 3 refuses unless the
//! query's `state` equals that cookie, which no other site can read or set.
//!
//! **The ticket in a query string.** Step 3 is a server redirect, so the
//! ticket cannot ride in a fragment as `/token-login`'s does. It is single
//! use and spent by the request that carries it, and step 3 answers with a
//! redirect, so no page ever loads with it in the URL to leak by `Referer`.

use axum::extract::{Extension, Query};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use chrono::Utc;
use oxy::database::client::establish_connection;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_app_core::custom_domain_host::{self, CustomDomainCtx};
use oxy_auth::constants::SESSION_COOKIE_NAME;
use oxy_auth::token::SandboxAgent;
use oxy_auth::token::browser_session;
use oxy_custom_apps::custom_domains::{self, session as domain_session};
use oxy_custom_apps::resolve::{Refusal, resolve_app_request};
use serde::Deserialize;

/// The cookie step 1 sets and step 3 checks. Path-scoped to step 3, so it is
/// sent nowhere else.
const STATE_COOKIE: &str = "oxy_domain_state";
const SESSION_PATH: &str = "/api/custom-domains/session";
const AUTHORIZE_PATH: &str = "/api/custom-domains/authorize";
/// How long a visitor has to finish signing in on the product host.
const STATE_TTL_SECS: i64 = 15 * 60;

#[derive(Debug, Deserialize)]
pub struct AuthorizeQuery {
    domain: String,
    state: String,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SessionQuery {
    ticket: String,
    state: String,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TlsAskQuery {
    domain: String,
}

// ── Step 1 ────────────────────────────────────────────────────────────────

/// What the serve path answers an unauthenticated visitor on a custom domain:
/// a redirect to the product host's [`authorize`], with a fresh `state` set as
/// a cookie here. `rest` is the visitor's path on the domain, `/` and up.
pub fn start(headers: &HeaderMap, ctx: &CustomDomainCtx, rest: &str) -> Response {
    let state = uuid::Uuid::new_v4().simple().to_string();
    let base = oxy_app_core::custom_apps_host_dispatch::admin_base_url().unwrap_or_default();
    let target = format!(
        "{base}{AUTHORIZE_PATH}?domain={}&state={state}&next={}",
        urlencoding::encode(&ctx.domain),
        urlencoding::encode(&safe_next(Some(rest))),
    );
    let cookie = format!(
        "{STATE_COOKIE}={state}; Path={SESSION_PATH}; Max-Age={STATE_TTL_SECS}; HttpOnly; \
         SameSite=Lax{}",
        secure_attr(headers)
    );
    with_cookies(Redirect::to(&target).into_response(), &[cookie])
}

// ── Step 2 ────────────────────────────────────────────────────────────────

/// `GET /api/custom-domains/authorize` on the product host. Signs the visitor
/// in here if they are not, checks they may open the app, and sends the
/// browser back to the domain with a one-time ticket.
pub async fn authorize(
    headers: HeaderMap,
    on_domain: Option<Extension<CustomDomainCtx>>,
    Query(q): Query<AuthorizeQuery>,
) -> Response {
    // Only the product host carries the visitor's login.
    if on_domain.is_some() || !valid_state(&q.state) {
        return page(StatusCode::NOT_FOUND, "This sign-in link is not valid.");
    }
    let Some(ctx) = resolve_domain(&q.domain).await else {
        return page(
            StatusCode::NOT_FOUND,
            "This domain is not connected to an app.",
        );
    };
    let request = match resolve_app_request(
        &headers,
        (ctx.org_slug.as_str(), ctx.app_slug.as_str()),
        SandboxAgent::Refuse,
        &AppEnvironment::Production,
        oxy_server_authz::enforcer(),
    )
    .await
    {
        Ok(request) => request,
        Err(Refusal::Unauthenticated(_) | Refusal::UnknownUser) => return to_login(&q),
        Err(Refusal::NotFound | Refusal::Forbidden(_)) => {
            return page(StatusCode::FORBIDDEN, "You don't have access to this app.");
        }
        Err(Refusal::Internal(_)) => return internal(),
    };
    // A person's login only: a token already carries a narrowed reach, and a
    // session opened from one must not renew itself through another domain.
    if request.user.credential.is_some() {
        return page(
            StatusCode::FORBIDDEN,
            "Sign in to Oxygen in this browser to open this app.",
        );
    }
    match domain_session::open_ticket(&request.db, request.user.id, &ctx).await {
        Ok(ticket) => Redirect::to(&format!(
            "{}://{}{SESSION_PATH}?ticket={}&state={}&next={}",
            domain_scheme(),
            ctx.domain,
            urlencoding::encode(&ticket.ticket),
            q.state,
            urlencoding::encode(&safe_next(q.next.as_deref())),
        ))
        .into_response(),
        Err(e) => {
            tracing::error!(domain = %ctx.domain, "custom domain ticket failed: {e}");
            internal()
        }
    }
}

/// The product host's login, coming back to this same authorize URL.
fn to_login(q: &AuthorizeQuery) -> Response {
    let base = oxy_app_core::custom_apps_host_dispatch::admin_base_url().unwrap_or_default();
    let back = format!(
        "{base}{AUTHORIZE_PATH}?domain={}&state={}&next={}",
        urlencoding::encode(&q.domain),
        q.state,
        urlencoding::encode(&safe_next(q.next.as_deref())),
    );
    Redirect::to(&format!(
        "{base}/login?return_to={}",
        urlencoding::encode(&back)
    ))
    .into_response()
}

// ── Step 3 ────────────────────────────────────────────────────────────────

/// `GET /api/custom-domains/session` on the custom domain. Redeems the ticket
/// and sets this host's own `oxy_session`.
pub async fn session(
    headers: HeaderMap,
    on_domain: Option<Extension<CustomDomainCtx>>,
    Query(q): Query<SessionQuery>,
) -> Response {
    let Some(Extension(ctx)) = on_domain else {
        return page(StatusCode::NOT_FOUND, "This sign-in link is not valid.");
    };
    if !valid_state(&q.state) || cookie(&headers, STATE_COOKIE).as_deref() != Some(&q.state) {
        return expired();
    }
    let Ok(db) = establish_connection().await else {
        return internal();
    };
    let (row, user) = match domain_session::redeem(&db, q.ticket.trim(), &ctx.domain).await {
        Ok(found) => found,
        Err(e) => {
            tracing::info!(domain = %ctx.domain, "custom domain sign-in refused: {e}");
            return expired();
        }
    };
    let now = Utc::now();
    let Ok(session) = browser_session::mint(&row, user.email.as_deref().unwrap_or(""), now) else {
        return internal();
    };
    // Admit it as every later request will. An org whose token policy refuses
    // personal tokens refuses this one too — answered here, once, rather than
    // by a redirect back to step 1 that would loop.
    if let Err(e) =
        oxy_auth::token::authenticate_browser_session(&crate::auth_platform(), &session.jwt).await
    {
        tracing::info!(domain = %ctx.domain, "custom domain session not admitted: {e}");
        return page(
            StatusCode::FORBIDDEN,
            "Your organization does not allow signing in to this app here.",
        );
    }
    let secure = secure_attr(&headers);
    let session_cookie = format!(
        "{SESSION_COOKIE_NAME}={}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax{secure}",
        session.jwt,
        session.max_age_secs(now),
    );
    let clear_state =
        format!("{STATE_COOKIE}=; Path={SESSION_PATH}; Max-Age=0; HttpOnly; SameSite=Lax{secure}");
    let next = safe_next(q.next.as_deref());
    with_cookies(
        Redirect::to(&next).into_response(),
        &[session_cookie, clear_state],
    )
}

/// The `Set-Cookie` that ends this host's own session — what logout sends on a
/// custom domain, where the product's `Domain=`-scoped clear cannot reach.
pub fn clear_session_cookie(headers: &HeaderMap) -> String {
    format!(
        "{SESSION_COOKIE_NAME}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax{}",
        secure_attr(headers)
    )
}

// ── TLS edge ──────────────────────────────────────────────────────────────

/// `GET /api/custom-domains/tls-ask?domain=` — `200` when the TLS edge may
/// request a certificate for `domain` (a verified custom domain), `404`
/// otherwise. The on-demand-TLS contract (Caddy's `on_demand_tls.ask`): it is
/// what stops a stranger pointing any hostname at the edge from making it
/// issue certificates. Unauthenticated: whether a hostname is connected is
/// already public in its DNS.
pub async fn tls_ask(Query(q): Query<TlsAskQuery>) -> StatusCode {
    if custom_domains::is_verified(&q.domain).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// A known domain is read again from the database: the answer names the
/// workspace [`authorize`]'s token is granted on, and one sign-in per twelve
/// hours can afford the read. An unknown one answers from the cache, so a
/// spray of made-up names costs no reads.
async fn resolve_domain(domain: &str) -> Option<CustomDomainCtx> {
    let candidate = custom_domain_host::candidate_host(domain)?;
    custom_domain_host::resolve(&candidate).await?;
    custom_domain_host::resolve_fresh(&candidate).await
}

/// A path on the same host and nothing else: absolute, not protocol-relative
/// (`//evil`), no backslash a browser reads as a slash. Anything else is `/`.
pub(crate) fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(path)
            if path.starts_with('/')
                && !path.starts_with("//")
                && !path.contains('\\')
                && !path.chars().any(char::is_control) =>
        {
            path.to_string()
        }
        _ => "/".to_string(),
    }
}

/// A `state` this module minted: 32 lowercase hex characters.
fn valid_state(state: &str) -> bool {
    state.len() == 32
        && state
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `https` unless `OXY_CUSTOM_DOMAIN_SCHEME` says otherwise (a local test of
/// the handoff against a hosts-file name has no certificate).
fn domain_scheme() -> String {
    std::env::var("OXY_CUSTOM_DOMAIN_SCHEME")
        .ok()
        .filter(|s| s == "http" || s == "https")
        .unwrap_or_else(|| "https".to_string())
}

fn secure_attr(headers: &HeaderMap) -> &'static str {
    let plain_http = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|p| p.eq_ignore_ascii_case("http"))
        && domain_scheme() == "http";
    if plain_http { "" } else { "; Secure" }
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

fn with_cookies(mut response: Response, cookies: &[String]) -> Response {
    for c in cookies {
        if let Ok(value) = HeaderValue::from_str(c) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}

fn expired() -> Response {
    page(
        StatusCode::BAD_REQUEST,
        "This sign-in link has expired. Reload the app to try again.",
    )
}

fn internal() -> Response {
    page(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Something went wrong signing you in. Try again in a minute.",
    )
}

/// A plain page: these routes are browser navigations, so a bare status would
/// show the visitor nothing.
fn page(status: StatusCode, message: &'static str) -> Response {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"viewport\" \
         content=\"width=device-width\"><title>Oxygen</title>\
         <p style=\"font:16px system-ui;margin:3rem auto;max-width:32rem;padding:0 1rem\">\
         {message}</p>"
    );
    (status, Html(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_stays_on_the_host() {
        assert_eq!(safe_next(Some("/dashboard?x=1")), "/dashboard?x=1");
        assert_eq!(safe_next(Some("//evil.example/")), "/");
        assert_eq!(safe_next(Some("/\\evil.example")), "/");
        assert_eq!(safe_next(Some("https://evil.example/")), "/");
        assert_eq!(safe_next(Some("/a\nb")), "/");
        assert_eq!(safe_next(None), "/");
    }

    #[test]
    fn state_is_only_what_start_mints() {
        assert!(valid_state(&uuid::Uuid::new_v4().simple().to_string()));
        assert!(!valid_state("short"));
        assert!(!valid_state(&"Z".repeat(32)));
        assert!(!valid_state("0123456789abcdef0123456789abcdef&x=1"));
    }

    #[test]
    fn reads_one_cookie_among_many() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; oxy_domain_state=abc; b=2"),
        );
        assert_eq!(cookie(&headers, STATE_COOKIE).as_deref(), Some("abc"));
        assert_eq!(cookie(&headers, "missing"), None);
    }

    #[test]
    fn start_sets_a_path_scoped_state_cookie() {
        let ctx = CustomDomainCtx {
            domain: "app.acme.com".into(),
            app_id: uuid::Uuid::new_v4(),
            org_id: uuid::Uuid::new_v4(),
            project_id: uuid::Uuid::new_v4(),
            org_slug: "acme".into(),
            app_slug: "store".into(),
        };
        let response = start(&HeaderMap::new(), &ctx, "/orders?id=7");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response.headers()[header::LOCATION].to_str().unwrap();
        assert!(location.contains("/api/custom-domains/authorize?domain=app.acme.com&state="));
        assert!(location.ends_with("&next=%2Forders%3Fid%3D7"), "{location}");
        let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(set_cookie.starts_with("oxy_domain_state="));
        assert!(set_cookie.contains("Path=/api/custom-domains/session"));
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("Secure"));
        // The state in the URL is the state in the cookie.
        let state = set_cookie
            .trim_start_matches("oxy_domain_state=")
            .split(';')
            .next()
            .unwrap();
        assert!(location.contains(&format!("state={state}")));
    }
}
