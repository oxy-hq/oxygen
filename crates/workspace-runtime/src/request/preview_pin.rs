//! The preview pin as the workspace middleware applies it, with the staff
//! check it consults ([`oxy_previews::pin::MayPreview`]).
//!
//! `oxy-previews` takes the check as a parameter, so it need not depend on the
//! authz guards; [`may_preview`] is the one implementation, the
//! `WorkspacePreviewer` decision, asked through the authorization port
//! (`AuthzEnforcer::may_preview`, which delegates to
//! `oxy_server_authz::role_guards::may_preview`'s body).

use axum::http::request::Parts;
use contracts_authz::AuthzEnforcer;
use oxy_previews::pin::PreviewPin;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

/// The enforcer [`may_preview`] asks, carried to it in the request's
/// extensions for the one call: [`oxy_previews::pin::MayPreview`] is a plain
/// `fn` pointer, so it cannot capture one. Private to this module, inserted by
/// [`for_request`] and removed again before the request goes on, so nothing
/// downstream can read it.
#[derive(Clone)]
struct PreviewEnforcer(Arc<dyn AuthzEnforcer>);

/// `oxy_previews::pin::for_request`, consulting [`may_preview`] through
/// `authz` (the host's enforcer, `AppState.authz`).
pub(super) async fn for_request<B>(
    workspace_id: Uuid,
    query_branch: Option<&str>,
    mut request: axum::http::Request<B>,
    authz: &Arc<dyn AuthzEnforcer>,
) -> (axum::http::Request<B>, Option<PreviewPin>) {
    request
        .extensions_mut()
        .insert(PreviewEnforcer(authz.clone()));
    let (mut request, pin) =
        oxy_previews::pin::for_request(workspace_id, query_branch, request, may_preview).await;
    request.extensions_mut().remove::<PreviewEnforcer>();
    (request, pin)
}

/// `AuthzEnforcer::may_preview` over the request's extensions, boxed to fit
/// [`oxy_previews::pin::MayPreview`]. With no enforcer on the request (a wiring
/// bug: [`for_request`] always inserts one) it answers `false`: previews fail
/// closed.
fn may_preview(parts: &mut Parts) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
    Box::pin(async move {
        let Some(PreviewEnforcer(authz)) = parts.extensions.get::<PreviewEnforcer>().cloned()
        else {
            tracing::error!("preview pin ran with no authorization enforcer on the request");
            return false;
        };
        authz.may_preview(&mut parts.extensions).await
    })
}
