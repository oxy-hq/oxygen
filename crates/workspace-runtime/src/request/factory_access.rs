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
//! - [`may_use_factory`](oxy_authz::factory::may_use_factory) — the decision:
//!   `Action::FactoryUse`: staff over the org, an owner or admin of the org, or a
//!   member holding the workspace's grant.
//! - [`gate`] — runs inside `workspace_middleware`, so every Factory route asks the
//!   decision and a customer route never does. [`FactoryAccess`] hands the same answer
//!   to the response the rail reads. **One exception, stated by the model:** on the
//!   two routes that check a compile ([`routes::COMPILE_CHECK`]) the gate also lets
//!   through a caller `Action::CompileCheck` admits
//!   ([`may_check_compile`](oxy_authz::factory::may_check_compile)) — no person
//!   the Factory refuses, and the org's service account through a CI run token (the
//!   customer CI compile gate).
//!
//! **Always enforced.** It began as part of the `apps_first_experience` flag, turned
//! on per org; that flag is gone. Every Factory route refuses a caller the decision
//! refuses with `403 factory_access_required`. Granting a member access:
//! `internal-docs/roles-and-authorization.md`.

pub mod routes;

use std::convert::Infallible;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{FromRef, FromRequestParts, OriginalUri};
use axum::http::request::Parts;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use contracts_authz::AuthzEnforcer;
use entity::workspaces;
use oxy_app_core::AppState;
use oxy_authz::factory::{may_check_compile, may_use_factory};

pub use routes::{Resolution, Surface, resolve, surface_of};

/// The `code` in the 403 body, which the web app words itself.
pub const REFUSAL_CODE: &str = "factory_access_required";

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
            "message": "Oxygen Nexus is not available to you here. It is for Oxy \
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
            "message": "Could not check access to Oxygen Nexus. Try again in a moment.",
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
/// The facts are read through `authz`, the host's enforcer (`AppState.authz`):
/// `AuthzEnforcer::request_facts` is `oxy_server_authz::request_facts`'s body.
///
/// Returns at once, reading nothing, for the external API and for any route on the
/// customer list. A route on neither list, or whose entries
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
    authz: &Arc<dyn AuthzEnforcer>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Request<Body>, Response>> + Send>> {
    Box::pin(decide(request, authz.clone()))
}

async fn decide(
    request: Request<Body>,
    authz: Arc<dyn AuthzEnforcer>,
) -> Result<Request<Body>, Response> {
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
    let checks_a_compile = routes::checks_a_compile(parts.method.as_str(), parts.uri.path());
    match authz.request_facts(&mut parts.extensions).await {
        Some(facts) if may_use_factory(&facts, workspace.id, org_id) => {
            Ok(Request::from_parts(parts, body))
        }
        // The one exception, stated by the model: on the two routes that check a
        // compile, a caller `Action::CompileCheck` admits — for a person that already
        // implies the Factory, so this adds only the org's service account through a
        // CI run token. Unknown facts still fail closed below.
        Some(facts) if checks_a_compile && may_check_compile(&facts, workspace.id, org_id) => {
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
/// workspace response tells the rail.
///
/// An unreadable standing answers `true`: the rail is not access control, the gate
/// is, and a database blip must not hide the Factory from the staff who run it.
///
/// The facts are read through the host's enforcer (`AppState.authz`), so the
/// router's state must give an `AppState`.
pub struct FactoryAccess(pub bool);

impl<S> FromRequestParts<S> for FactoryAccess
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Infallible> {
        let Some((workspace, org_id)) = workspace_of(parts) else {
            return Ok(Self(true));
        };
        let authz = AppState::from_ref(state).authz;
        Ok(Self(
            match authz.request_facts(&mut parts.extensions).await {
                Some(facts) => may_use_factory(&facts, workspace.id, org_id),
                None => true,
            },
        ))
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

    const ORG: uuid::Uuid = uuid::Uuid::from_u128(0x7);
    const WS: uuid::Uuid = uuid::Uuid::from_u128(0x71);

    fn workspace() -> workspaces::Model {
        let now = chrono::Utc::now().fixed_offset();
        workspaces::Model {
            id: WS,
            name: "ws".into(),
            git_namespace_id: None,
            git_remote_url: None,
            created_at: now,
            updated_at: now,
            path: None,
            last_opened_at: None,
            created_by: None,
            org_id: Some(ORG),
            status: workspaces::WorkspaceStatus::Ready,
            error: None,
            monthly_vlm_budget_micros: None,
            current_revision_id: None,
            default_branch: None,
            repo_subdir: None,
            default_branch_attempted_at: None,
        }
    }

    /// The org's service account through a token holding `viewer` on `WS`;
    /// `ci_run` says whether that token is a CI run's.
    fn account(ci_run: bool) -> oxy_authz::PrincipalFacts {
        use oxy_authz::{
            PrincipalFacts, RoleCeiling, ServiceAccountStanding, TokenGrant, TokenReach,
        };
        PrincipalFacts {
            service_account: Some(ServiceAccountStanding {
                org_id: ORG,
                admin: false,
            }),
            ..Default::default()
        }
        .narrowed_by(&TokenReach {
            all_access: false,
            platform: false,
            partner: false,
            grants: vec![TokenGrant {
                org_id: ORG,
                workspace_id: Some(WS),
                ceiling: RoleCeiling::Viewer,
            }],
            blocked_orgs: Vec::new(),
            sandbox_agent: None,
            app: None,
            ci_run,
        })
    }

    /// The gate's answer for `method path`, with the facts pre-inserted where
    /// `request_facts` memoizes them, so no database is read.
    async fn gate_status(
        facts: oxy_authz::PrincipalFacts,
        method: &str,
        path: &str,
    ) -> Option<StatusCode> {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(workspace());
        request.extensions_mut().insert(facts);
        // The shipped enforcer: it answers from the memoized facts above.
        decide(request, oxy_server_authz::enforcer())
            .await
            .err()
            .map(|refusal| refusal.status())
    }

    /// The gate's one exception, end to end: the CI account passes the two compile
    /// check pairs and nothing else on the Factory side — a sibling Factory route,
    /// and the same path under another method, still answer `factory_access_required`.
    #[tokio::test]
    async fn the_ci_account_passes_the_compile_check_routes_and_no_other_factory_route() {
        for (method, path) in [
            ("POST", "/compile/staging"),
            ("GET", "/compile/staging/status"),
        ] {
            assert_eq!(
                gate_status(account(true), method, path).await,
                None,
                "{method} {path}"
            );
        }
        for (method, path) in [
            ("PUT", "/secrets/abc"),
            ("POST", "/compile"),
            ("GET", "/compile/staging"),
            ("GET", "/semantic/objects"),
        ] {
            assert_eq!(
                gate_status(account(true), method, path).await,
                Some(StatusCode::FORBIDDEN),
                "{method} {path}"
            );
        }
    }

    #[tokio::test]
    async fn the_accounts_own_token_and_a_plain_member_are_refused_the_compile_check() {
        let member = oxy_authz::PrincipalFacts {
            member_orgs: vec![ORG],
            ..Default::default()
        };
        for facts in [account(false), member] {
            assert_eq!(
                gate_status(facts, "POST", "/compile/staging").await,
                Some(StatusCode::FORBIDDEN)
            );
        }
    }
}
