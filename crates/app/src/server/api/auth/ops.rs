use axum::{
    http::{HeaderMap, StatusCode},
    response::Json,
};
use chrono::{Duration, Utc};
use entity::users;
use jsonwebtoken::{Header, encode};
use oxy_auth::session_key::{self, Purpose};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use url::Url;

use oxy::database::client::establish_connection;
use oxy_auth::constants::SESSION_COOKIE_NAME;

use super::dto::*;

/// The session window: how long a minted JWT stays valid, and how long the
/// cookie carrying it lives.
///
/// ONE constant, read by both `create_auth_token` and [`build_session_cookie`],
/// because the two drifting apart is a real bug rather than an untidiness: a
/// cookie outliving its JWT leaves the browser holding a dead credential and
/// 401ing on every call instead of re-prompting, which is exactly what
/// `frontline.rs` hit when the shift TTL was enforced by the JWT `exp` alone.
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// Mint a session token with the default thirty-day window.
///
/// The window is [`SESSION_TTL_SECS`], shared with the cookie that
/// wraps this JWT so the two cannot drift.
pub async fn create_auth_token(user: users::Model) -> Result<String, StatusCode> {
    create_auth_token_with_ttl(user, Duration::seconds(SESSION_TTL_SECS)).await
}

/// Mint a session token with an explicit lifetime.
///
/// Split out for frontline sign-in, which gets twelve hours rather than the
/// thirty-day default:
/// that credential was proved by four digits typed on a shared tablet, and it
/// should not outlive the shift. Sharing the minting path rather than writing a
/// second one keeps there being exactly one place a session is created.
pub async fn create_auth_token_with_ttl(
    user: users::Model,
    ttl: Duration,
) -> Result<String, StatusCode> {
    let connection = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish database connection: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let user_clone = user.clone();
    let mut user_update: users::ActiveModel = user.into();
    user_update.last_login_at = Set(chrono::Utc::now().into());
    user_update.update(&connection).await.map_err(|e| {
        tracing::error!("Failed to update user last login: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let now = Utc::now();
    let exp = now + ttl;

    let claims = Claims {
        sub: user_clone.id.to_string(),
        // The ADDRESS, not a display label. `sub` is what resolves the session
        // now (see `BuiltInAuthenticator::validate`), so this claim is only
        // read by older code paths and by a human reading a decoded token —
        // and a label there would put a non-unique `name` in a field every
        // reader takes for an email. Empty for a frontline worker, which is
        // true rather than convenient.
        email: user_clone.email.clone().unwrap_or_default(),
        exp: exp.timestamp() as usize,
        iat: now.timestamp() as usize,
    };

    let key = session_key::encoding_key(Purpose::Session)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let token = encode(&Header::default(), &claims, &key).map_err(|e| {
        tracing::error!("Failed to generate JWT token: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(token)
}

/// Build the `Set-Cookie` header value for the session cookie that wraps
/// the existing JWT. Carrying the JWT in a `.oxygen-hq.com`-scoped cookie lets
/// the external auth proxy (`/api/auth/check`) authenticate browser
/// traffic on `<app>.oxygen-hq.com` subdomains using the same credential the
/// web-app already uses on the `Authorization` header.
///
/// `Domain` is configurable via the `OXY_SESSION_COOKIE_DOMAIN` env var
/// (set to `.oxygen-hq.com` in prod). When unset, the cookie is host-only —
/// fine for local dev where there are no subdomains to gate.
pub fn build_session_cookie(jwt: &str, secure: bool) -> String {
    build_session_cookie_with_max_age(jwt, secure, SESSION_TTL_SECS)
}

/// As [`build_session_cookie`], with the browser's copy expiring alongside the
/// token rather than on the default schedule.
///
/// The two lifetimes have to agree. A cookie that outlives its JWT leaves the
/// client presenting a credential the server will refuse — which does not read
/// as "signed out", it reads as every request failing while the UI still thinks
/// there is a session. That is the whole difference between a kiosk showing the
/// name picker the next morning and one showing 401s.
pub fn build_session_cookie_with_max_age(jwt: &str, secure: bool, max_age_secs: i64) -> String {
    render_session_cookie(&session_cookie_attributes(secure), jwt, max_age_secs)
}

/// The attributes of every `oxy_session` cookie this server sets. One value,
/// read by [`build_session_cookie_with_max_age`] and by the mobile sign-in,
/// which hands them to a native app to set the same cookie in its WebView —
/// so the two cookies cannot drift apart.
pub struct SessionCookieAttributes {
    pub name: &'static str,
    /// `None`: host-only (no `OXY_SESSION_COOKIE_DOMAIN`).
    pub domain: Option<String>,
    pub path: &'static str,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: &'static str,
}

/// The session cookie's attributes, with `Domain` from
/// `OXY_SESSION_COOKIE_DOMAIN`. `secure` is [`is_request_secure`]'s answer.
pub fn session_cookie_attributes(secure: bool) -> SessionCookieAttributes {
    let domain = std::env::var("OXY_SESSION_COOKIE_DOMAIN")
        .ok()
        .map(|domain| domain.trim().to_string())
        .filter(|domain| !domain.is_empty());
    SessionCookieAttributes {
        name: SESSION_COOKIE_NAME,
        domain,
        path: "/",
        secure,
        http_only: true,
        same_site: "Lax",
    }
}

/// The `Set-Cookie` value carrying `jwt` with `attrs`.
pub fn render_session_cookie(
    attrs: &SessionCookieAttributes,
    jwt: &str,
    max_age_secs: i64,
) -> String {
    let mut parts = vec![
        format!("{}={jwt}", attrs.name),
        format!("Path={}", attrs.path),
        format!("Max-Age={max_age_secs}"),
    ];
    if attrs.http_only {
        parts.push("HttpOnly".to_string());
    }
    parts.push(format!("SameSite={}", attrs.same_site));
    if attrs.secure {
        parts.push("Secure".to_string());
    }
    if let Some(domain) = &attrs.domain {
        parts.push(format!("Domain={domain}"));
    }
    parts.join("; ")
}

/// Build the `Set-Cookie` header that clears the session cookie. Sent on
/// logout. Must mirror the same `Domain` and `Path` as `build_session_cookie`
/// so the browser actually overwrites the existing cookie.
pub fn clear_session_cookie() -> String {
    let mut parts = vec![
        format!("{SESSION_COOKIE_NAME}="),
        "Path=/".to_string(),
        "Max-Age=0".to_string(),
        "HttpOnly".to_string(),
        "SameSite=Lax".to_string(),
    ];
    if let Ok(domain) = std::env::var("OXY_SESSION_COOKIE_DOMAIN") {
        let domain = domain.trim();
        if !domain.is_empty() {
            parts.push(format!("Domain={domain}"));
        }
    }
    parts.join("; ")
}

/// Should the session cookie carry the `Secure` attribute?
///
/// Three signals are consulted, in priority order:
///
/// 1. `OXY_SESSION_COOKIE_FORCE_SECURE=1` env var — explicit override.
/// 2. `OXY_SESSION_COOKIE_DOMAIN` set to a non-localhost value — if you're
///    scoping to a real domain (e.g. `.oxygen-hq.com`) the cookie must be Secure;
///    browsers ignore domain-scoped cookies on plain HTTP anyway.
/// 3. The `X-Forwarded-Proto` request header (set by the ingress) — `https`
///    means the original client request was HTTPS.
///
/// In local dev none of these are set so we default to `false`, allowing the
/// cookie to work on `http://localhost`.
///
/// `pub` so every path that mints a session answers this the same way. It is not
/// derivable from the serve mode: a dev box is cloud mode with non-prod secrets
/// and is served over plain `http://localhost`, so `!process_is_local()` sets
/// `Secure` there and the browser silently discards the cookie — a sign-in that
/// returns 200 and does not stick.
pub fn is_request_secure(headers: &HeaderMap) -> bool {
    if std::env::var("OXY_SESSION_COOKIE_FORCE_SECURE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return true;
    }
    // Auto-enable Secure when a real domain scope is configured.
    if let Ok(domain) = std::env::var("OXY_SESSION_COOKIE_DOMAIN") {
        let domain = domain.trim().to_ascii_lowercase();
        if !domain.is_empty() && !domain.contains("localhost") && !domain.contains("127.0.0.1") {
            return true;
        }
    }
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

/// Parse a `return_to` URL into its lowercase scheme and host. Rejects any
/// scheme other than http/https (mailto:, javascript:, file:, …) and
/// anything malformed. http:// is allowed at parse time so local dev
/// (`http://localhost:3000/customer-apps/...`) works; production gating still
/// happens via [`host_in_session_zone`] which only matches the configured
/// cookie zone.
fn parse_return_to_host(url: &str) -> Option<(String, String)> {
    let parsed = Url::parse(url).ok()?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let host = parsed.host_str().map(|h| h.to_ascii_lowercase())?;
    Some((scheme, host))
}

/// Static check: is `host` inside the configured session-cookie zone? The
/// session-cookie domain doubles as the trust boundary — anything that
/// shares the cookie also shares the trust.
fn host_in_session_zone(host: &str) -> bool {
    let Ok(zone) = std::env::var("OXY_SESSION_COOKIE_DOMAIN") else {
        return false;
    };
    let zone = zone.trim().trim_start_matches('.').to_ascii_lowercase();
    if zone.is_empty() {
        return false;
    }
    host == zone || host.ends_with(&format!(".{zone}"))
}

/// Check used by the magic-link request flow and by the standalone
/// validator endpoint. Allows any URL on a subdomain of the configured
/// session-cookie zone. A localhost escape hatch exists for local dev but
/// is *explicitly opt-in* via `OXY_AUTH_ALLOW_LOCALHOST_RETURN=1` — it
/// must NOT silently activate just because `OXY_SESSION_COOKIE_DOMAIN`
/// happens to be unset on a misconfigured production deploy, since that
/// would turn `?return_to=http://localhost/...` into an open redirect.
pub fn validate_return_to_url(url: &str) -> bool {
    let Some((scheme, host)) = parse_return_to_host(url) else {
        return false;
    };
    if host_in_session_zone(&host) {
        return true;
    }
    if (host == "localhost" || host == "127.0.0.1")
        && (scheme == "http" || scheme == "https")
        && allow_localhost_return()
    {
        return true;
    }
    false
}

fn allow_localhost_return() -> bool {
    std::env::var("OXY_AUTH_ALLOW_LOCALHOST_RETURN")
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// Build the `(HeaderMap, Json<AuthResponse>)` tuple returned by every
/// successful login finalize. Centralized so all four providers
/// (Google/GitHub/Okta/magic-link) emit the cookie identically.
pub fn login_response(
    request_headers: &HeaderMap,
    token: String,
    user: UserInfo,
    orgs: Vec<OrgInfo>,
) -> (HeaderMap, Json<AuthResponse>) {
    let mut response_headers = HeaderMap::new();
    let cookie = build_session_cookie(&token, is_request_secure(request_headers));
    if let Ok(value) = cookie.parse() {
        response_headers.insert(axum::http::header::SET_COOKIE, value);
    } else {
        tracing::error!("Failed to build session cookie header value");
    }
    (response_headers, Json(AuthResponse { token, user, orgs }))
}

/// Build the auth token, the `UserInfo` payload, and the user's org memberships.
///
/// Called after every login (Google, Okta, GitHub, magic link verify). Role
/// and admin status are per-org and appear on `OrgInfo` below — not on
/// `UserInfo` — so that callers never have to reason about a global role.
pub async fn finalize_login(
    user: users::Model,
    connection: &DatabaseConnection,
) -> Result<(String, UserInfo, Vec<OrgInfo>), StatusCode> {
    let token = create_auth_token(user.clone()).await.map_err(|e| {
        tracing::error!("Failed to create auth token: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    // A login is a browser session being born: no key or token is involved.
    let caller = crate::server::authz::Caller::without_credential(
        user.id,
        user.email.as_deref().unwrap_or(""),
    );
    let (user_info, orgs) = session_payload(&user, &caller, connection).await?;
    Ok((token, user_info, orgs))
}

/// What a session's response says about who it is: the profile, and the orgs
/// `caller` reaches. For a login that is every org the user belongs to; for a
/// session a token opened (`super::token_session`) it is the orgs the token
/// touches, so the web app lands somewhere the session can actually read.
pub(super) async fn session_payload(
    user: &users::Model,
    caller: &crate::server::authz::Caller,
    connection: &DatabaseConnection,
) -> Result<(UserInfo, Vec<OrgInfo>), StatusCode> {
    // Reported on the payload, not decided here — the flag door, not a ring.
    let standing = crate::server::authz::globals::platform_standing(connection, caller).await;
    let user_info = UserInfo {
        id: user.id.to_string(),
        email: user.label().to_string(),
        name: user.name.clone(),
        picture: user.picture.clone(),
        is_owner: standing.is_global_owner,
        is_app_admin: standing.is_global_admin,
    };

    use entity::org_members::{self, Entity as OrgMembers};
    use entity::organizations::{self, Entity as Organizations};

    let memberships: Vec<org_members::Model> = OrgMembers::find()
        .filter(org_members::Column::UserId.eq(user.id))
        .all(connection)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query org memberships: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .into_iter()
        .filter(|m| caller.touches_org(m.org_id))
        .collect();

    let org_ids: Vec<uuid::Uuid> = memberships.iter().map(|m| m.org_id).collect();
    let orgs = if org_ids.is_empty() {
        vec![]
    } else {
        let org_rows = Organizations::find()
            .filter(organizations::Column::Id.is_in(org_ids))
            .all(connection)
            .await
            .map_err(|e| {
                tracing::error!("Failed to query organizations: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        org_rows
            .iter()
            .filter_map(|org| {
                let membership = memberships.iter().find(|m| m.org_id == org.id)?;
                Some(OrgInfo {
                    id: org.id.to_string(),
                    name: org.name.clone(),
                    slug: org.slug.clone(),
                    role: membership.role.as_str().to_string(),
                })
            })
            .collect()
    };

    Ok((user_info, orgs))
}

/// The origin the request's `Origin` (else `Referer`) names, pinned off an org
/// subdomain. It believes the request, so it is never the base of a link an
/// anonymous request makes Oxygen mail: that is
/// `anonymous_link::link_base_for_anonymous_request`.
pub fn extract_base_url_from_headers(headers: &HeaderMap) -> String {
    pin_org_subdomain_to_app_host(extract_base_url_raw(headers))
}

/// The fallback base when nothing better is known: a laptop's `oxy serve` on
/// its default port.
pub const LOCAL_DEFAULT_BASE: &str = "http://localhost:3000";

fn extract_base_url_raw(headers: &HeaderMap) -> String {
    origin_named_by_request(headers).unwrap_or_else(|| LOCAL_DEFAULT_BASE.to_string())
}

/// `scheme://host[:port]` as the request's `Origin` header states it, else as
/// its `Referer` does; `None` when it carries neither. What the request says,
/// unchecked: any client that is not a browser can send either header.
pub fn origin_named_by_request(headers: &HeaderMap) -> Option<String> {
    if let Some(origin) = headers.get("origin").and_then(|h| h.to_str().ok()) {
        let origin = origin.trim_end_matches('/');
        if origin.starts_with("http://") || origin.starts_with("https://") {
            return Some(origin.to_string());
        }
        // Some reverse proxies/CDNs may forward Origin without scheme.
        // Default to https for non-localhost hosts.
        let scheme = if origin.starts_with("localhost") || origin.starts_with("127.0.0.1") {
            "http"
        } else {
            "https"
        };
        return Some(format!("{scheme}://{origin}"));
    }

    if let Some(referer) = headers.get("referer").and_then(|h| h.to_str().ok())
        && let Ok(url) = Url::parse(referer)
        && let Some(host) = url.host_str()
    {
        let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
        return Some(format!("{}://{}{}", url.scheme(), host, port));
    }

    None
}

/// The origin for a link an AUTHENTICATED caller causes to be sent — an
/// invitation, an org's Owner invite. As [`extract_base_url_from_headers`],
/// plus one step before its localhost default: a request with no `Origin` and
/// no `Referer` (`oxyc`, a script) gets `Host` and the proxy's scheme, as the
/// frontline enrol link has always composed it. An invitation `oxyc` created
/// on staging used to email `http://localhost:3000/invite/…`.
///
/// Authenticated callers only, deliberately. `Host` is attacker-supplied on an
/// anonymous request, so the magic-link path must not use this: a forged
/// `Host` would mail someone else's sign-in link to the forger's server. Here
/// the only person who can forge it is the one sending their own invitation.
///
/// A loopback `X-Forwarded-Host` wins over `Host`, and no other does. The Vite
/// dev proxy rewrites `Host` to its upstream (`changeOrigin`) and says in that
/// header where the caller really addressed; without it, a native app talking
/// to a local stack through Vite gets links on the backend's port, which its
/// cookie and bearer don't cover. Any other value is ignored: these links are
/// mailed to someone else (the invitee), and production's load balancer passes
/// a client's `X-Forwarded-Host` through untouched, so believing it would let
/// an Owner have Oxy mail a real invitation pointing at any host they named.
/// A loopback link reaches only the recipient's own machine.
pub fn extract_link_base_for_authenticated_request(headers: &HeaderMap) -> String {
    let browser_told_us = headers.contains_key("origin") || headers.contains_key("referer");
    let named = |name: &str| {
        headers
            .get(name)
            .and_then(|h| h.to_str().ok())
            .map(|h| h.split(',').next().unwrap_or(h).trim())
            .filter(|h| !h.is_empty())
    };
    let host = named("x-forwarded-host")
        .filter(|h| is_bare_loopback_authority(h))
        .or_else(|| named("host"));
    let (false, Some(host)) = (browser_told_us, host) else {
        return extract_base_url_from_headers(headers);
    };
    let scheme = match headers
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
    {
        Some(p) if p.eq_ignore_ascii_case("http") => "http",
        Some(_) => "https",
        None if is_request_secure(headers) => "https",
        None if host.starts_with("localhost") || host.starts_with("127.0.0.1") => "http",
        None => "https",
    };
    pin_org_subdomain_to_app_host(format!("{scheme}://{host}"))
}

/// A loopback `host[:port]` and nothing else. The value is pasted into a link
/// verbatim, so it must parse to the same host for a browser as for the
/// loopback check: `evil.com?.localhost` ends in `.localhost`, and
/// `localhost:@evil.com` starts with `localhost`, yet a browser takes both to
/// `evil.com`. Only hostname/IP characters, a bracketed IPv6 literal and a
/// numeric port are accepted.
fn is_bare_loopback_authority(value: &str) -> bool {
    let plain = value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']'));
    // What follows the host: nothing, or `:` and a port. An IPv6 literal's
    // colons sit inside its brackets.
    let after_host = value.rfind(']').map_or(value, |close| &value[close + 1..]);
    let port_ok = match after_host.split_once(':') {
        None => true,
        Some((_, port)) => !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()),
    };
    let Ok(authority) = value.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    plain && port_ok && crate::server::router::is_loopback_host(authority.host())
}

#[cfg(test)]
mod base_url_tests {
    use super::{extract_base_url_raw, extract_link_base_for_authenticated_request};
    use axum::http::HeaderMap;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn a_cli_request_links_to_the_host_it_addressed() {
        let h = headers(&[
            ("host", "aip.staging.oxy.tech"),
            ("x-forwarded-proto", "https"),
        ]);
        assert_eq!(
            extract_link_base_for_authenticated_request(&h),
            "https://aip.staging.oxy.tech"
        );
    }

    #[test]
    fn a_proxy_that_rewrote_host_is_believed_about_the_original() {
        // Vite's dev proxy: `changeOrigin` sets Host to the backend, `xfwd`
        // says what the caller addressed.
        let h = headers(&[
            ("host", "localhost:3100"),
            ("x-forwarded-host", "127.0.0.1:5273"),
            ("x-forwarded-proto", "http"),
        ]);
        assert_eq!(
            extract_link_base_for_authenticated_request(&h),
            "http://127.0.0.1:5273"
        );
    }

    #[test]
    fn a_forwarded_host_that_is_not_loopback_never_reaches_an_invitation_link() {
        // An Owner's `oxyc api -X POST …/invitations -H 'X-Forwarded-Host: …'`:
        // the load balancer passes the header through, and the link is mailed
        // to the invitee.
        let h = headers(&[
            ("host", "aip.staging.oxy.tech"),
            ("x-forwarded-host", "oxygen-login.evil.example"),
            ("x-forwarded-proto", "https"),
        ]);
        assert_eq!(
            extract_link_base_for_authenticated_request(&h),
            "https://aip.staging.oxy.tech"
        );
    }

    #[test]
    fn a_forwarded_host_that_only_looks_loopback_is_ignored() {
        // Each passes a naive prefix or suffix check, and a browser reads each
        // as another host (or a broken URL).
        for forged in [
            "evil.example?.localhost",
            "evil.example#.localhost",
            "localhost:@evil.example",
            "localhost@evil.example",
            "127.0.0.1:5273/evil.example",
            "localhost:evil.example",
            "localhost:",
            "::1",
        ] {
            let h = headers(&[
                ("host", "aip.staging.oxy.tech"),
                ("x-forwarded-host", forged),
                ("x-forwarded-proto", "https"),
            ]);
            assert_eq!(
                extract_link_base_for_authenticated_request(&h),
                "https://aip.staging.oxy.tech",
                "{forged}"
            );
        }
    }

    #[test]
    fn a_bare_loopback_forwarded_host_is_believed() {
        for local in [
            "127.0.0.1:5273",
            "localhost:5173",
            "localhost",
            "[::1]:3000",
        ] {
            let h = headers(&[
                ("host", "localhost:3100"),
                ("x-forwarded-host", local),
                ("x-forwarded-proto", "http"),
            ]);
            assert_eq!(
                extract_link_base_for_authenticated_request(&h),
                format!("http://{local}"),
                "{local}"
            );
        }
    }

    #[test]
    fn a_local_host_without_a_proxy_stays_http() {
        let h = headers(&[("host", "localhost:3000")]);
        assert_eq!(
            extract_link_base_for_authenticated_request(&h),
            "http://localhost:3000"
        );
    }

    #[test]
    fn origin_still_wins_over_host() {
        let h = headers(&[
            ("origin", "https://app.oxygen-hq.com"),
            ("host", "internal:3000"),
        ]);
        assert_eq!(extract_base_url_raw(&h), "https://app.oxygen-hq.com");
    }

    #[test]
    fn the_anonymous_helper_never_trusts_host() {
        // The magic-link path: a forged Host must not reach the emailed link.
        let h = headers(&[("host", "evil.example"), ("x-forwarded-proto", "https")]);
        assert_eq!(extract_base_url_raw(&h), "http://localhost:3000");
    }

    #[test]
    fn no_headers_at_all_keeps_the_dev_default() {
        assert_eq!(
            extract_base_url_raw(&HeaderMap::new()),
            "http://localhost:3000"
        );
    }
}

/// Centralized auth: OAuth callbacks and magic-link emails must resolve to a
/// single host (the app host) so providers need exactly one registered
/// callback. With the centralized-auth bounce in place, an auth request
/// already arrives on the app host — but if one ever lands on a bare org
/// subdomain (`pokehouse.oxygen-hq.com`), pin the base URL back to the app
/// host rather than echoing the subdomain. Non-subdomain hosts (app host,
/// custom-branded, localhost) pass through unchanged.
pub fn pin_org_subdomain_to_app_host(base: String) -> String {
    let Ok(url) = Url::parse(&base) else {
        return base;
    };
    let Some(host) = url.host_str() else {
        return base;
    };
    if oxy_app_core::org_host_dispatch::parse_org_subdomain(host).is_some()
        && let Some(app_host) = oxy_app_core::custom_apps_host_dispatch::admin_base_url()
    {
        return app_host;
    }
    base
}
