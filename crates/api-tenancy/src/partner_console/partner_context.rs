//! Partner context middleware + the `PartnerActor` guard.
//!
//! Keyed on `/partners/{partner_org_id}/…`. `partner_middleware` resolves the
//! caller's [`PartnerScope`] — the partner's ceiling and its managed clients — and
//! inserts it as a request extension. A caller with no partner access in that org
//! is rejected with `403` before any handler runs.
//!
//! Every operator of a partner has the same authority (the ceiling); there are no
//! per-person roles. Capability gating is still per-handler via
//! [`PartnerActor::require`], because a capability the ceiling withholds must block
//! the specific action, not the whole route tree.

use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use std::future::Future;
use uuid::Uuid;

use oxy_server_authz::partner_authz::{PartnerCapability, PartnerScope, resolve_scope};

#[derive(serde::Deserialize)]
pub struct PartnerPath {
    partner_org_id: Uuid,
}

/// Resolve the caller's partner scope for `/partners/{partner_org_id}/…` and
/// insert it into request extensions. `403` if the caller holds no partner role
/// in that org — the cross-tenant boundary, so a partner's people can only ever
/// enter their own partner's subtree.
pub async fn partner_middleware(
    Path(PartnerPath { partner_org_id }): Path<PartnerPath>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("partner_middleware: DB connection failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Acting as a CLIENT closes the console, mirroring the way acting closes the
    // admin surface for Oxy staff: you cannot wield partner powers over a tenant
    // while wearing that tenant's identity.
    //
    // A session for THIS partner org is the opposite case — that is Oxy staff
    // acting *as* the partner, which is precisely how they reach this console. So
    // only a session pointed somewhere else locks it.
    let elsewhere = oxy_server_authz::assume_liveness::live_sessions_for(&db, user.id)
        .await
        .into_iter()
        .find(|s| s.org_id != partner_org_id);
    if let Some(session) = elsewhere {
        tracing::info!(
            actor = %user.label(),
            acting_as = %session.org_id,
            "partner_console: refused — actor is currently acting as a client"
        );
        return Err(StatusCode::FORBIDDEN);
    }

    let scope = resolve_scope(
        &db,
        partner_org_id,
        user.id,
        user.email.as_deref().unwrap_or(""),
    )
    .await
    .ok_or(StatusCode::FORBIDDEN)?;

    request.extensions_mut().insert(scope);
    Ok(next.run(request).await)
}

/// Typed guard: the caller holds *some* partner role in the org in the route.
/// Handlers destructure it to reach the scope and enforce per-action
/// capabilities:
///
/// ```ignore
/// pub async fn deploy(actor: PartnerActor) -> Result<_, StatusCode> {
///     actor.require(PartnerCapability::ManageApps)?;
///     ...
/// }
/// ```
#[derive(Debug, Clone)]
pub struct PartnerActor(pub PartnerScope);

impl PartnerActor {
    /// `Ok` if this actor's ceiling holds `cap`, else `403`. The single place a
    /// capability-only check is turned into an HTTP decision; the decision itself is
    /// made by the unified authz model.
    pub fn require(&self, cap: PartnerCapability) -> Result<(), StatusCode> {
        if oxy_server_authz::partner_allows(&self.0, None, cap) {
            Ok(())
        } else {
            Err(StatusCode::FORBIDDEN)
        }
    }

    // NOTE: there is deliberately no `require_org` here. Org-scoped decisions go
    // through ONE door — `partner_console::require_org_scope` — which also decides
    // 404-vs-403 so a partner can't probe for orgs outside their subtree. A second
    // entry point would eventually drift from it.
}

impl<S> FromRequestParts<S> for PartnerActor
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        // Missing extension = the route wasn't wrapped in `partner_middleware`
        // (a wiring bug), so 500 — matches the org role-guard convention.
        let result = parts
            .extensions
            .get::<PartnerScope>()
            .cloned()
            .map(PartnerActor)
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR);
        async move { result }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use oxy_server_authz::partner_authz::Capabilities;

    fn scope_with(manage_apps: bool) -> PartnerScope {
        PartnerScope {
            partner_id: Uuid::new_v4(),
            slug: "acme".into(),
            org_ids: Vec::new(),
            capabilities: Capabilities {
                manage_apps,
                ..Default::default()
            },
        }
    }

    fn parts_with(scope: Option<PartnerScope>) -> Parts {
        let mut req = Request::builder().body(Body::empty()).unwrap();
        if let Some(s) = scope {
            req.extensions_mut().insert(s);
        }
        req.into_parts().0
    }

    #[tokio::test]
    async fn extracts_when_scope_present() {
        let mut parts = parts_with(Some(scope_with(true)));
        assert!(
            PartnerActor::from_request_parts(&mut parts, &())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn missing_scope_is_wiring_bug_500() {
        let mut parts = parts_with(None);
        let err = PartnerActor::from_request_parts(&mut parts, &())
            .await
            .unwrap_err();
        assert_eq!(err, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn require_enforces_capability() {
        let mut parts = parts_with(Some(scope_with(true)));
        let guard = PartnerActor::from_request_parts(&mut parts, &())
            .await
            .unwrap();
        assert!(guard.require(PartnerCapability::ManageApps).is_ok());
        assert_eq!(
            guard.require(PartnerCapability::ManageBilling).unwrap_err(),
            StatusCode::FORBIDDEN
        );
    }
}
