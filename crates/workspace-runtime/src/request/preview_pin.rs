//! The preview pin as the workspace middleware applies it, with the staff
//! check it consults ([`oxy_previews::pin::MayPreview`]).
//!
//! `oxy-previews` takes the check as a parameter, so it need not depend on the
//! authz guards; [`may_preview`] is the one implementation, the
//! `WorkspacePreviewer` decision.

use axum::http::request::Parts;
use oxy_previews::pin::PreviewPin;
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

/// `oxy_previews::pin::for_request`, consulting [`may_preview`].
pub(super) async fn for_request<B>(
    workspace_id: Uuid,
    query_branch: Option<&str>,
    request: axum::http::Request<B>,
) -> (axum::http::Request<B>, Option<PreviewPin>) {
    oxy_previews::pin::for_request(workspace_id, query_branch, request, may_preview).await
}

/// `oxy_server_authz::role_guards::may_preview`, boxed to fit
/// [`oxy_previews::pin::MayPreview`].
fn may_preview(parts: &mut Parts) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
    Box::pin(oxy_server_authz::role_guards::may_preview(parts))
}
