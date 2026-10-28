//! The typed role-guard extractors now live in the `oxy-server-authz` crate:
//! they are pure `FromRequestParts` over request extensions plus the authz
//! model, with no `AppState` coupling, so they belong with the model rather
//! than in this transport crate. The DB-loading middleware that *populates*
//! those extensions (`org_middleware` / `workspace_middleware` /
//! `local_context_middleware`) stays here.
//!
//! Re-exported at the original path so existing
//! `middlewares::role_guards::{OrgOwner, WorkspaceAdmin, ...}` call sites —
//! including the `oxy_app::api::middlewares::role_guards::*` path used by
//! integration tests — keep resolving unchanged.
pub use oxy_server_authz::role_guards::*;

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;

/// Who may create a workspace in an org (`POST /orgs/{org_id}/onboarding/{demo,new,github}`).
///
/// With the apps-first experience on (`feature_flags::APPS_FIRST_EXPERIENCE`), Oxy
/// staff only: [`OrgWorkspaceProvisioner`], an org owner or admin who is also staff
/// with `operate_platform` over the org, because customers do not build. Off, an
/// org owner or admin ([`OrgAdmin`]), as before that experience existed, so the flag
/// off behaves as it did.
#[derive(Debug)]
pub struct OrgWorkspaceCreator(pub oxy_server_authz::org_context::OrgContext);

impl<S> FromRequestParts<S> for OrgWorkspaceCreator
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        if crate::server::feature_flags::apps_first_experience() {
            OrgWorkspaceProvisioner::from_request_parts(parts, state)
                .await
                .map(|OrgWorkspaceProvisioner(ctx)| OrgWorkspaceCreator(ctx))
        } else {
            OrgAdmin::from_request_parts(parts, state)
                .await
                .map(|OrgAdmin(ctx)| OrgWorkspaceCreator(ctx))
        }
    }
}
