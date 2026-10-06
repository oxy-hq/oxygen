//! The handler-level refusal of a **sandbox agent token** (`oxy_sbx_`) on a
//! write to an app's production or staging state (sandbox agent credential
//! design, decision 6 and §3.3, carried to every such write).
//!
//! A sandbox agent token is its minter, who is Oxy staff, seen through the
//! narrowest credential there is. The staff console's two platform doors
//! admit it — it must pass them to reach its own sandbox routes — and the
//! console's scope check is by org, not by app. So what keeps the token from
//! promoting, rolling back, unpublishing or deleting an app in a granted
//! app's org is the route allow-list (`middlewares::app_grant_scope`) and
//! nothing else: none of those operations reaches `oxy-authz` with an
//! environment, so the model never sees them.
//!
//! [`RefuseSandboxAgent`] is the second refusal. A handler that writes state
//! a sandbox is not — a channel pointer, the app row, its audience, its
//! publishers, its storage — takes it as an argument, and the token is
//! answered `403 sandbox_token_refused` before the handler runs. It travels
//! with the handler wherever the handler is mounted, and shares no code with
//! the allow-list: it reads the credential the request authenticated with and
//! nothing about the route.
//!
//! **Keyed on the credential kind alone.** A session, a legacy API key, a
//! personal token, a service-account token, a `ci` token and a publish token
//! pass through untouched, as does a request nothing authenticated: each
//! handler's own checks decide those exactly as before.

use axum::Json;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use oxy_auth::token::CredentialContext;
use oxy_auth::types::AuthenticatedUser;

/// The refusal's machine-readable code: the one a channel publish answers.
pub const CODE: &str = "sandbox_token_refused";

const MESSAGE: &str = "a sandbox agent token writes only to a sandbox it created: this changes \
                       an app's production or staging state";

/// Every route whose handler takes [`RefuseSandboxAgent`], as `(method, path
/// under /api)`: the staff console's writes to state a sandbox is not. The
/// `/admin` rows mount the same handlers as the `/customer-apps` ones.
///
/// Two tests read this table. The route-catalog walk
/// (`route-catalog/src/tests/sandbox_agent_refusal.rs`) fails when the
/// console gains a write that is neither here nor in [`ENVIRONMENT_DECIDED`],
/// and `tests/custom_apps/sandbox_agent_token/second_refusal_writes.rs` sends
/// the token to every row with no allow-list in front.
pub const REFUSED_WRITES: &[(&str, &str)] = &[
    ("POST", "/customer-apps"),
    ("PATCH", "/customer-apps/{id}"),
    ("DELETE", "/customer-apps/{id}"),
    ("POST", "/customer-apps/{id}/publish"),
    ("DELETE", "/customer-apps/{id}/publish"),
    ("POST", "/customer-apps/{id}/rollback"),
    ("POST", "/customer-apps/batch/publish"),
    ("POST", "/customer-apps/batch/promote-latest"),
    ("POST", "/customer-apps/batch/unpublish"),
    ("POST", "/customer-apps/batch/delete"),
    ("POST", "/customer-apps/storage/sweep"),
    ("POST", "/customer-apps/{id}/storage/delete"),
    ("POST", "/customer-apps/{id}/publishers"),
    ("DELETE", "/customer-apps/{id}/publishers/{publisher_id}"),
    ("POST", "/admin/apps"),
    ("PATCH", "/admin/apps/{id}"),
    ("DELETE", "/admin/apps/{id}"),
    ("POST", "/admin/apps/{id}/publish"),
    ("DELETE", "/admin/apps/{id}/publish"),
    ("PUT", "/admin/apps/{id}/access"),
    ("POST", "/admin/app-publish-tokens"),
    ("POST", "/admin/app-publish-tokens/{id}/revoke"),
];

/// The console's other writes: the sandbox loop's own. Each handler decides
/// by the environment the request names, through `oxy-authz`
/// (`may_open_environment`) and its own ownership check, and refuses the
/// token anywhere but a sandbox it created — production and staging included
/// — so it takes no [`RefuseSandboxAgent`]. `(method, path, what decides)`.
pub const ENVIRONMENT_DECIDED: &[(&str, &str, &str)] = &[
    (
        "POST",
        "/customer-apps/{id}/environments",
        "E2: sandboxes::handlers admit",
    ),
    (
        "DELETE",
        "/customer-apps/{id}/environments/{name}",
        "E3: sandboxes::handlers admit, then the row lock",
    ),
    (
        "POST",
        "/customer-apps/publish",
        "P1: publish_to refuses a channel",
    ),
    (
        "POST",
        "/customer-apps/{id}/functions/{name}/runs",
        "C2: run_function_job refuses production",
    ),
    (
        "POST",
        "/customer-apps/{id}/secrets",
        "S1: secrets::agent authorize",
    ),
    (
        "DELETE",
        "/customer-apps/{id}/secrets/{key}",
        "S2: secrets::agent authorize",
    ),
    (
        "POST",
        "/admin/apps/{id}/functions/{name}/runs",
        "C2's handler, mounted on the admin surface",
    ),
];

/// Taken by a handler that writes production or staging state of a custom
/// app. Extracting it refuses a sandbox agent token; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefuseSandboxAgent;

/// Whether the request authenticated with a sandbox agent token: the
/// credential on the user, or the one the auth layer left beside it.
fn is_sandbox_agent(parts: &Parts) -> bool {
    let on_user = parts
        .extensions
        .get::<AuthenticatedUser>()
        .and_then(|user| user.credential.as_ref());
    let beside = parts.extensions.get::<CredentialContext>();
    on_user
        .into_iter()
        .chain(beside)
        .any(CredentialContext::is_sandbox_agent)
}

/// `403 {code, error, message}`: the body a refused channel publish has.
pub fn refusal() -> Response {
    let body = serde_json::json!({ "code": CODE, "error": CODE, "message": MESSAGE });
    (StatusCode::FORBIDDEN, Json(body)).into_response()
}

impl<S: Send + Sync> FromRequestParts<S> for RefuseSandboxAgent {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if is_sandbox_agent(parts) {
            Err(refusal())
        } else {
            Ok(Self)
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Request;
    use oxy_auth::token::StoredKind;
    use uuid::Uuid;

    use super::*;
    use crate::server::api::custom_apps_agent_fixture as fixture;

    fn parts() -> Parts {
        Request::new(()).into_parts().0
    }

    fn user(credential: Option<CredentialContext>) -> AuthenticatedUser {
        fixture::user(credential)
    }

    fn token(kind: StoredKind) -> CredentialContext {
        let mut credential =
            fixture::credential(Uuid::from_u128(0x70), Uuid::nil(), Uuid::nil(), Uuid::nil());
        credential.kind = kind;
        credential
    }

    async fn extracted(mut parts: Parts) -> Result<RefuseSandboxAgent, Response> {
        RefuseSandboxAgent::from_request_parts(&mut parts, &()).await
    }

    /// The token is refused wherever the auth layer left its credential: on
    /// the user, beside the user, or both.
    #[tokio::test]
    async fn a_sandbox_agent_token_is_refused_with_the_code() {
        let agent = token(StoredKind::SandboxAgent);
        let mut on_user = parts();
        on_user.extensions.insert(user(Some(agent.clone())));
        let mut beside = parts();
        beside.extensions.insert(user(None));
        beside.extensions.insert(agent);
        for parts in [on_user, beside] {
            let refused = extracted(parts).await.expect_err("refused");
            assert_eq!(refused.status(), StatusCode::FORBIDDEN);
            let bytes = axum::body::to_bytes(refused.into_body(), 4096)
                .await
                .expect("body");
            let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
            assert_eq!(body["code"], "sandbox_token_refused");
            assert_eq!(body["error"], "sandbox_token_refused");
        }
    }

    /// Every other credential, a session, and a request nothing authenticated
    /// pass through: the refusal is keyed on this kind and no other.
    #[tokio::test]
    async fn no_other_credential_is_touched() {
        assert!(extracted(parts()).await.is_ok(), "nothing authenticated");
        let mut session = parts();
        session.extensions.insert(user(None));
        assert!(extracted(session).await.is_ok(), "a browser session");
        for kind in [
            StoredKind::Personal,
            StoredKind::LegacyKey,
            StoredKind::ServiceAccount,
            StoredKind::Ci,
        ] {
            let mut parts = parts();
            parts.extensions.insert(user(Some(token(kind))));
            parts.extensions.insert(token(kind));
            assert!(extracted(parts).await.is_ok(), "{kind:?}");
        }
    }
}
