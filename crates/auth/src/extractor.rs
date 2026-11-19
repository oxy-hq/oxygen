use std::marker::PhantomData;

use axum::{
    extract::{FromRequestParts, Request},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};

use crate::token::CredentialContext;
use crate::types::AuthenticatedUser;

#[derive(Clone)]
pub struct AuthenticatedUserExtractor(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for AuthenticatedUserExtractor
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let result = parts
            .extensions
            .get::<AuthenticatedUser>()
            .cloned()
            .map(AuthenticatedUserExtractor)
            .ok_or(StatusCode::UNAUTHORIZED);

        async move { result }
    }
}

/// Optional user extractor that doesn't fail if user is not authenticated
#[derive(Clone)]
pub struct OptionalUserExtractor(pub Option<AuthenticatedUser>);

impl<S> FromRequestParts<S> for OptionalUserExtractor
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let result = OptionalUserExtractor(parts.extensions.get::<AuthenticatedUser>().cloned());

        async move { Ok(result) }
    }
}

/// Like `AuthenticatedUserExtractor` but returns `None` instead of rejecting
/// when the request is unauthenticated. Handlers that want to render different
/// UI for logged-in vs logged-out users use this.
#[derive(Clone)]
pub struct OptionalAuthenticatedUser(pub Option<AuthenticatedUser>);

impl<S> FromRequestParts<S> for OptionalAuthenticatedUser
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let result =
            OptionalAuthenticatedUser(parts.extensions.get::<AuthenticatedUser>().cloned());
        async move { Ok(result) }
    }
}

/// What a credential-guarded route answers a key or token with: the body of
/// its 403. One implementor per action, so each refusal can say what it was.
pub trait SessionAction: Send + Sync + 'static {
    const REFUSAL: &'static str;
    /// A machine-readable `code` beside the message. The token-management
    /// routes carry [`SESSION_REQUIRED`]; an action that wants its body left
    /// exactly as it was carries none.
    const CODE: Option<&'static str> = None;
}

/// The `code` of a session-only refusal, per the tokens HTTP contract.
pub const SESSION_REQUIRED: &str = "session_required";

/// 403 `{"error": "<refusal>"}`, plus `"code"` when the action names one.
#[derive(Debug)]
pub struct SessionOnlyRejection {
    error: &'static str,
    code: Option<&'static str>,
}

impl SessionOnlyRejection {
    fn of<A: SessionAction>() -> Self {
        Self {
            error: A::REFUSAL,
            code: A::CODE,
        }
    }
}

impl IntoResponse for SessionOnlyRejection {
    fn into_response(self) -> Response {
        let body = match self.code {
            Some(code) => serde_json::json!({ "error": self.error, "code": code }),
            None => serde_json::json!({ "error": self.error }),
        };
        (StatusCode::FORBIDDEN, axum::Json(body)).into_response()
    }
}

/// Refuses any request a key or token authenticated — anything carrying a
/// [`CredentialContext`] — so only a browser session gets through.
///
/// A credential cannot manage credentials: a leaked token cannot keep itself
/// alive by extending itself, or mint its own successor (design §4.6).
pub struct SessionOnly<A: SessionAction>(PhantomData<A>);

impl<S: Send + Sync, A: SessionAction> FromRequestParts<S> for SessionOnly<A> {
    type Rejection = SessionOnlyRejection;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let result = if parts.extensions.get::<CredentialContext>().is_some() {
            Err(SessionOnlyRejection::of::<A>())
        } else {
            Ok(SessionOnly(PhantomData))
        };
        async move { result }
    }
}

/// Extension trait to extract authenticated user from request
pub trait RequestUserExt {
    fn user(&self) -> Option<&AuthenticatedUser>;
}

impl RequestUserExt for Request {
    fn user(&self) -> Option<&AuthenticatedUser> {
        self.extensions().get::<AuthenticatedUser>()
    }
}

#[cfg(test)]
mod session_only_tests {
    use super::*;
    use crate::token::StoredKind;
    use axum::http::Request as HttpRequest;

    struct Extend;
    impl SessionAction for Extend {
        const REFUSAL: &'static str = "extend requires a browser session";
    }

    fn parts(kind: Option<StoredKind>) -> Parts {
        let (mut parts, _) = HttpRequest::new(()).into_parts();
        if let Some(kind) = kind {
            parts.extensions.insert(CredentialContext {
                token_id: uuid::Uuid::new_v4(),
                kind,
                principal_user_id: uuid::Uuid::new_v4(),
                all_access: true,
                platform: true,
                partner: true,
                name: "k".into(),
                display_prefix: "oxy_".into(),
                blocked_orgs: Vec::new(),
                expires_at: None,
                service_account: None,
                grants: Vec::new(),
                app_publish: Vec::new(),
                app_sandbox: Vec::new(),
                app_access: None,
            });
        }
        parts
    }

    #[tokio::test]
    async fn session_only_admits_a_session_and_refuses_every_credential() {
        assert!(
            SessionOnly::<Extend>::from_request_parts(&mut parts(None), &())
                .await
                .is_ok()
        );
        for kind in [
            StoredKind::Personal,
            StoredKind::ServiceAccount,
            StoredKind::Ci,
            StoredKind::SandboxAgent,
        ] {
            let Err(err) =
                SessionOnly::<Extend>::from_request_parts(&mut parts(Some(kind)), &()).await
            else {
                panic!("{kind:?} must be refused");
            };
            assert_eq!(err.error, Extend::REFUSAL);
            assert_eq!(
                err.code, None,
                "an action that names no code keeps its body"
            );
            assert_eq!(err.into_response().status(), StatusCode::FORBIDDEN);
        }
    }

    /// An action that names a `code`, as the token-management routes do.
    struct Mint;
    impl SessionAction for Mint {
        const REFUSAL: &'static str = "minting requires a browser session";
        const CODE: Option<&'static str> = Some(SESSION_REQUIRED);
    }

    #[tokio::test]
    async fn every_token_kind_is_refused_with_the_actions_status_and_code() {
        for kind in [
            StoredKind::Personal,
            StoredKind::ServiceAccount,
            StoredKind::Ci,
            StoredKind::SandboxAgent,
        ] {
            let Err(refused) =
                SessionOnly::<Mint>::from_request_parts(&mut parts(Some(kind)), &()).await
            else {
                panic!("{kind:?} must be refused");
            };
            assert_eq!(refused.error, Mint::REFUSAL, "{kind:?}");
            assert_eq!(refused.code, Some(SESSION_REQUIRED), "{kind:?}");
            assert_eq!(refused.into_response().status(), StatusCode::FORBIDDEN);
        }
    }
}
