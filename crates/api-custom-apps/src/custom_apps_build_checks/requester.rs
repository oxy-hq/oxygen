//! Who asks a checks route: the request's actor and, beside it, the marker
//! the auth layer leaves for a publish token. Every checks route reads the
//! two together to make its [`Caller`], so they are one extractor.

use axum::Extension;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use oxy_app_core::audit::RequestActor;
use oxy_auth::types::AppPublishTokenAuth;

use crate::admin::apps::environment_scope::Caller;

/// The actor of a checks request and its publish-token marker.
#[derive(Clone)]
pub struct Requester {
    pub actor: RequestActor,
    /// `Some` when the request authenticated with a publish token.
    pub marker: Option<Extension<AppPublishTokenAuth>>,
}

impl Requester {
    /// The caller the checks' doors decide for (`super::admit`).
    pub(crate) fn caller(&self) -> Caller<'_> {
        Caller::new(&self.actor.user, self.marker.as_ref())
    }
}

/// The actor's own refusal (`401` before authentication has run), and the
/// marker as `Option<Extension<_>>` reads it: never a refusal.
impl<S: Send + Sync> FromRequestParts<S> for Requester {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let actor = RequestActor::from_request_parts(parts, state).await?;
        let marker = parts
            .extensions
            .get::<AppPublishTokenAuth>()
            .cloned()
            .map(Extension);
        Ok(Self { actor, marker })
    }
}
