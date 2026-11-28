//! The workspace middleware's reads of the authorization port
//! (`contracts-authz`): the request's caller, and whether that caller acts as
//! an org through a live assume-role session.
//!
//! The caller carries the host's enforcer (`AppState.authz`), and the
//! assume-role reads go through it, so this crate enforces without
//! `oxy-server-authz` (C4b, 2026-11-28). Each read is the `oxy-server-authz`
//! function the middleware called before, behind the port.

use std::sync::Arc;

use axum::http::Extensions;
use contracts_authz::{ActingAs, AuthzEnforcer, Caller};
use oxy_auth::token::CredentialContext;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

/// The caller of a request: its authenticated user AND the credential the
/// request arrived with, so an API token's grants still decide whether a
/// workspace is reachable and at what ceiling. Authentication's projection
/// (`oxy_auth::caller_parts`), as `oxy_server_authz::Caller::of` builds it,
/// carrying `authz`.
pub(super) fn request_caller(
    user: &AuthenticatedUser,
    extensions: &Extensions,
    authz: &Arc<dyn AuthzEnforcer>,
) -> Caller {
    let credential = extensions.get::<CredentialContext>();
    Caller::from_parts(oxy_auth::caller_parts(user, credential), authz.clone())
}

/// Who `caller` may act as in `org_id`, and whether it holds a live
/// assume-role session there.
///
/// Capability is independent of an active session: `may_act_as` reads platform
/// standing and partner scope only, so it is asked FIRST, and liveness only
/// when there is authority. Both read the CALLER: standing is what the
/// credential carries, and a session belongs to the credential that opened it,
/// so a new-format token does not inherit the browser's. Both fail closed.
pub(super) async fn assume_authority(
    db: &DatabaseConnection,
    caller: &Caller,
    org_id: Uuid,
) -> (Option<ActingAs>, bool) {
    let authz = caller.authz();
    let authority = authz.may_act_as(db, caller, org_id).await;
    let live = authority.is_some() && authz.is_session_live(db, caller, org_id).await;
    (authority, live)
}
