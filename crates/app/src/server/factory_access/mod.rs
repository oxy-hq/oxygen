//! Who may use the Factory — the server-side gate.
//!
//! The Factory (the IDE and back office under a workspace's `/ide` route, and the
//! workspace routes they call) is for Oxy staff, the org's owners and admins, and
//! the members an Oxy operator granted a workspace to. A plain org membership and a
//! workspace role do not mean it. See `internal-docs/roles-and-authorization.md` § The Factory, and
//! `internal-docs/apps-first-product-plan.md` W2.
//!
//! Three parts, one decision between them:
//!
//! - [`routes`] — the list of which workspace routes are the Factory's and which are
//!   a customer's, with a test that fails when a route is on neither.
//! - [`may_use_factory`](oxy_server_authz::factory::may_use_factory) — the decision:
//!   `Action::FactoryUse`: staff over the org, an owner or admin of the org, or a
//!   member holding the workspace's grant.
//! - [`gate`] — runs inside `workspace_middleware`, so every Factory route asks the
//!   decision and a customer route never does. [`FactoryAccess`] hands the same answer
//!   to the response the rail reads.
//!
//! **Behind a switch, off by default.** Enforcement is one of the things the
//! apps-first flag ([`APPS_FIRST_EXPERIENCE`]) turns on; the admin console flips it
//! fleet-wide within the flag cache's refresh interval. Off, the
//! gate returns before it reads a route or a fact, and [`FactoryAccess`] says yes to
//! everyone: behaviour is exactly what it was. On, every Factory route refuses a caller
//! the decision refuses with `403 factory_access_required`. Operators grant the people
//! who use the Factory today before turning it on; the steps are in
//! `internal-docs/roles-and-authorization.md`.

pub mod routes;

use std::convert::Infallible;

use axum::body::Body;
use axum::extract::{FromRequestParts, OriginalUri};
use axum::http::request::Parts;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use entity::workspaces;
use oxy_server_authz::factory::may_use_factory;
use oxy_server_authz::request_facts;

pub use routes::{Resolution, Surface, resolve, surface_of};

/// The feature flag that turns enforcement on, with the rest of the apps-first
/// experience. Registered in `feature_flags::registry`, default off. It replaced
/// `factory_access_enforcement`, so there is one switch rather than two that could
/// disagree (a new home with an open Factory, or the reverse).
pub use crate::server::feature_flags::APPS_FIRST_EXPERIENCE;

/// The `code` in the 403 body, which the web app words itself.
pub const REFUSAL_CODE: &str = "factory_access_required";

/// Whether the switch is on.
pub fn enforced() -> bool {
    crate::server::feature_flags::apps_first_experience()
}

/// Whether this request arrived through the external API, a curated surface that is
/// the customer's programmatic access and so is never the Factory's.
fn is_external(parts: &Parts) -> bool {
    parts
        .extensions
        .get::<OriginalUri>()
        .is_some_and(|original| original.0.path().starts_with("/external/"))
}

/// The refusal for a caller the decision refused.
fn refused(workspace_id: uuid::Uuid) -> Response {
    (
        StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({
            "error": REFUSAL_CODE,
            "code": REFUSAL_CODE,
            "message": "The Oxygen Factory is not available to you here. It is for Oxy \
                        engineers, your company's owners and admins, and the people Oxy has \
                        named; ask Oxy if you need access.",
            "workspace_id": workspace_id,
        })),
    )
        .into_response()
}

/// The refusal for a decision that could not be made: the caller's standing is
/// unknown, which is not the same as having none.
fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(serde_json::json!({
            "error": "factory_access_unavailable",
            "code": "factory_access_unavailable",
            "message": "Could not check access to the Oxygen Factory. Try again in a moment.",
        })),
    )
        .into_response()
}

/// The workspace the request was authorized for, as `workspace_middleware`
/// attached it, with its org.
fn workspace_of(parts: &Parts) -> Option<(workspaces::Model, uuid::Uuid)> {
    let workspace = parts.extensions.get::<workspaces::Model>()?.clone();
    let org_id = workspace.org_id?;
    Some((workspace, org_id))
}

/// Refuse a Factory route to a caller who may not use the Factory here.
///
/// Called by `workspace_middleware` after it has authorized the caller for the
/// workspace and attached the workspace row. `Ok` hands the request back untouched
/// but for the caller's facts, memoized in its extensions where the role guards
/// read them (`request_facts`), so a Factory route pays for them once, not twice.
///
/// Returns at once, reading nothing, when the switch is off; and for the external API
/// and for any route on the customer list. A route on neither list, or whose entries
/// tie, is the Factory's — a table gap refuses a customer a page rather than handing
/// them the IDE.
///
/// Fails closed when the facts cannot be read (`503`), unlike the role guards, which
/// defer to the check they replaced: there is no check this one replaced, so there is
/// nothing to defer to.
///
/// Returned boxed, from a plain function, so the decision's future is built in this
/// function's frame and lives on the heap. `workspace_middleware` is polled at the
/// bottom of every workspace request's stack; in a debug build an `async fn`'s poll
/// frame reserves room for every future it awaits, and this one inline pushed a
/// ClickHouse `sql/query` past the 2 MiB worker stack in the custom-app canary.
pub fn gate(
    request: Request<Body>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Request<Body>, Response>> + Send>> {
    Box::pin(decide(request))
}

async fn decide(request: Request<Body>) -> Result<Request<Body>, Response> {
    if !enforced() {
        return Ok(request);
    }
    let (mut parts, body) = request.into_parts();
    if is_external(&parts)
        || surface_of(parts.method.as_str(), parts.uri.path()) == Surface::Customer
    {
        return Ok(Request::from_parts(parts, body));
    }
    let Some((workspace, org_id)) = workspace_of(&parts) else {
        // The middleware attaches the row before it calls this, and refuses a
        // workspace with no org before that. Reaching here is a wiring bug.
        tracing::error!(
            path = parts.uri.path(),
            "factory gate ran with no authorized workspace on the request"
        );
        return Err(unavailable());
    };
    match request_facts(&mut parts).await {
        Some(facts) if may_use_factory(&facts, workspace.id, org_id) => {
            Ok(Request::from_parts(parts, body))
        }
        Some(_) => {
            tracing::info!(
                target: "authz",
                workspace_id = %workspace.id,
                method = %parts.method,
                path = parts.uri.path(),
                "factory route refused: neither Oxy staff nor granted this workspace"
            );
            Err(refused(workspace.id))
        }
        None => Err(unavailable()),
    }
}

/// Whether the caller may use the Factory in the request's workspace — what the
/// workspace response tells the rail. `true` for everyone while the switch is off.
///
/// An unreadable standing answers `true`: the rail is not access control, the gate
/// is, and a database blip must not hide the Factory from the staff who run it.
pub struct FactoryAccess(pub bool);

impl<S: Send + Sync> FromRequestParts<S> for FactoryAccess {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Infallible> {
        if !enforced() {
            return Ok(Self(true));
        }
        let Some((workspace, org_id)) = workspace_of(parts) else {
            return Ok(Self(true));
        };
        Ok(Self(match request_facts(parts).await {
            Some(facts) => may_use_factory(&facts, workspace.id, org_id),
            None => true,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Uri;

    #[test]
    fn the_external_api_is_recognised_by_its_original_uri() {
        let external = {
            let mut request = Request::builder().uri("/ws/threads").body(()).unwrap();
            request
                .extensions_mut()
                .insert(OriginalUri(Uri::from_static("/external/api/ws/threads")));
            request.into_parts().0
        };
        let internal = {
            let mut request = Request::builder().uri("/ws/threads").body(()).unwrap();
            request
                .extensions_mut()
                .insert(OriginalUri(Uri::from_static("/api/ws/threads")));
            request.into_parts().0
        };
        let bare = Request::builder()
            .uri("/ws/threads")
            .body(())
            .unwrap()
            .into_parts()
            .0;
        assert!(is_external(&external));
        assert!(!is_external(&internal));
        assert!(!is_external(&bare));
    }

    #[test]
    fn the_refusal_names_the_code_the_web_app_keys_on() {
        let response = refused(uuid::Uuid::nil());
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(REFUSAL_CODE, "factory_access_required");
    }
}
