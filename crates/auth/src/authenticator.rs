use axum::http::{HeaderMap, StatusCode};
use std::future::Future;

use crate::token::Authenticated;
use crate::types::Identity;

pub trait Authenticator<TSource = HeaderMap> {
    type Error: std::error::Error + Into<StatusCode>;

    fn authenticate(
        &self,
        source: &TSource,
    ) -> impl Future<Output = Result<Identity, Self::Error>> + Send;

    /// [`Self::authenticate`], plus the API key or token that authenticated
    /// the request when one did (`None` for a session). `auth_middleware`
    /// attaches it as the `CredentialContext` request marker.
    fn authenticate_with_credential(
        &self,
        source: &TSource,
    ) -> impl Future<Output = Result<Authenticated, Self::Error>> + Send;
}
