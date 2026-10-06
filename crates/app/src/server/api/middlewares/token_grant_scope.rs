//! Decides which **flat** routes an API token that reaches less than its
//! bearer may call.
//!
//! Under `/{workspace_id}/…` and `/orgs/{org_id}/…` the workspace and org
//! middlewares enforce a token's reach against the id in the path. The flat
//! routes carry no such id, and most of them answer from the user's raw
//! memberships (`/chat`, `/work`, `/notifications`, …). Two kinds of token
//! reach less than that, and this is the one place that decides for both:
//!
//! - A **grant-bound** token (`all_access = false`, API-tokens design §3.2)
//!   reaches only what its grants name. [`treatment`] names the flat routes
//!   that filter by the grants themselves or gate on a standing the credential
//!   already bounds; **everything else is refused with 404**, the answer
//!   anything outside a grant gets, so the token cannot tell a refused route
//!   from a missing one.
//! - An **all-access token an org has blocked** (the org's revoke-grant, or its
//!   token policy, §5) loses that org's data **and nothing else**. It keeps the
//!   membership-keyed routes: each one in [`WHEN_BLOCKED`] says how its handler
//!   leaves the blocked orgs out. A route that could not would be refused —
//!   for such a token only ([`Blocked::Refused`]).
//!
//! Refusal is the default for both: a flat route added later is closed to
//! them until it is listed, and `oxy-route-catalog` fails the build until it
//! is. A session, a legacy key and an all-access token no org has blocked pass
//! untouched — this never narrows a credential that exists today.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

use crate::server::authz::Caller;

/// What a flat route does with a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Treatment {
    /// The route honours the token's reach itself.
    Honoured,
    /// 404.
    Refused,
}

/// Runs inside the auth layer (it reads the credential auth attached). Paths
/// are relative to `/api`, as every layer in `api_auth_layers` sees them.
pub async fn token_grant_scope_middleware(
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let path = request.uri().path();
    let refused = match Caller::from_extensions(request.extensions()) {
        Some(caller) if caller.bound_to_grants() => {
            (treatment(path) == Treatment::Refused).then_some("grant-bound")
        }
        Some(caller) if caller.blocked_somewhere() => {
            (treatment_when_blocked(path) == Treatment::Refused).then_some("blocked by an org")
        }
        _ => None,
    };
    if let Some(token) = refused {
        tracing::warn!(
            method = %request.method(),
            path = %path,
            token,
            "token on a flat route that cannot honour its reach — 404"
        );
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(next.run(request).await)
}

/// The flat route families a grant-bound token is refused on, each with why
/// filtering it by grant is not meaningful. Adding a flat route outside both
/// this list and [`treatment`]'s honoured arms fails
/// `every_flat_route_has_a_decided_treatment` in `oxy-route-catalog`, beside
/// the generated table it walks — the decision is made here, not by a default
/// nobody looked at. Documentation for that test: [`treatment`] alone decides.
pub const REFUSED: &[(&str, &str)] = &[
    (
        "/chat",
        "org-wide human messaging, keyed by channel membership",
    ),
    ("/work", "tasks keyed by the roles the user holds"),
    ("/notifications", "the user's inbox and push devices"),
    (
        "/invitations",
        "the user's own invitations; accepting one joins an org",
    ),
    (
        "/airhouse",
        "mints a warehouse credential from raw membership",
    ),
    ("/oltp", "per-org database status from raw membership"),
    ("/user/github", "the user's own GitHub account"),
];

/// The [`REFUSED`] family `path` (relative to `/api`) belongs to, if any.
pub fn refused_family(path: &str) -> Option<&'static str> {
    REFUSED
        .iter()
        .map(|(prefix, _)| *prefix)
        .find(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")))
}

/// The treatment of `path` (relative to `/api`) for a grant-bound token.
pub fn treatment(path: &str) -> Treatment {
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match segments.as_slice() {
        // The workspace tree: `workspace_middleware` checks the grant on the
        // workspace in the path and caps the role at its ceiling.
        [workspace, ..] if Uuid::parse_str(workspace).is_ok() => Treatment::Honoured,
        // `GET /orgs` lists only the orgs a grant touches; under
        // `/orgs/{org_id}/…`, `org_middleware` checks the grant and caps the role.
        ["orgs", ..] => Treatment::Honoured,
        // Filtered to the orgs a grant touches.
        ["apps", "mine"] => Treatment::Honoured,
        // Staff standing, which the credential bounds to its grants' orgs —
        // and drops entirely without `platform`.
        ["admin", ..] | ["customer-apps", ..] | ["assume", ..] => Treatment::Honoured,
        // Partner standing, which a grant-bound token never carries.
        ["partners", ..] => Treatment::Honoured,
        // Document visibility is resolved per org through the caller's reach.
        ["documents", ..] | ["document-categories"] | ["document-folders"] => Treatment::Honoured,
        // Token management is session-only, and answers 403 `session_required`
        // rather than 404. `/auth/token` is the calling token about itself.
        ["user", "tokens", ..] | ["user", "token-options"] | ["auth", ..] => Treatment::Honoured,
        // No tenant data: the route table, and ending a session.
        ["_catalog"] | ["logout"] => Treatment::Honoured,
        _ => Treatment::Refused,
    }
}

/// What an all-access token gets on a membership-keyed route once an org has
/// blocked it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// The handler leaves the blocked orgs out: a list omits their rows, and a
    /// request that names one of them — by an id in the path, the body or the
    /// query — is 404, as for anything else the token does not reach
    /// (`AuthenticatedUser::{reaches_org, require_org_reach, blocked_orgs}`).
    LeftOut,
    /// The route reads and writes nothing of any org's.
    NoOrgData,
    /// The route cannot leave one org out: 404, for a token an org has blocked
    /// and for no other. None today.
    Refused,
}

/// Every route of the [`REFUSED`] families, as the route catalog spells it
/// (relative to `/api`), with what a blocked all-access token gets there and
/// how. A route added to one of those families fails
/// `every_membership_keyed_route_decides_for_a_blocked_token` in
/// `oxy-route-catalog` until it is listed here — and is refused to such a
/// token until then.
pub const WHEN_BLOCKED: &[(&str, Blocked, &str)] = &[
    (
        "/chat/channels",
        Blocked::LeftOut,
        "lists no channel of a blocked org; creating one there is 404",
    ),
    (
        "/chat/channels/{id}/join",
        Blocked::LeftOut,
        "404 for a blocked org's channel",
    ),
    (
        "/chat/channels/{id}/messages",
        Blocked::LeftOut,
        "404 for a blocked org's channel",
    ),
    (
        "/chat/channels/{id}/read",
        Blocked::LeftOut,
        "404 for a blocked org's channel",
    ),
    (
        "/chat/channels/{id}/stream",
        Blocked::LeftOut,
        "404 for a blocked org's channel, decided when the stream opens",
    ),
    (
        "/work",
        Blocked::LeftOut,
        "lists no item of a blocked org; creating one there is 404",
    ),
    (
        "/work/{id}",
        Blocked::LeftOut,
        "404 for a blocked org's item",
    ),
    (
        "/notifications",
        Blocked::LeftOut,
        "neither lists nor counts a blocked org's notifications",
    ),
    (
        "/notifications/read-all",
        Blocked::LeftOut,
        "leaves a blocked org's notifications unread",
    ),
    (
        "/notifications/{id}/read",
        Blocked::LeftOut,
        "404 for a blocked org's notification",
    ),
    (
        // A push carries no payload: the device is woken and reads the inbox,
        // which is filtered. A push that carried content would have to refuse.
        "/notifications/devices",
        Blocked::NoOrgData,
        "registers the user's own device",
    ),
    (
        "/notifications/vapid-public-key",
        Blocked::NoOrgData,
        "the server's public push key",
    ),
    (
        "/invitations/mine",
        Blocked::LeftOut,
        "lists no invitation into a blocked org",
    ),
    (
        "/invitations/{token}/accept",
        Blocked::LeftOut,
        "404 for an invitation into a blocked org",
    ),
    (
        "/airhouse/version",
        Blocked::NoOrgData,
        "the Airhouse deployment's version",
    ),
    (
        "/airhouse/me/connection",
        Blocked::LeftOut,
        "404 when `workspace_id` is a blocked org's",
    ),
    (
        "/airhouse/me/credentials",
        Blocked::LeftOut,
        "404 when `workspace_id` is a blocked org's",
    ),
    (
        "/airhouse/me/provision",
        Blocked::LeftOut,
        "404 when `workspace_id` is a blocked org's",
    ),
    (
        "/airhouse/me/catalog-indexes",
        Blocked::LeftOut,
        "404 when `workspace_id` is a blocked org's",
    ),
    (
        "/airhouse/me/tokens/{username}",
        Blocked::LeftOut,
        "404 when `workspace_id` is a blocked org's",
    ),
    (
        "/oltp/me/connection",
        Blocked::LeftOut,
        "404 when `workspace_id` is a blocked org's",
    ),
    (
        "/oltp/me/erd",
        Blocked::LeftOut,
        "404 when `workspace_id` is a blocked org's",
    ),
    (
        "/user/github/account",
        Blocked::NoOrgData,
        "the user's own GitHub account",
    ),
    (
        "/user/github/installations",
        Blocked::NoOrgData,
        "the installations the user's GitHub account can see",
    ),
    (
        "/user/github/account/oauth-url",
        Blocked::LeftOut,
        "404 when `org_id` is a blocked org",
    ),
    (
        "/user/github/installations/new-url",
        Blocked::LeftOut,
        "404 when `org_id` is a blocked org",
    ),
    (
        "/user/github/callback",
        Blocked::LeftOut,
        "404 when the signed state names a blocked org",
    ),
];

/// Whether `path` is an instance of the catalog route `route`: the same
/// segments, with a `{param}` standing for any one.
fn is_route(route: &str, path: &str) -> bool {
    let mut route = route.split('/').filter(|s| !s.is_empty());
    let mut path = path.split('/').filter(|s| !s.is_empty());
    loop {
        match (route.next(), path.next()) {
            (None, None) => return true,
            (Some(want), Some(got)) if want.starts_with('{') || want == got => {}
            _ => return false,
        }
    }
}

/// What [`WHEN_BLOCKED`] says of `path` (relative to `/api`): a concrete
/// request path, or a catalog route. `None` = nobody has decided.
///
/// A route listed as written is tried first, so one with a literal segment is
/// never read as a sibling that has a `{param}` in the same place.
pub fn when_blocked(path: &str) -> Option<Blocked> {
    let exact = WHEN_BLOCKED.iter().find(|(route, ..)| *route == path);
    exact
        .or_else(|| {
            WHEN_BLOCKED
                .iter()
                .find(|(route, ..)| is_route(route, path))
        })
        .map(|(_, blocked, _)| *blocked)
}

/// The treatment of `path` (relative to `/api`) for an all-access token an
/// org has blocked. A route that honours a grant-bound token's reach honours
/// this one's by the same means; of the rest, only the ones [`WHEN_BLOCKED`]
/// lets through.
pub fn treatment_when_blocked(path: &str) -> Treatment {
    if treatment(path) == Treatment::Honoured {
        return Treatment::Honoured;
    }
    match when_blocked(path) {
        Some(Blocked::LeftOut | Blocked::NoOrgData) => Treatment::Honoured,
        Some(Blocked::Refused) | None => Treatment::Refused,
    }
}

#[cfg(test)]
#[path = "token_grant_scope_tests.rs"]
mod tests;
