//! The workspace the workspace middleware resolved, as a handler takes it.
//!
//! `workspace_middleware` (cloud) and `local_context_middleware` (local) leave
//! the workspace row in the request's extensions; this reads it back. A handler
//! that takes it is answered 401 on a tree those middlewares do not wrap.
//! Re-exported at `server::router::WorkspaceExtractor`, where it was defined
//! until surfaces needed it through the prelude.

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use entity::workspaces as workspace_entity;
use std::future::Future;

#[derive(Clone)]
pub struct WorkspaceExtractor(pub workspace_entity::Model);

impl<S> FromRequestParts<S> for WorkspaceExtractor
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
            .get::<workspace_entity::Model>()
            .cloned()
            .map(WorkspaceExtractor)
            .ok_or(StatusCode::UNAUTHORIZED);

        async move { result }
    }
}
